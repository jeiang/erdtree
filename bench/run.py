#!/usr/bin/env python3
"""Performance ratchet runner for erd.

For each (case, rep): checks that the candidate's stdout and exit code are byte-identical to the
frozen reference binary, then times both binaries interleaved (warm cache, stdout to /dev/null)
and records speed = ref median / candidate median (0 when the output differs).

Writes results.jsonl, errors.jsonl and traces/<id>_rep<k>.json (train ids only) under --out.
Resumable: (case, rep) slots already in results.jsonl or errors.jsonl are skipped.

Usage: run.py --variant NAME --out DIR [--bin PATH] [--reps N] [--only ID ...]
              [--train-ids FILE] [--no-verify-corpus]
"""

import argparse
import json
import os
import shutil
import statistics
import subprocess
import sys
import threading
import time

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)
sys.path.insert(0, HERE)
import corpus  # noqa: E402

CASE_TIMEOUT_S = 180  # hard per-run ceiling
MIN_ITERS, MAX_ITERS, TARGET_S = 7, 25, 1.0  # timed runs per binary per rep

def run_once(argv, cwd, env, stdout):
    """Runs argv; returns (exit code, wall seconds, cpu seconds, rusage)."""
    t0 = time.perf_counter()
    p = subprocess.Popen(argv, cwd=cwd, env=env, stdout=stdout, stderr=subprocess.DEVNULL)
    fired = []
    timer = threading.Timer(CASE_TIMEOUT_S, lambda: (fired.append(1), p.kill()))
    timer.start()
    _, status, ru = os.wait4(p.pid, 0)
    wall = time.perf_counter() - t0
    timer.cancel()
    p.returncode = os.waitstatus_to_exitcode(status)
    if fired:
        raise TimeoutError(f"exceeded {CASE_TIMEOUT_S}s")
    return p.returncode, wall, ru.ru_utime + ru.ru_stime, ru

def capture(argv, cwd, env):
    t0 = time.perf_counter()
    p = subprocess.run(argv, cwd=cwd, env=env, capture_output=True, timeout=CASE_TIMEOUT_S)
    return p.returncode, p.stdout, p.stderr, time.perf_counter() - t0


def build(out_dir):
    """Release-builds the working tree and copies the binary into out_dir."""
    cmd = ["nix", "develop", "-c", "cargo", "build", "--release", "--locked"]
    p = subprocess.run(cmd, cwd=REPO, capture_output=True, text=True)
    if p.returncode != 0:
        raise RuntimeError("build failed:\n" + p.stderr[-4000:])
    dst = os.path.join(out_dir, "erd")
    shutil.copy2(os.path.join(REPO, "target", "release", "erd"), dst)
    return dst


def first_diff(a, b):
    al, bl = a.splitlines(), b.splitlines()
    for i, (x, y) in enumerate(zip(al, bl)):
        if x != y:
            return f"line {i + 1}: ref={x[:200]!r} cand={y[:200]!r}"
    return f"line count ref={len(al)} cand={len(bl)}"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--variant", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--bin", help="candidate binary; default: build the working tree")
    ap.add_argument("--reps", type=int, default=2)
    ap.add_argument("--only", nargs="*")
    ap.add_argument("--train-ids", help="file with one train case id per line; traces only for these")
    ap.add_argument("--no-verify-corpus", action="store_true")
    a = ap.parse_args()

    spec = json.load(open(os.path.join(HERE, "cases.json")))
    croot = os.path.expanduser(spec["corpus_root"])
    ref = os.path.expanduser(spec["ref_bin"])
    env = {"PATH": "/usr/bin:/bin", "HOME": os.environ["HOME"], "LANG": "en_US.UTF-8", "TZ": "UTC", **spec["env"]}
    os.makedirs(os.path.join(a.out, "traces"), exist_ok=True)
    res_path, err_path = os.path.join(a.out, "results.jsonl"), os.path.join(a.out, "errors.jsonl")

    done = set()
    for path in (res_path, err_path):
        if os.path.exists(path):
            for line in open(path):
                row = json.loads(line)
                done.add((row["prompt_id"], row["rep"]))
    train = set()
    if a.train_ids and os.path.exists(a.train_ids):
        train = {x.strip() for x in open(a.train_ids) if x.strip()}
    cases = [c for c in spec["cases"] if not a.only or c["id"] in a.only]

    def error(case, rep, cls, msg):
        with open(err_path, "a") as f:
            f.write(json.dumps({"prompt_id": case["id"], "rep": rep, "tags": [case["tag"]], "class": cls, "error": msg[-2000:]}) + "\n")

    if not a.no_verify_corpus:
        manifest = json.load(open(os.path.join(croot, "manifest.json")))
        needed = {c["args"][-1] for c in cases} | {x for c in cases for x in c["args"] if x in manifest}
        for name in sorted(needed & set(manifest)):
            if corpus.fingerprint(croot, name) != manifest[name]:
                sys.exit(f"corpus {name} drifted from manifest; regenerate with bench/corpus.py")

    try:
        cand = os.path.abspath(a.bin) if a.bin else build(a.out)
    except RuntimeError as e:
        for c in cases:
            for rep in range(a.reps):
                if (c["id"], rep) not in done:
                    error(c, rep, "build", str(e))
        sys.exit(1)
    model = f"erd:{a.variant}"

    for c in cases:
        argv_tail = spec["common_args"] + c["args"]
        for rep in range(a.reps):
            if (c["id"], rep) in done:
                continue
            try:
                rc_r, out_r, _, t_r = capture([ref] + argv_tail, croot, env)
                rc_c, out_c, err_c, t_c = capture([cand] + argv_tail, croot, env)
                correct = rc_r == rc_c and out_r == out_c
                # Interleaved timing, alternating which binary goes first; one warm-up each.
                iters = max(MIN_ITERS, min(MAX_ITERS, int(TARGET_S / max(t_r, t_c, 1e-3))))
                walls = {"ref": [], "cand": []}
                cpus = {"ref": [], "cand": []}
                last_ru = None
                with open(os.devnull, "wb") as null:
                    for i in range(iters + 1):
                        order = [("ref", ref), ("cand", cand)] if i % 2 == 0 else [("cand", cand), ("ref", ref)]
                        for who, b in order:
                            _, w, cpu, ru = run_once([b] + argv_tail, croot, env, null)
                            if i:
                                walls[who].append(w)
                                cpus[who].append(cpu)
                                if who == "cand":
                                    last_ru = ru
            except (TimeoutError, subprocess.TimeoutExpired) as e:
                error(c, rep, "timeout", str(e))
                continue
            med_r, med_c = statistics.median(walls["ref"]), statistics.median(walls["cand"])
            speed = med_r / med_c if correct else 0.0
            expl = (
                f"ref {med_r * 1e3:.2f} ms, cand {med_c * 1e3:.2f} ms over {iters} iters"
                if correct
                else f"OUTPUT DIFFERS (rc ref={rc_r} cand={rc_c}): {first_diff(out_r.decode('utf-8', 'replace'), out_c.decode('utf-8', 'replace'))}"
            )
            row = {
                "prompt_id": c["id"],
                "rep": rep,
                "prompt": "erd " + " ".join(argv_tail),
                "tags": [c["tag"]],
                "grade": {"speed": speed, "correct": 1.0 if correct else 0.0},
                "explanation": expl,
                "status": "ok",
                "model": model,
                "usage": {"input_tokens": 0, "output_tokens": 0},
                "latency_s": med_c,
                "ref_latency_s": med_r,
                "cpu_s": statistics.median(cpus["cand"]),
                "ref_cpu_s": statistics.median(cpus["ref"]),
                "iters": iters,
            }
            with open(res_path, "a") as f:
                f.write(json.dumps(row) + "\n")
            if c["id"] in train:
                ru = last_ru
                stats = (
                    f"{expl}\n"
                    f"walls ref ms: {[round(x * 1e3, 2) for x in walls['ref']]}\n"
                    f"walls cand ms: {[round(x * 1e3, 2) for x in walls['cand']]}\n"
                    f"cpu (user+sys) median ref {row['ref_cpu_s'] * 1e3:.1f} ms, cand {row['cpu_s'] * 1e3:.1f} ms\n"
                    f"last cand rusage: utime={ru.ru_utime:.4f}s stime={ru.ru_stime:.4f}s maxrss={ru.ru_maxrss}B "
                    f"minflt={ru.ru_minflt} majflt={ru.ru_majflt} nvcsw={ru.ru_nvcsw} nivcsw={ru.ru_nivcsw}\n"
                    f"stdout bytes: {len(out_c)}; stderr: {err_c[-500:].decode('utf-8', 'replace')!r}"
                )
                trace = [
                    {"role": "system", "content": f"erd performance case {c['id']} (tag {c['tag']}), cwd {croot}"},
                    {"role": "user", "content": row["prompt"]},
                    {"role": "assistant", "content": stats},
                ]
                with open(os.path.join(a.out, "traces", f"{c['id']}_rep{rep}.json"), "w") as f:
                    json.dump(trace, f, indent=1)
            print(f"{c['id']} rep{rep}: {expl}", flush=True)


if __name__ == "__main__":
    main()

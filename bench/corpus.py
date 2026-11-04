#!/usr/bin/env python3
"""Builds the frozen benchmark corpora for the erd performance ratchet.

Deterministic (fixed seed, fixed mtimes). Corpora live outside any git checkout so that parent
.gitignore files do not apply. `fingerprint()` lets the runner detect drift.

Usage: corpus.py [--root DIR] [--only NAME ...]
"""

import argparse
import hashlib
import json
import os
import random
import shutil
import subprocess
import sys

ROOT = os.path.expanduser("~/.cache/erd-bench/corpus")
MTIME = 1_700_000_000  # fixed timestamp for generated entries
WORDS = (
    "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi rho "
    "sigma tau upsilon phi chi psi omega fn let mut struct impl pub use mod return match"
).split()
REAL_SRC = os.path.expanduser("~/.cargo/registry/src")


def touch(path, size, rng, text=False):
    with open(path, "wb") as f:
        if size:
            if text:
                out, n = [], 0
                while n < size:
                    line = " ".join(rng.choice(WORDS) for _ in range(rng.randint(0, 12))) + "\n"
                    out.append(line)
                    n += len(line)
                f.write("".join(out).encode()[:size])
            else:
                f.write(rng.randbytes(size) if hasattr(rng, "randbytes") else os.urandom(size))
    os.utime(path, (MTIME, MTIME + rng.randint(0, 10_000_000)))


def name(rng, i):
    stem = "".join(rng.choice("abcdefghijklmnopqrstuvwxyzABCDEFGHIJ_-0123456789") for _ in range(rng.randint(3, 14)))
    ext = rng.choice(["", ".rs", ".txt", ".json", ".md", ".c", ".h", ".png", ".tar.gz", ".so", ".py"])
    return f"{stem}{i}{ext}"


def gen_wide(d, rng):
    """One directory with 100k files (split threshold, one huge listing)."""
    for i in range(100_000):
        touch(os.path.join(d, name(rng, i)), rng.choice([0, 0, 12, 300, 4096, 9000]), rng)


def gen_tree(d, rng, fanout=5, depth=5, files=15):
    """Balanced tree: fanout^depth directories, `files` files each."""
    def rec(p, lvl):
        for i in range(files):
            touch(os.path.join(p, name(rng, i)), rng.randint(0, 6000), rng)
        if lvl < depth:
            for j in range(fanout):
                c = os.path.join(p, f"d{lvl}_{j}")
                os.mkdir(c)
                rec(c, lvl + 1)
    rec(d, 1)


def gen_smalldirs(d, rng):
    """node_modules-like: 15k packages, each a dir with a few tiny files and sometimes a lib dir."""
    for i in range(15_000):
        p = os.path.join(d, f"pkg-{i:05d}")
        os.mkdir(p)
        touch(os.path.join(p, "package.json"), rng.randint(200, 900), rng, text=True)
        touch(os.path.join(p, "index.js"), rng.randint(50, 3000), rng, text=True)
        if i % 3 == 0:
            os.mkdir(os.path.join(p, "lib"))
            for k in range(rng.randint(1, 4)):
                touch(os.path.join(p, "lib", f"m{k}.js"), rng.randint(10, 2000), rng, text=True)
        if i % 7 == 0:
            touch(os.path.join(p, ".npmignore"), 20, rng, text=True)


def gen_deep(d, rng):
    """A chain 300 levels deep (one-letter names, within macOS PATH_MAX) with 3 files per level,
    plus a second shallower branch."""
    p = d
    for lvl in range(300):
        for k in range(3):
            touch(os.path.join(p, f"f{k}"), rng.randint(0, 2000), rng)
        p = os.path.join(p, "d")
        os.mkdir(p)
    q = os.path.join(d, "side")
    os.mkdir(q)
    gen_tree(q, rng, fanout=3, depth=4, files=5)


def gen_repo(d, rng):
    """Git repository: gitignore-heavy (ignored target/ and node_modules/), nested ignores, hidden files."""
    subprocess.run(["git", "init", "-q", d], check=True)
    with open(os.path.join(d, ".gitignore"), "w") as f:
        f.write("/target\nnode_modules/\n*.log\n*.tmp\nbuild-*/\n!keep.log\n")
    src = os.path.join(d, "src")
    os.mkdir(src)
    gen_tree(src, rng, fanout=4, depth=4, files=12)
    for sub in ("d1_0", "d1_1"):
        with open(os.path.join(src, sub, ".gitignore"), "w") as f:
            f.write("*.rs\n!lib.rs\ngen/\n")
    for i in range(3000):
        touch(os.path.join(src, f"noise{i}.log" if i % 2 else f"scratch{i}.tmp"), 100, rng)
    touch(os.path.join(src, "keep.log"), 100, rng)
    tgt = os.path.join(d, "target")
    os.mkdir(tgt)
    gen_tree(tgt, rng, fanout=6, depth=4, files=20)
    nm = os.path.join(d, "web", "node_modules")
    os.makedirs(nm)
    gen_smalldirs_n(nm, rng, 3000)
    for i in range(500):
        touch(os.path.join(d, f".hidden{i}"), 50, rng)
    os.mkdir(os.path.join(d, ".config"))
    gen_tree(os.path.join(d, ".config"), rng, fanout=3, depth=3, files=8)


def gen_smalldirs_n(d, rng, n):
    for i in range(n):
        p = os.path.join(d, f"p{i}")
        os.mkdir(p)
        touch(os.path.join(p, "index.js"), rng.randint(50, 3000), rng, text=True)
        touch(os.path.join(p, "package.json"), 300, rng, text=True)


def gen_text(d, rng):
    """1500 text files of 2-120 KB for --disk-usage line|word."""
    for i in range(30):
        p = os.path.join(d, f"chapter{i:02d}")
        os.mkdir(p)
        for k in range(50):
            touch(os.path.join(p, f"part{k:02d}.txt"), rng.randint(2_000, 120_000), rng, text=True)


def gen_links(d, rng):
    """Symlinks (files, dirs, dangling, loops) and hard links for --follow and dedupe."""
    data = os.path.join(d, "data")
    os.mkdir(data)
    gen_tree(data, rng, fanout=4, depth=3, files=20)
    files = [os.path.join(r, f) for r, _, fs in sorted(os.walk(data)) for f in sorted(fs)]
    dirs = [os.path.join(r, x) for r, ds, _ in sorted(os.walk(data)) for x in sorted(ds)]
    ln = os.path.join(d, "links")
    os.mkdir(ln)
    for i, f in enumerate(files[:1500]):
        os.symlink(os.path.relpath(f, ln), os.path.join(ln, f"s{i}"))
    for i, x in enumerate(dirs[:15]):
        os.symlink(os.path.relpath(x, ln), os.path.join(ln, f"sd{i}"))
    for i in range(200):
        os.symlink(f"missing{i}", os.path.join(ln, f"dangling{i}"))
    os.symlink("..", os.path.join(ln, "loop"))
    hl = os.path.join(d, "hard")
    os.mkdir(hl)
    for i, f in enumerate(files[:1200]):
        os.link(f, os.path.join(hl, f"h{i}"))


def gen_real(d, rng):
    """Frozen APFS clone of ~/.cargo/registry/src: real crate sources (~88k entries)."""
    os.rmdir(d)
    subprocess.run(["cp", "-cRp", REAL_SRC, d], check=True)


def gen_tiny(d, rng):
    """Small tree: process startup and fixed costs dominate."""
    gen_tree(d, rng, fanout=3, depth=2, files=6)


GENERATORS = {
    "wide": gen_wide,
    "tree": lambda d, rng: gen_tree(d, rng, fanout=7, depth=5, files=15),
    "smalldirs": gen_smalldirs,
    "deep": gen_deep,
    "repo": gen_repo,
    "text": gen_text,
    "links": gen_links,
    "real": gen_real,
    "tiny": gen_tiny,
}


def fingerprint(root, name):
    """Entry count, byte total and a hash of sorted (path, size, mtime) for one corpus."""
    h = hashlib.sha256()
    n = total = 0
    base = os.path.join(root, name)
    for r, ds, fs in os.walk(base):
        ds.sort()
        for f in sorted(fs) + ds:
            p = os.path.join(r, f)
            st = os.lstat(p)
            n += 1
            total += st.st_size if not os.path.isdir(p) else 0
            h.update(f"{os.path.relpath(p, base)}\0{st.st_size if not os.path.isdir(p) else 0}\0{int(st.st_mtime)}\n".encode())
    return {"entries": n, "bytes": total, "sha256": h.hexdigest()}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", default=ROOT)
    ap.add_argument("--only", nargs="*")
    a = ap.parse_args()
    os.makedirs(a.root, exist_ok=True)
    manifest_path = os.path.join(a.root, "manifest.json")
    manifest = json.load(open(manifest_path)) if os.path.exists(manifest_path) else {}
    for name, gen in GENERATORS.items():
        if a.only and name not in a.only:
            continue
        d = os.path.join(a.root, name)
        if os.path.exists(d):
            shutil.rmtree(d)
        os.mkdir(d)
        print(f"generating {name}", file=sys.stderr, flush=True)
        gen(d, random.Random(f"erd-bench-{name}"))
        os.utime(d, (MTIME, MTIME))
        manifest[name] = fingerprint(a.root, name)
        print(f"  {manifest[name]['entries']} entries", file=sys.stderr, flush=True)
    with open(manifest_path, "w") as f:
        json.dump(manifest, f, indent=1, sort_keys=True)


if __name__ == "__main__":
    main()

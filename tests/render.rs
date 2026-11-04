//! The parallel row writer must print exactly what a single thread prints, and the prefixes it
//! works out per row must equal those of the sequential plan that `--follow` still uses (the
//! two differ only for trees with symlinks, which these have none of).

use std::{fs, path::Path, process::Command};
use tempfile::TempDir;

/// Runs `erd` with `args` and returns stdout, which must be a success.
fn erd(args: &[&str]) -> Vec<u8> {
    let out = Command::new(env!("CARGO_BIN_EXE_erd"))
        .args(["--no-config", "--disk-usage", "logical", "--sort", "name"])
        .args(args)
        .env_remove("NO_COLOR")
        .env_remove("LS_COLORS")
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

/// A tree of about 12000 entries with uneven fan-out, empty directories and depth up to 7, which
/// is more rows than one thread formats inline.
fn big_tree() -> TempDir {
    fn next(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    fn fill(dir: &Path, state: &mut u64, depth: usize, budget: &mut usize) {
        let entries = 1 + next(state) % 9;

        for k in 0..entries {
            if *budget == 0 {
                return;
            }
            *budget -= 1;

            if depth < 7 && next(state) % 3 == 1 {
                let sub = dir.join(format!("d{k}"));
                fs::create_dir(&sub).unwrap();
                fill(&sub, state, depth + 1, budget);
            } else {
                let len = usize::try_from(next(state) % 40).unwrap();
                fs::write(dir.join(format!("f{k}.txt")), "x".repeat(len)).unwrap();
            }
        }
    }

    let dir = TempDir::new().unwrap();
    let mut state = 0x2545_F491_4F6C_DD1D;
    let mut budget = 12_000;

    // Several top-level directories, as `fill` stops early when it runs out of entries.
    for k in 0..40 {
        let sub = dir.path().join(format!("top{k}"));
        fs::create_dir(&sub).unwrap();
        fill(&sub, &mut state, 1, &mut budget);
    }

    dir
}

const LAYOUTS: [&[&str]; 7] = [
    &[],
    &["-y", "inverted"],
    &["-y", "flat"],
    &["-y", "iflat"],
    &["-L", "2"],
    &["-y", "inverted", "-L", "3"],
    &["--prune", "-p", "f1"],
];

#[test]
fn threads_print_the_same_as_one() {
    let dir = big_tree();
    let root = dir.path().to_str().unwrap();

    for layout in LAYOUTS {
        for color in ["none", "force"] {
            let mut args = layout.to_vec();
            args.extend(["-C", color, root]);

            let one = erd(&[&["--threads", "1"], &args[..]].concat());
            let four = erd(&[&["--threads", "4"], &args[..]].concat());

            assert!(one.iter().filter(|&&b| b == b'\n').count() > 100);
            assert!(one == four, "{layout:?} {color}");
        }
    }
}

#[test]
fn prefixes_equal_those_of_the_sequential_plan() {
    let dir = big_tree();
    let root = dir.path().to_str().unwrap();

    for layout in [
        &["-y", "inverted"][..],
        &[][..],
        &["-L", "2"],
        &["-y", "inverted", "-L", "4"],
        &["--dirs-only"],
        &["--prune", "-p", "f1"],
        &["--min-size", "20"],
        &["-s", "rsize"],
    ] {
        for color in ["none", "force"] {
            let mut args = vec!["--threads", "4", "-C", color];
            args.extend_from_slice(layout);
            args.push(root);

            let cursor = erd(&args);
            args.push("--follow");
            let plan = erd(&args);

            assert!(cursor == plan, "{layout:?} {color}");
        }
    }
}

#[cfg(unix)]
#[test]
fn flat_paths_replace_invalid_utf8() {
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt};

    let dir = TempDir::new().unwrap();
    let sub = dir.path().join(OsStr::from_bytes(b"d\xe2\x82"));
    fs::create_dir(&sub).unwrap();
    fs::write(sub.join(OsStr::from_bytes(b"bad\xffname")), "x").unwrap();

    let out = String::from_utf8(erd(&[
        "-y",
        "iflat",
        "--suppress-size",
        dir.path().to_str().unwrap(),
    ]))
    .unwrap();

    assert!(out.contains("d\u{fffd}/bad\u{fffd}name\n"), "{out}");
}

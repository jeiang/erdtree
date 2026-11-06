//! The parallel row writer must print exactly what a single thread prints, also with the link
//! theme of `--follow`, which each worker works out from a row's ancestors.

use std::{fs, path::Path};
use tempfile::TempDir;
use utils::Erd;

mod utils;

/// Stdout of a successful `erd` with `threads` workers, escapes included.
fn erd(threads: usize, args: &[&str]) -> String {
    Erd::new().threads(threads).args(args).ok().raw()
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

            let one = erd(1, &args);

            assert!(one.lines().count() > 100);
            assert!(one == erd(4, &args), "{layout:?} {color}");
        }
    }
}

/// `--threads 0` means the default thread count, in every phase, and prints what one thread
/// prints.
#[test]
fn zero_threads_print_the_same_as_one() {
    let dir = big_tree();
    let root = dir.path().to_str().unwrap();

    for layout in [&[][..], &["-y", "flat"], &["-y", "iflat"]] {
        let mut args = layout.to_vec();
        args.extend(["-C", "none", root]);

        let one = erd(1, &args);

        assert!(one.lines().count() > 100);
        assert!(one == erd(0, &args), "{layout:?}");
    }
}

/// Workers that start in the middle of a followed directory, or of one below it, paint the
/// same branches as one thread that saw every row before.
#[cfg(unix)]
#[test]
fn follow_threads_print_the_same_as_one() {
    use std::os::unix::fs::symlink;

    let dir = big_tree();
    let root = dir.path();

    // Links to whole top-level directories, one of them inside a linked directory, and links
    // to a file and to nothing.
    symlink("../top10", root.join("top3/lnk")).unwrap();
    symlink("../top20", root.join("top17/lnk")).unwrap();
    symlink("../top25", root.join("top10/lnk")).unwrap();
    symlink("../top3/lnk", root.join("top33/lnk")).unwrap();
    fs::write(root.join("file"), "x").unwrap();
    symlink("../file", root.join("top5/flnk")).unwrap();
    symlink("missing", root.join("top6/dangling")).unwrap();

    let root = root.to_str().unwrap();

    for layout in [
        &[][..],
        &["-y", "inverted"],
        &["-L", "3"],
        &["-y", "inverted", "-L", "2"],
        &["--dirs-only"],
        &["--prune", "-p", "f1"],
        &["--min-size", "20"],
        &["-s", "rsize"],
    ] {
        for color in ["none", "force"] {
            let mut args = vec!["--follow", "-C", color];
            args.extend_from_slice(layout);
            args.push(root);

            let one = erd(1, &args);

            assert!(one.lines().count() > 100);
            assert!(one == erd(4, &args), "{layout:?} {color}");

            if color == "force" {
                assert!(utils::in_link_theme(&one), "{layout:?}");
            }
        }
    }
}

/// Without symlinks the link theme of `--follow` paints nothing.
#[test]
fn follow_without_links_prints_the_tree_theme() {
    let dir = big_tree();
    let root = dir.path().to_str().unwrap();

    for layout in [
        &[][..],
        &["-y", "inverted"],
        &["-L", "2"],
        &["--prune", "-p", "f1"],
    ] {
        let mut args = vec!["-C", "force"];
        args.extend_from_slice(layout);
        args.push(root);

        let plain = erd(4, &args);
        args.push("--follow");

        assert!(plain == erd(4, &args), "{layout:?}");
    }
}

// APFS and other UTF-8-only file systems refuse such names (EILSEQ).
#[cfg(target_os = "linux")]
#[test]
fn flat_paths_replace_invalid_utf8() {
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt};

    let dir = TempDir::new().unwrap();
    let sub = dir.path().join(OsStr::from_bytes(b"d\xe2\x82"));
    fs::create_dir(&sub).unwrap();
    fs::write(sub.join(OsStr::from_bytes(b"bad\xffname")), "x").unwrap();

    let out = erd(
        1,
        &[
            "-y",
            "iflat",
            "--suppress-size",
            dir.path().to_str().unwrap(),
        ],
    );

    assert!(out.contains("d\u{fffd}/bad\u{fffd}name\n"), "{out}");
}

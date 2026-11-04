//! A tree deeper than `PATH_MAX` can only be walked relative to directory descriptors.

#![cfg(target_os = "linux")]

mod utils;

use std::{
    ffi::CString,
    fs::File,
    io::Write,
    os::fd::{AsRawFd, FromRawFd, RawFd},
    path::Path,
};
use utils::run_cmd;

/// Creates a chain of `levels` directories named `name` below `root`, and a file in the deepest
/// one, without ever using a path longer than one name. `fill` adds more to the deepest one.
fn chain(root: &Path, name: &str, levels: usize, fill: impl FnOnce(RawFd)) {
    let mut dir = File::open(root).unwrap();
    let name = CString::new(name).unwrap();

    for _ in 0..levels {
        // SAFETY: `dir` is an open descriptor and `name` is NUL-terminated.
        unsafe {
            assert_eq!(libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), 0o755), 0);
            let fd = libc::openat(dir.as_raw_fd(), name.as_ptr(), libc::O_RDONLY);
            assert!(fd >= 0);
            dir = File::from_raw_fd(fd);
        }
    }

    let leaf = CString::new("leaf.txt").unwrap();

    // SAFETY: as above.
    unsafe {
        let fd = libc::openat(
            dir.as_raw_fd(),
            leaf.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT,
            0o644,
        );
        assert!(fd >= 0);
        libc::close(fd);
    }

    fill(dir.as_raw_fd());
}

/// Creates the file `name` with `contents` in the directory open as `dir`.
fn write_file(dir: RawFd, name: &str, contents: &str) {
    let name = CString::new(name).unwrap();

    // SAFETY: `dir` is an open descriptor and `name` is NUL-terminated.
    let fd = unsafe {
        libc::openat(
            dir,
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_CLOEXEC,
            0o644,
        )
    };
    assert!(fd >= 0);

    // SAFETY: `fd` is a new descriptor that nothing else owns.
    unsafe { File::from_raw_fd(fd) }
        .write_all(contents.as_bytes())
        .unwrap();
}

/// Creates the directory `name` and the symlink `link` to `target` in the directory open as `dir`.
fn make_dir(dir: RawFd, name: &str) {
    let name = CString::new(name).unwrap();

    // SAFETY: as above.
    assert_eq!(unsafe { libc::mkdirat(dir, name.as_ptr(), 0o755) }, 0);
}

fn make_link(dir: RawFd, link: &str, target: &str) {
    let link = CString::new(link).unwrap();
    let target = CString::new(target).unwrap();

    // SAFETY: as above.
    assert_eq!(
        unsafe { libc::symlinkat(target.as_ptr(), dir, link.as_ptr()) },
        0
    );
}

/// The names in the flat listing of a tree whose deepest path is above `PATH_MAX` (60 levels of
/// 100 bytes), built by `fill`.
fn deep_listing(args: &[&str], fill: impl FnOnce(RawFd)) -> Vec<String> {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("deep");
    std::fs::create_dir(&root).unwrap();
    chain(&root, &"d".repeat(100), 60, fill);

    let root = root.to_str().unwrap();
    let mut all = vec![
        "--no-report",
        "--suppress-size",
        "--layout",
        "flat",
        "--hidden",
    ];
    all.extend_from_slice(args);
    all.push(root);

    run_cmd(&all)
        .lines()
        .filter_map(|line| Some(line.rsplit('/').next()?.to_string()))
        .filter(|name| !name.starts_with("dd"))
        .collect()
}

#[test]
fn walks_paths_longer_than_path_max() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("deep");
    std::fs::create_dir(&root).unwrap();

    // 60 levels of 100 bytes: the deepest path is above PATH_MAX (4096).
    chain(&root, &"d".repeat(100), 60, |_| {});

    let out = run_cmd(&[
        "--no-report",
        "--suppress-size",
        "--layout",
        "flat",
        root.to_str().unwrap(),
    ]);

    assert_eq!(out.lines().count(), 62, "{out}");
    assert!(out.lines().any(|l| l.ends_with("leaf.txt")));
}

#[test]
fn applies_ignore_files_beyond_path_max() {
    let out = deep_listing(&[], |dir| {
        write_file(dir, ".ignore", "*.log\nsecret\n");
        write_file(dir, "a.log", "");
        write_file(dir, "secret", "");
        write_file(dir, "keep.txt", "");
    });

    assert!(out.contains(&"keep.txt".into()), "{out:?}");
    assert!(
        !out.contains(&"a.log".into()) && !out.contains(&"secret".into()),
        "{out:?}"
    );
}

#[test]
fn recognizes_git_directories_beyond_path_max() {
    // `.gitignore` applies only inside a repository, and a `.git` directory makes one.
    let out = deep_listing(&[], |dir| {
        make_dir(dir, ".git");
        write_file(dir, ".gitignore", "*.log\n");
        write_file(dir, "a.log", "");
        write_file(dir, "keep.txt", "");
    });

    assert!(out.contains(&"keep.txt".into()), "{out:?}");
    assert!(!out.contains(&"a.log".into()), "{out:?}");
}

#[test]
fn follows_links_beyond_path_max() {
    let out = deep_listing(&["--follow"], |dir| {
        make_dir(dir, "realdir");
        write_file(dir, "marker", "");
        make_link(dir, "dirlnk", "realdir");
        // Loops are still found: the link leads to a directory being walked.
        make_link(dir, "selfloop", ".");
    });

    assert!(out.contains(&"dirlnk".into()), "{out:?}");
    assert!(!out.contains(&"selfloop".into()), "{out:?}");
}

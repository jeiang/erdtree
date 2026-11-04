//! A tree deeper than `PATH_MAX` can only be walked relative to directory descriptors.

#![cfg(target_os = "linux")]

mod utils;

use std::{
    ffi::CString,
    fs::File,
    os::fd::{AsRawFd, FromRawFd},
    path::Path,
};
use utils::run_cmd;

/// Creates a chain of `levels` directories named `name` below `root`, and a file in the deepest
/// one, without ever using a path longer than one name.
fn chain(root: &Path, name: &str, levels: usize) {
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
}

#[test]
fn walks_paths_longer_than_path_max() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("deep");
    std::fs::create_dir(&root).unwrap();

    // 60 levels of 100 bytes: the deepest path is above PATH_MAX (4096).
    chain(&root, &"d".repeat(100), 60);

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

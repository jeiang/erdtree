//! A tree deeper than `PATH_MAX` can only be walked relative to directory descriptors.

#![cfg(target_os = "linux")]

mod utils;

use std::{
    ffi::CString,
    fs::File,
    io::Write,
    os::fd::{AsRawFd, FromRawFd, RawFd},
    path::PathBuf,
};
use tempfile::TempDir;

/// A tree `deep` of 60 directories of 100 bytes, so that the deepest path is above `PATH_MAX`
/// (4096), holding `leaf.txt` and what `fill` adds in the deepest directory.
fn deep_tree(fill: impl FnOnce(RawFd)) -> (TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("deep");
    std::fs::create_dir(&root).unwrap();
    let deepest = utils::chain(&root, &"d".repeat(100), 60, |_| {});
    write_file(deepest.as_raw_fd(), "leaf.txt", "");
    fill(deepest.as_raw_fd());
    (tmp, root)
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

/// Creates the directory `name` in the directory open as `dir`.
fn make_dir(dir: RawFd, name: &str) {
    let name = CString::new(name).unwrap();

    // SAFETY: as above.
    assert_eq!(unsafe { libc::mkdirat(dir, name.as_ptr(), 0o755) }, 0);
}

/// Creates the symlink `link` to `target` in the directory open as `dir`.
fn make_link(dir: RawFd, link: &str, target: &str) {
    let link = CString::new(link).unwrap();
    let target = CString::new(target).unwrap();

    // SAFETY: as above.
    assert_eq!(
        unsafe { libc::symlinkat(target.as_ptr(), dir, link.as_ptr()) },
        0
    );
}

/// The names below the root of [`deep_tree`], the chain left out, `--hidden` and `args` given.
fn deep_listing(args: &[&str], fill: impl FnOnce(RawFd)) -> Vec<String> {
    let (_tmp, root) = deep_tree(fill);
    utils::paths(&root, &[&["--hidden"], args].concat())
        .into_iter()
        .map(|path| path.rsplit('/').next().unwrap().to_owned())
        .filter(|name| !name.starts_with("dd"))
        .collect()
}

#[test]
fn walks_paths_longer_than_path_max() {
    let (_tmp, root) = deep_tree(|_| {});
    let paths = utils::paths(&root, &[]);

    assert_eq!(paths.len(), 61, "{paths:?}");
    assert!(paths.iter().any(|p| p.ends_with("leaf.txt")));
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

//! The walk holds parent directories open while their subdirectories wait. Under a low limit of
//! open files it must not run out and lose subtrees.

#![cfg(target_os = "linux")]

use std::{
    ffi::CString,
    fs::File,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::process::CommandExt,
    },
    process::Command,
};

/// Creates `levels` nested directories `d`, each also holding a sibling directory `s`, so that
/// every level has a subdirectory waiting while the chain is followed.
fn chain_with_siblings(root: &std::path::Path, levels: usize) {
    let mut dir = File::open(root).unwrap();
    let (d, s) = (CString::new("d").unwrap(), CString::new("s").unwrap());

    for _ in 0..levels {
        // SAFETY: `dir` is an open descriptor and the names are NUL-terminated.
        unsafe {
            assert_eq!(libc::mkdirat(dir.as_raw_fd(), s.as_ptr(), 0o755), 0);
            assert_eq!(libc::mkdirat(dir.as_raw_fd(), d.as_ptr(), 0o755), 0);
            let fd = libc::openat(dir.as_raw_fd(), d.as_ptr(), libc::O_RDONLY);
            assert!(fd >= 0);
            dir = File::from_raw_fd(fd);
        }
    }
}

/// Number of lines `erd` prints for `root` with at most `limit` open files (soft and hard).
fn lines_under_limit(root: &std::path::Path, threads: &str, limit: libc::rlim_t) -> usize {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_erd"));
    cmd.args([
        "--no-config",
        "--no-report",
        "--suppress-size",
        "--layout",
        "flat",
        "--threads",
        threads,
    ])
    .arg(root);

    // SAFETY: `setrlimit` is async-signal-safe.
    unsafe {
        cmd.pre_exec(move || {
            let lim = libc::rlimit {
                rlim_cur: limit,
                rlim_max: limit,
            };
            if libc::setrlimit(libc::RLIMIT_NOFILE, &raw const lim) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().lines().count()
}

#[test]
fn deep_trees_are_complete_under_a_low_file_limit() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("deep");
    std::fs::create_dir(&root).unwrap();

    // More levels than the limit has files, so holding a parent per level would run out.
    chain_with_siblings(&root, 300);

    // The root, and a `d` and an `s` per level.
    for threads in ["1", "4"] {
        assert_eq!(
            lines_under_limit(&root, threads, 256),
            1 + 2 * 300,
            "{threads} threads"
        );
    }
}

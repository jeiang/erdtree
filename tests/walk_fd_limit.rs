//! The walk holds parent directories open while their subdirectories wait. Under a low limit of
//! open files it must not run out and lose subtrees.

#![cfg(target_os = "linux")]

use std::{ffi::CString, os::unix::process::CommandExt};

mod utils;

/// Number of lines `erd` prints for `root` with at most `limit` open files (soft and hard).
fn lines_under_limit(root: &std::path::Path, threads: usize, limit: libc::rlim_t) -> usize {
    let (mut cmd, _home) = utils::Erd::new()
        .threads(threads)
        .args(["--no-report", "--suppress-size", "--layout", "flat"])
        .arg(root)
        .command();

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

    // More levels than the limit has files, so holding a parent per level would run out. A
    // sibling `s` per level has to wait while the chain is followed.
    let s = CString::new("s").unwrap();
    utils::chain(&root, "d", 300, |dir| {
        // SAFETY: `dir` is an open descriptor and `s` is NUL-terminated.
        assert_eq!(unsafe { libc::mkdirat(dir, s.as_ptr(), 0o755) }, 0);
    });

    // The root, and a `d` and an `s` per level.
    for threads in [1, 4] {
        assert_eq!(
            lines_under_limit(&root, threads, 256),
            1 + 2 * 300,
            "{threads} threads"
        );
    }
}

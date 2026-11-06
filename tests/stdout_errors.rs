//! Failures writing to stdout: broken pipes are quiet, other errors are reported.

#![cfg(unix)]

use std::process::Stdio;
use utils::Erd;

mod utils;

fn big_tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    // Output must exceed the pipe buffer (64 KiB on Linux).
    for i in 0..3000 {
        std::fs::write(
            dir.path().join(format!("file-with-a-long-name-{i:05}.txt")),
            "x",
        )
        .unwrap();
    }
    dir
}

#[test]
fn broken_pipe_exits_quietly() {
    let dir = big_tree();
    let (mut cmd, _home) = Erd::new().arg("--no-progress").arg(dir.path()).command();
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    // Close the read end without reading anything.
    drop(child.stdout.take());

    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "status: {:?}", output.status);
    assert!(
        output.stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn write_error_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "x").unwrap();

    let full = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let (mut cmd, _home) = Erd::new().arg("--no-progress").arg(dir.path()).command();
    let output = cmd.stdout(full).stderr(Stdio::piped()).output().unwrap();

    assert!(!output.status.success(), "status: {:?}", output.status);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.starts_with("erd: "), "stderr: {stderr}");
    assert!(
        stderr.contains("No space left on device"),
        "stderr: {stderr}"
    );
}

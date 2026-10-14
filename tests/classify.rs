#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt};
use tempfile::TempDir;

mod utils;

fn fixture() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir(root.join("d")).unwrap();
    fs::write(root.join("f"), "x").unwrap();
    fs::write(root.join("x.sh"), "#!/bin/sh\n").unwrap();
    fs::set_permissions(root.join("x.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink("f", root.join("link")).unwrap();
    dir
}

/// Entry rows (without the root and the summary), with the tree prefix removed.
fn rows(dir: &TempDir, args: &[&str]) -> Vec<String> {
    let mut all = vec!["--suppress-size"];
    all.extend_from_slice(args);
    all.push(dir.path().to_str().unwrap());

    let out = utils::run_cmd(&all);
    let (rows, _summary) = out.split_once("\n\n").unwrap();
    let mut rows = rows
        .lines()
        .map(|l| l.trim_start_matches([' ', '┌', '├', '─']).to_string())
        .collect::<Vec<_>>();
    rows.pop(); // root
    rows
}

#[test]
fn classify_marks_entry_kinds() {
    let dir = fixture();

    assert_eq!(
        rows(&dir, &["--classify"]),
        ["x.sh*", "link@ \u{2192} f", "f", "d/"]
    );
    assert_eq!(
        rows(&dir, &["--layout", "flat", "-F"]),
        ["x.sh*", "link@", "f", "d/"]
    );
    assert_eq!(rows(&dir, &[]), ["x.sh", "link \u{2192} f", "f", "d"]);
}

#[test]
fn classify_marks_fifo() {
    let dir = TempDir::new().unwrap();
    let fifo = dir.path().join("p");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );

    assert_eq!(rows(&dir, &["-F"]), ["p|"]);
}

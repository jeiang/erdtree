#![cfg(unix)]

use std::fs::{self, File};
use tempfile::TempDir;

mod utils;

/// The size column of each row of the colored output, escapes included.
fn size_columns(args: &[&str]) -> Vec<String> {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("root");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("a"), "abcde").unwrap();
    // Sparse files: logical sizes in every prefix without writing them.
    for (name, len) in [
        ("b", 1_500),
        ("c", 2_000_000),
        ("d", 3_000_000_000),
        ("e", 4_000_000_000_000),
    ] {
        File::create(root.join(name)).unwrap().set_len(len).unwrap();
    }

    let mut all = vec!["--color", "force"];
    all.extend_from_slice(args);
    all.push(root.to_str().unwrap());

    let out = String::from_utf8(utils::run_cmd_raw(&all, &[])).unwrap();
    out.lines()
        .take(6)
        .map(|l| l.split_once("\u{1b}[0m ").unwrap().0.to_string())
        .collect()
}

/// Sizes are colored by their closest prefix, also when shown in bytes.
#[test]
fn sizes_are_colored_by_prefix() {
    let colored = |color: &str, size: &str| format!("\u{1b}[1;{color}m{size}");

    assert_eq!(
        size_columns(&["--unit", "si"]),
        [
            colored("34", "4000000000000 B"),
            colored("31", "   3000000000 B"),
            colored("32", "      2000000 B"),
            colored("33", "         1500 B"),
            colored("36", "            5 B"),
            colored("34", "4003002001505 B"),
        ]
    );
    assert_eq!(
        size_columns(&["--unit", "si", "-H"]),
        [
            colored("34", "4.0 TB"),
            colored("31", "3.0 GB"),
            colored("32", "2.0 MB"),
            colored("33", "1.5 KB"),
            colored("36", "  5  B"),
            colored("34", "4.0 TB"),
        ]
    );
}

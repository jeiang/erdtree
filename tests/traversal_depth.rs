//! `--traversal-depth` limits the walk; `--level` only limits the display.

use indoc::indoc;
use std::{fs, process::Command};
use tempfile::TempDir;

mod utils;

#[test]
fn level_alone_keeps_full_tree_sizes_and_counts() {
    assert_eq!(
        utils::run_cmd(&["--level", "1", "tests/data"]),
        indoc!(
            "143 B ┌─ the_yellow_king
 100 B ├─ nylarlathotep.txt
 161 B ├─ nemesis.txt
  83 B ├─ necronomicon.txt
 446 B ├─ lipsum
 308 B ├─ dream_cycle
1241 B data

3 directories, 6 files"
        )
    );
}

#[test]
fn level_alone_with_suppress_size_counts_full_tree() {
    let out = utils::run_cmd(&["--level", "1", "--suppress-size", "tests/data"]);
    assert!(out.ends_with("3 directories, 6 files"), "{out}");
}

#[test]
fn traversal_depth_limits_sizes_and_counts() {
    let out = utils::run_cmd(&["--traversal-depth", "1", "tests/data"]);
    assert!(out.ends_with("3 directories, 3 files"), "{out}");
    assert!(!out.contains("1241 B data"), "{out}");
    assert!(!out.contains("lipsum.txt"), "{out}");
}

#[test]
fn level_and_traversal_depth_list_same_entries() {
    let names = |args: &[&str]| -> Vec<String> {
        let mut v: Vec<String> = utils::run_cmd(args)
            .lines()
            .filter(|l| l.contains("─ "))
            .map(|l| l.split("─ ").nth(1).unwrap().to_string())
            .collect();
        v.sort();
        v
    };
    assert_eq!(
        names(&["--level", "1", "tests/data"]),
        names(&["--level", "1", "--traversal-depth", "1", "tests/data"])
    );
    assert_eq!(
        names(&["--level", "2", "tests/data"]),
        names(&["--level", "2", "--traversal-depth", "2", "tests/data"])
    );
}

#[test]
fn traversal_depth_from_config() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join(".erdtree.toml");
    for key in ["traversal-depth", "traversal_depth"] {
        fs::write(&path, format!("{key} = 1\n")).unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_erd"))
            .env_remove("ERDTREE_CONFIG_PATH")
            .env("HOME", dir.path())
            .env("XDG_CONFIG_HOME", dir.path().join("xdg"))
            .env("ERDTREE_TOML_PATH", &path)
            .args(["--threads", "1", "--color", "none", "tests/data"])
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            stdout.trim_end().ends_with("3 directories, 3 files"),
            "{key}: {stdout}"
        );
    }
}

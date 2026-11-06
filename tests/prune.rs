use indoc::indoc;
use std::fs;
use tempfile::TempDir;

mod utils;

/// `t/` holding `b.md`, `full/a.txt` and the empty directories `empty` and `full/inner_empty`.
fn with_empty_dirs() -> (TempDir, String) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("t");
    fs::create_dir_all(root.join("empty")).unwrap();
    fs::create_dir_all(root.join("full/inner_empty")).unwrap();
    fs::write(root.join("full/a.txt"), "aa").unwrap();
    fs::write(root.join("b.md"), "bb").unwrap();
    let root = root.to_str().unwrap().to_owned();
    (tmp, root)
}

const PRUNED: &str = indoc!(
    "2 B    ┌─ a.txt
    2 B ┌─ full
    2 B ├─ b.md
    4 B t

    1 directory, 2 files"
);

#[test]
fn prune_hides_empty_directories() {
    let (_tmp, root) = with_empty_dirs();

    let out = utils::run_cmd(&[&root]);
    assert!(out.contains("─ empty\n"), "{out}");
    assert!(out.contains("─ inner_empty\n"), "{out}");

    assert_eq!(utils::run_cmd(&["--prune", &root]), PRUNED);
}

/// A pattern prunes the directories left without matches, `--prune` or not.
#[test]
fn pattern_implies_prune() {
    let (_tmp, root) = with_empty_dirs();
    assert_eq!(utils::run_cmd(&["--glob", "--pattern", "*", &root]), PRUNED);
}

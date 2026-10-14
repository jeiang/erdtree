use std::fs;
use tempfile::TempDir;

mod utils;

/// `root/repo` is a git work tree (the walker only reads `.gitignore` inside one) with a
/// gitignored directory, a hidden file, an excluded file, and two shown files.
fn fixture() -> TempDir {
    let dir = TempDir::new().unwrap();
    let repo = dir.path().join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    fs::create_dir_all(repo.join("target/debug")).unwrap();
    fs::create_dir_all(repo.join("src")).unwrap();
    fs::write(repo.join(".gitignore"), "target/\n").unwrap();
    fs::write(repo.join("target/debug/bin"), "").unwrap();
    fs::write(repo.join("src/main.rs"), "").unwrap();
    fs::write(repo.join("src/notes.tmp"), "").unwrap();
    fs::write(repo.join("README"), "").unwrap();
    dir
}

fn summary(dir: &TempDir, args: &[&str]) -> String {
    let repo = dir.path().join("repo");
    let mut all = vec!["--suppress-size"];
    all.extend_from_slice(args);
    all.push(repo.to_str().unwrap());

    utils::run_cmd(&all).lines().last().unwrap().to_owned()
}

#[test]
fn counts_entries_skipped_by_the_walk() {
    let dir = fixture();

    assert_eq!(summary(&dir, &[]), "1 directory, 3 files");
    // `.git`, `.gitignore`, `target` (counted once, not descended into).
    assert_eq!(
        summary(&dir, &["--report-ignored"]),
        "1 directory, 3 files, 3 ignored"
    );
    assert_eq!(
        summary(&dir, &["--report-ignored", "--exclude", "*.tmp"]),
        "1 directory, 2 files, 4 ignored"
    );
    // `.git` is shown with `--hidden` (its contents are walked), so only `target` is ignored.
    assert_eq!(
        summary(&dir, &["--report-ignored", "--hidden"])
            .split(", ")
            .last(),
        Some("1 ignored")
    );
}

#[test]
fn unread_directories_at_the_traversal_limit_are_not_counted() {
    let dir = fixture();

    // `src` is shown but not read, so `src/notes.tmp` is neither shown nor counted.
    assert_eq!(
        summary(
            &dir,
            &[
                "--report-ignored",
                "--traversal-depth",
                "1",
                "--exclude",
                "*.tmp"
            ]
        ),
        "1 directory, 1 file, 3 ignored"
    );
}

#[test]
fn conflicts_with_pattern() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_erd"))
        .args([
            "--no-config",
            "--report-ignored",
            "--pattern",
            "x",
            "tests/data",
        ])
        .output()
        .unwrap();

    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot be used with"));
}

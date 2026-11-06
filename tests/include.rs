use std::fs;
use tempfile::TempDir;

mod utils;

fn fixture() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join(".github/workflows")).unwrap();
    fs::create_dir_all(root.join(".hidden")).unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    for file in [
        ".github/workflows/ci.yml",
        ".github/.keep",
        ".hidden/x",
        ".env",
        "src/main.rs",
    ] {
        fs::write(root.join(file), "").unwrap();
    }
    dir
}

#[test]
fn include_shows_matching_hidden_entries_only() {
    let dir = fixture();
    let root = dir.path();

    assert_eq!(utils::paths(root, &[]), ["src", "src/main.rs"]);
    assert_eq!(
        utils::paths(root, &["--include", ".github"]),
        [
            ".github",
            ".github/workflows",
            ".github/workflows/ci.yml",
            "src",
            "src/main.rs"
        ]
    );
    assert_eq!(
        utils::paths(root, &["--include", ".github", "--include", ".github/**"]),
        [
            ".github",
            ".github/.keep",
            ".github/workflows",
            ".github/workflows/ci.yml",
            "src",
            "src/main.rs"
        ]
    );
}

#[test]
fn include_composes_with_pattern() {
    let dir = fixture();

    assert_eq!(
        utils::paths(
            dir.path(),
            &["--include", ".github", "--pattern", "ci.yml", "--prune"]
        ),
        [".github", ".github/workflows", ".github/workflows/ci.yml"]
    );
}

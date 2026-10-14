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

/// Paths of the shown entries, root excluded, sorted.
fn shown(dir: &TempDir, args: &[&str]) -> Vec<String> {
    let mut all = vec!["--layout", "iflat", "--suppress-size", "--no-report"];
    all.extend_from_slice(args);
    all.push(dir.path().to_str().unwrap());

    let mut paths = utils::run_cmd(&all)
        .lines()
        .skip(1)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

#[test]
fn include_shows_matching_hidden_entries_only() {
    let dir = fixture();

    assert_eq!(shown(&dir, &[]), ["src", "src/main.rs"]);
    assert_eq!(
        shown(&dir, &["--include", ".github"]),
        [
            ".github",
            ".github/workflows",
            ".github/workflows/ci.yml",
            "src",
            "src/main.rs"
        ]
    );
    assert_eq!(
        shown(&dir, &["--include", ".github", "--include", ".github/**"]),
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
        shown(
            &dir,
            &["--include", ".github", "--pattern", "ci.yml", "--prune"]
        ),
        [".github", ".github/workflows", ".github/workflows/ci.yml"]
    );
}

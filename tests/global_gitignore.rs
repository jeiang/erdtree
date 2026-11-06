//! The global gitignore is read from the home `erd` runs with; the harness gives every run an
//! empty one, so the machine running the tests cannot change their output.

use std::fs;
use tempfile::TempDir;
use utils::Erd;

mod utils;

/// A repository holding `a.txt` and `b.md`.
fn repo() -> (TempDir, String) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::write(root.join("a.txt"), "a").unwrap();
    fs::write(root.join("b.md"), "b").unwrap();
    let root = root.to_str().unwrap().to_owned();
    (tmp, root)
}

fn listing(erd: Erd, root: &str) -> String {
    erd.args(["--suppress-size", root]).ok().stdout()
}

#[test]
fn isolated_home_has_no_global_gitignore() {
    let (_tmp, root) = repo();
    let out = listing(Erd::new(), &root);
    assert!(out.contains("a.txt"), "{out}");
}

#[cfg(unix)]
#[test]
fn xdg_global_gitignore_applies() {
    let (_tmp, root) = repo();

    let erd = Erd::new();
    let ignore = erd.home().join("xdg/git/ignore");
    fs::create_dir_all(ignore.parent().unwrap()).unwrap();
    fs::write(ignore, "*.txt\n").unwrap();
    let out = listing(erd, &root);
    assert!(!out.contains("a.txt"), "{out}");
    assert!(out.contains("b.md"), "{out}");
}

/// `Erd` whose home `.gitconfig` names an excludes file ignoring `*.txt`.
#[cfg(unix)]
fn with_excludes_file() -> Erd {
    let erd = Erd::new();
    let excludes = erd.home().join("excludes");
    fs::write(&excludes, "*.txt\n").unwrap();
    let excludes = excludes.to_str().unwrap();
    fs::write(
        erd.home().join(".gitconfig"),
        format!("[core]\n\texcludesFile = {excludes}\n"),
    )
    .unwrap();
    erd
}

#[cfg(unix)]
#[test]
fn excludes_file_of_home_gitconfig_applies() {
    let (_tmp, root) = repo();

    let out = listing(with_excludes_file(), &root);
    assert!(!out.contains("a.txt"), "{out}");
    assert!(out.contains("b.md"), "{out}");

    let out = listing(with_excludes_file().arg("--no-ignore"), &root);
    assert!(out.contains("a.txt"), "{out}");
}

//! Case-insensitive name sorting (`--sort nameci|rnameci`).

use std::{fs, path::Path, process::Command};
use tempfile::{Builder, TempDir};

/// Creates a tree with names whose case-sensitive and case-insensitive orders differ, plus a
/// directory `Delta` containing `eta` and `Zeta`.
fn fixture() -> TempDir {
    let dir = Builder::new().prefix("root").tempdir().unwrap();
    for name in ["Apple", "banana", "Cherry", "apricot"] {
        fs::write(dir.path().join(name), "x").unwrap();
    }
    fs::create_dir(dir.path().join("Delta")).unwrap();
    fs::write(dir.path().join("Delta").join("eta"), "x").unwrap();
    fs::write(dir.path().join("Delta").join("Zeta"), "x").unwrap();
    dir
}

/// Runs `erd` on `root` and returns the printed entry names (root excluded, tree-drawing
/// characters and parent directories stripped) in output order. With `config`, it is used as the
/// `.erdtree.toml`; otherwise config is skipped.
fn names(root: &Path, config: Option<&str>, args: &[&str]) -> Vec<String> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_erd"));
    cmd.env_remove("ERDTREE_TOML_PATH")
        .env_remove("ERDTREE_CONFIG_PATH")
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join("xdg"))
        .env("APPDATA", root.join("appdata"))
        .args(["--threads", "1", "--color", "none", "--suppress-size"]);

    let cfg_dir = TempDir::new().unwrap();
    if let Some(config) = config {
        let path = cfg_dir.path().join(".erdtree.toml");
        fs::write(&path, config).unwrap();
        cmd.env("ERDTREE_TOML_PATH", path);
    } else {
        cmd.arg("--no-config");
    }

    let out = cmd.args(args).arg(root).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let root_name = root.file_name().unwrap().to_str().unwrap().to_owned();
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .take_while(|l| !l.is_empty())
        .map(|l| {
            let l = l.trim_start_matches(|c: char| !c.is_alphanumeric());
            l.rsplit(['/', '\\']).next().unwrap().to_string()
        })
        .filter(|n| *n != root_name)
        .collect()
}

#[test]
fn nameci_regular_layout() {
    let dir = fixture();
    let got = names(dir.path(), None, &["--sort", "nameci"]);
    assert_eq!(
        got,
        [
            "Zeta", "eta", "Delta", "Cherry", "banana", "apricot", "Apple"
        ]
    );
}

#[test]
fn rnameci_regular_layout() {
    let dir = fixture();
    let got = names(dir.path(), None, &["--sort", "rnameci"]);
    assert_eq!(
        got,
        [
            "Apple", "apricot", "banana", "Cherry", "eta", "Zeta", "Delta"
        ]
    );
}

#[test]
fn nameci_differs_from_name() {
    let dir = fixture();
    let ci = names(dir.path(), None, &["--sort", "nameci"]);
    let cs = names(dir.path(), None, &["--sort", "name"]);
    assert_ne!(ci, cs);
}

#[test]
fn rnameci_is_exact_reverse_of_nameci_in_flat_layouts() {
    let dir = fixture();
    for layout in ["flat", "iflat"] {
        let mut fwd = names(dir.path(), None, &["--sort", "nameci", "--layout", layout]);
        let rev = names(dir.path(), None, &["--sort", "rnameci", "--layout", layout]);
        fwd.reverse();
        assert_eq!(fwd, rev, "layout {layout}");
    }
}

#[test]
fn nameci_inverted_layout() {
    let dir = fixture();
    let got = names(
        dir.path(),
        None,
        &["--sort", "nameci", "--layout", "inverted"],
    );
    assert_eq!(
        got,
        [
            "Apple", "apricot", "banana", "Cherry", "Delta", "eta", "Zeta"
        ]
    );
}

#[test]
fn nameci_flat_layouts_sort_globally() {
    let dir = fixture();
    let flat = names(dir.path(), None, &["--sort", "nameci", "--layout", "flat"]);
    assert_eq!(
        flat,
        [
            "Zeta", "eta", "Delta", "Cherry", "banana", "apricot", "Apple"
        ]
    );
    let iflat = names(dir.path(), None, &["--sort", "nameci", "--layout", "iflat"]);
    assert_eq!(
        iflat,
        [
            "Apple", "apricot", "banana", "Cherry", "Delta", "eta", "Zeta"
        ]
    );
}

#[test]
fn nameci_dir_order_first() {
    let dir = fixture();
    let got = names(
        dir.path(),
        None,
        &["--sort", "nameci", "--dir-order", "first"],
    );
    assert_eq!(
        got,
        [
            "Zeta", "eta", "Delta", "Cherry", "banana", "apricot", "Apple"
        ]
    );
}

#[test]
fn nameci_dir_order_last() {
    let dir = fixture();
    let got = names(
        dir.path(),
        None,
        &["--sort", "nameci", "--dir-order", "last"],
    );
    assert_eq!(
        got,
        [
            "Cherry", "banana", "apricot", "Apple", "Zeta", "eta", "Delta"
        ]
    );
}

#[test]
fn nameci_from_config() {
    let dir = fixture();
    let got = names(dir.path(), Some("sort = \"nameci\"\n"), &[]);
    assert_eq!(
        got,
        [
            "Zeta", "eta", "Delta", "Cherry", "banana", "apricot", "Apple"
        ]
    );
}

/// Names differing only in case tie under folding and fall back to the case-sensitive order.
/// Needs a case-sensitive filesystem.
#[cfg(target_os = "linux")]
#[test]
fn nameci_tie_breaks_case_sensitively() {
    let dir = Builder::new().prefix("root").tempdir().unwrap();
    for name in ["b", "B", "a", "C"] {
        fs::write(dir.path().join(name), "x").unwrap();
    }
    let fwd = names(dir.path(), None, &["--sort", "nameci", "--layout", "iflat"]);
    assert_eq!(fwd, ["a", "B", "b", "C"]);
    let rev = names(
        dir.path(),
        None,
        &["--sort", "rnameci", "--layout", "iflat"],
    );
    assert_eq!(rev, ["C", "b", "B", "a"]);
}

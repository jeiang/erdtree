//! Case-insensitive name sorting (`--sort nameci|rnameci`).

use std::{fs, path::Path};
use tempfile::{Builder, TempDir};
use utils::Erd;

mod utils;

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

/// The entry names `erd` prints for `root`, root excluded, in output order.
fn names_of(erd: Erd, root: &Path, args: &[&str]) -> Vec<String> {
    let out = erd
        .arg("--suppress-size")
        .args(args)
        .arg(root)
        .ok()
        .stdout();
    let root_name = root.file_name().unwrap().to_str().unwrap();
    utils::rows(&out)
        .into_iter()
        .map(|row| row.rsplit('/').next().unwrap().to_owned())
        .filter(|name| name != root_name)
        .collect()
}

fn names(root: &Path, args: &[&str]) -> Vec<String> {
    names_of(Erd::new(), root, args)
}

#[test]
fn nameci_regular_layout() {
    let dir = fixture();
    let got = names(dir.path(), &["--sort", "nameci"]);
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
    let got = names(dir.path(), &["--sort", "rnameci"]);
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
    let ci = names(dir.path(), &["--sort", "nameci"]);
    let cs = names(dir.path(), &["--sort", "name"]);
    assert_ne!(ci, cs);
}

#[test]
fn flat_layouts_follow_tree_order_for_name_sorts() {
    let dir = fixture();
    for sort in ["nameci", "rnameci"] {
        for (flat, tree) in [("flat", "regular"), ("iflat", "inverted")] {
            let flat_names = names(dir.path(), &["--sort", sort, "--layout", flat]);
            let tree_names = names(dir.path(), &["--sort", sort, "--layout", tree]);
            assert_eq!(flat_names, tree_names, "--sort {sort} --layout {flat}");
        }
    }
}

#[test]
fn nameci_inverted_layout() {
    let dir = fixture();
    let got = names(dir.path(), &["--sort", "nameci", "--layout", "inverted"]);
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
    let flat = names(dir.path(), &["--sort", "nameci", "--layout", "flat"]);
    assert_eq!(
        flat,
        [
            "Zeta", "eta", "Delta", "Cherry", "banana", "apricot", "Apple"
        ]
    );
    let iflat = names(dir.path(), &["--sort", "nameci", "--layout", "iflat"]);
    assert_eq!(
        iflat,
        [
            "Apple", "apricot", "banana", "Cherry", "Delta", "eta", "Zeta"
        ]
    );
}

/// `Blueberry`, a directory, sorts among the files by name and moves next to `Delta`.
#[test]
fn nameci_dir_order_first() {
    let dir = fixture();
    fs::create_dir(dir.path().join("Blueberry")).unwrap();
    assert_eq!(
        names(dir.path(), &["--sort", "nameci"]),
        [
            "Zeta",
            "eta",
            "Delta",
            "Cherry",
            "Blueberry",
            "banana",
            "apricot",
            "Apple"
        ]
    );
    assert_eq!(
        names(dir.path(), &["--sort", "nameci", "--dir-order", "first"]),
        [
            "Zeta",
            "eta",
            "Delta",
            "Blueberry",
            "Cherry",
            "banana",
            "apricot",
            "Apple"
        ]
    );
}

#[test]
fn nameci_dir_order_last() {
    let dir = fixture();
    let got = names(dir.path(), &["--sort", "nameci", "--dir-order", "last"]);
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
    let got = names_of(Erd::new().config("sort = \"nameci\"\n"), dir.path(), &[]);
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
    let fwd = names(dir.path(), &["--sort", "nameci", "--layout", "iflat"]);
    assert_eq!(fwd, ["a", "B", "b", "C"]);
    let rev = names(dir.path(), &["--sort", "rnameci", "--layout", "iflat"]);
    assert_eq!(rev, ["C", "b", "B", "a"]);
}

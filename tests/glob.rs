use indoc::indoc;
use std::fs;
use tempfile::TempDir;
use utils::Erd;

mod utils;

/// A root `dreamroot/foo` whose parent matches `dream*`, holding `dreamy/c.txt`, `other/b.txt`
/// and `sub/a.txt`.
fn dream_parent() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("dreamroot/foo");
    for file in ["dreamy/c.txt", "other/b.txt", "sub/a.txt"] {
        let path = root.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "").unwrap();
    }
    dir
}

/// `--file-type dir` matches directories below the root only, not the root's ancestors.
#[test]
fn glob_with_filetype_ignores_ancestors_of_root() {
    let dir = dream_parent();
    let args = ["--glob", "--file-type", "dir", "--pattern"];

    let root = dir.path().join("dreamroot/foo");

    assert_eq!(
        utils::paths(&root, &[&args[..], &["dream*"]].concat()),
        ["dreamy", "dreamy/c.txt"]
    );
    assert_eq!(
        utils::paths(&root, &[&args[..], &["!dream*"]].concat()),
        ["other", "other/b.txt", "sub", "sub/a.txt"]
    );
}

#[test]
fn glob() {
    assert_eq!(
        utils::run_cmd(&["--glob", "--pattern", "*.txt", "tests/data"]),
        indoc!(
            "100 B ┌─ nylarlathotep.txt
 161 B ├─ nemesis.txt
  83 B ├─ necronomicon.txt
 446 B │  ┌─ lipsum.txt
 446 B ├─ lipsum
 308 B │  ┌─ polaris.txt
 308 B ├─ dream_cycle
1098 B data

2 directories, 5 files"
        )
    );
}

#[test]
fn glob_negative() {
    assert_eq!(
        utils::run_cmd(&["--glob", "--pattern", "!*.txt", "tests/data"]),
        indoc!(
            "143 B    ┌─ cassildas_song.md
 143 B ┌─ the_yellow_king
 143 B data
 
 1 directory, 1 file"
        )
    )
}

#[test]
fn glob_case_insensitive() {
    assert_eq!(
        utils::run_cmd(&["--iglob", "--pattern", "*.TXT", "tests/data"]),
        indoc!(
            "100 B ┌─ nylarlathotep.txt
 161 B ├─ nemesis.txt
  83 B ├─ necronomicon.txt
 446 B │  ┌─ lipsum.txt
 446 B ├─ lipsum
 308 B │  ┌─ polaris.txt
 308 B ├─ dream_cycle
1098 B data

2 directories, 5 files"
        )
    )
}

#[test]
fn glob_with_filetype() {
    assert_eq!(
        utils::run_cmd(&[
            "--glob",
            "--pattern",
            "dream*",
            "--file-type",
            "dir",
            "tests/data"
        ]),
        indoc!(
            "308 B    ┌─ polaris.txt
 308 B ┌─ dream_cycle
 308 B data
 
 1 directory, 1 file"
        )
    )
}

#[test]
fn negated_glob_with_filetype() {
    assert_eq!(
        utils::run_cmd(&[
            "--glob",
            "--pattern",
            "!dream*",
            "--file-type",
            "dir",
            "tests/data"
        ]),
        indoc!(
            "143 B    ┌─ cassildas_song.md
 143 B ┌─ the_yellow_king
 100 B ├─ nylarlathotep.txt
 161 B ├─ nemesis.txt
  83 B ├─ necronomicon.txt
 446 B │  ┌─ lipsum.txt
 446 B ├─ lipsum
 933 B data
 
 2 directories, 5 files"
        )
    )
}

#[test]
fn glob_empty_set_dir() {
    let err = Erd::new()
        .args([
            "--glob",
            "--pattern",
            "*.txt",
            "--file-type",
            "dir",
            "tests/data",
        ])
        .fails(1);
    assert_eq!(err, "erd: No entries to show with given arguments.\n");
}

#[test]
fn glob_empty_set_file() {
    let err = Erd::new()
        .args(["--glob", "--pattern", "*weewoo*", "tests/data"])
        .fails(1);
    assert_eq!(err, "erd: No entries to show with given arguments.\n");
}

/// A glob containing a slash is anchored to the root; it must match even when the root is given as
/// a relative path (entries are walked via the canonical root).
#[test]
fn glob_negative_anchored_pattern_with_relative_root() {
    assert_eq!(
        utils::run_cmd(&["--glob", "--pattern", "!lipsum/**", "tests/data"]),
        indoc!(
            "143 B    ┌─ cassildas_song.md
143 B ┌─ the_yellow_king
100 B ├─ nylarlathotep.txt
161 B ├─ nemesis.txt
 83 B ├─ necronomicon.txt
308 B │  ┌─ polaris.txt
308 B ├─ dream_cycle
795 B data

2 directories, 5 files"
        )
    );
}

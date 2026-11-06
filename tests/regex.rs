use indoc::indoc;
use std::fs;
use tempfile::TempDir;
use utils::Erd;

mod utils;

/// `--file-type dir` matches directories below the root only, not the root's ancestors.
#[test]
fn regex_file_type_ignores_ancestors_of_root() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("dreamroot/foo");
    for file in ["dreamy/c.txt", "other/b.txt", "sub/a.txt"] {
        let path = root.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "").unwrap();
    }

    assert_eq!(
        utils::paths(&root, &["--pattern", r"^dream.", "--file-type", "dir"]),
        ["dreamy", "dreamy/c.txt"]
    );
}

/// `--file-type link` keeps symlinks whose name matches, plus the directories leading to them.
#[cfg(unix)]
#[test]
fn regex_file_type_link() {
    use std::os::unix::fs::symlink;

    let dir = TempDir::new().unwrap();
    let root = dir.path().join("root");
    fs::create_dir_all(root.join("d")).unwrap();
    fs::create_dir_all(root.join("e")).unwrap();
    fs::write(root.join("a.txt"), "").unwrap();
    fs::write(root.join("e/b.txt"), "").unwrap();
    symlink("a.txt", root.join("l.txt")).unwrap();
    symlink("../a.txt", root.join("d/m.txt")).unwrap();
    symlink("a.txt", root.join("n.md")).unwrap();

    assert_eq!(
        utils::paths(
            &root,
            &["--prune", "--pattern", r"\.txt$", "--file-type", "link"]
        ),
        ["d", "d/m.txt", "l.txt"]
    );
}

#[test]
fn regex() {
    assert_eq!(
        utils::run_cmd(&["--pattern", r"\.txt$", "tests/data"]),
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

    assert_eq!(
        utils::run_cmd(&["--pattern", r"^cassildas.", "--prune", "tests/data"]),
        indoc!(
            "143 B    ┌─ cassildas_song.md
 143 B ┌─ the_yellow_king
 143 B data
 
 1 directory, 1 file"
        )
    );
}

#[test]
fn regex_file_type() {
    assert_eq!(
        utils::run_cmd(&["--pattern", r"^dream.", "--file-type", "dir", "tests/data"]),
        indoc!(
            "308 B    ┌─ polaris.txt
 308 B ┌─ dream_cycle
 308 B data
 
 1 directory, 1 file"
        )
    );
}

/// A file pattern with `--file-type dir` leaves nothing to show.
#[test]
fn regex_empty_set_dir() {
    let err = Erd::new()
        .args(["--pattern", r"\.md$", "--file-type", "dir", "tests/data"])
        .fails(1);
    assert_eq!(err, "erd: No entries to show with given arguments.\n");
}

#[test]
fn regex_empty_set_file() {
    let err = Erd::new()
        .args(["--pattern", "weewoo", "tests/data"])
        .fails(1);
    assert_eq!(err, "erd: No entries to show with given arguments.\n");
}

#[test]
fn invalid_regex() {
    let err = Erd::new()
        .args(["--pattern", "*.txt", "tests/data"])
        .fails(1);
    assert!(err.starts_with("erd: "), "{err}");
    assert!(
        err.contains("repetition operator missing expression"),
        "{err}"
    );
}

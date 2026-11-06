//! `--traversal-depth` limits the walk; `--level` only limits the display.

use utils::Erd;

mod utils;

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

/// The traversal depth shows the directories at the limit, unread, as the level does.
#[test]
fn level_and_traversal_depth_list_same_entries() {
    let names = |args: &[&str]| -> Vec<String> {
        let out = utils::run_cmd(&[&["--suppress-size"], args].concat());
        utils::rows(&out).into_iter().map(str::to_owned).collect()
    };
    assert_eq!(
        names(&["--level", "1", "tests/data"]),
        names(&["--level", "1", "--traversal-depth", "1", "tests/data"])
    );

    let (_tmp, root) = utils::three_levels();
    assert_eq!(
        names(&["--level", "2", &root]),
        ["e.txt", "d.txt", "b", "a", "t"]
    );
    assert_eq!(
        names(&["--level", "2", "--traversal-depth", "2", &root]),
        ["e.txt", "d.txt", "b", "a", "t"]
    );
}

#[test]
fn traversal_depth_from_config() {
    for key in ["traversal-depth", "traversal_depth"] {
        let out = Erd::new()
            .config(&format!("{key} = 1\n"))
            .arg("tests/data")
            .ok()
            .trimmed();
        assert!(out.ends_with("3 directories, 3 files"), "{key}: {out}");
    }
}

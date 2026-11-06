use utils::Erd;

mod utils;

#[test]
fn each_root_is_its_own_tree_with_one_combined_summary() {
    for layout in ["regular", "inverted", "flat", "iflat"] {
        let data = utils::run_cmd_untrimmed(&["--layout", layout, "--no-report", "tests/data"]);
        let flat_data =
            utils::run_cmd_untrimmed(&["--layout", layout, "--no-report", "tests/flat_data"]);

        let both = utils::run_cmd_untrimmed(&["--layout", layout, "tests/data", "tests/flat_data"]);

        // 3 directories, 6 files in tests/data; 3 directories, 3 files in tests/flat_data.
        assert_eq!(
            both,
            format!("{data}\n{flat_data}\n6 directories, 9 files\n"),
            "{layout}"
        );
    }
}

#[test]
fn summarize_prints_one_line_per_root() {
    assert_eq!(
        utils::run_cmd(&["--summarize", "tests/data", "tests/data/lipsum"]),
        // Each root's size column is sized for that root alone.
        "1241 B data\n446 B lipsum"
    );
}

#[test]
fn a_root_without_matches_is_shown_when_another_has_matches() {
    let out = utils::run_cmd(&["--pattern", "polaris", "tests/data", "tests/flat_data"]);
    assert!(out.ends_with("flat_data\n\n1 directory, 1 file"), "{out}");
}

#[test]
fn no_matches_in_any_root_is_an_error() {
    let stderr = Erd::new()
        .args(["--pattern", "zzz", "tests/data", "tests/flat_data"])
        .fails(1);
    assert!(stderr.contains("No entries to show"), "{stderr}");
}

#[test]
fn a_missing_root_is_named_in_the_error() {
    let stderr = Erd::new().args(["tests/data", "tests/missing"]).fails(1);
    assert!(stderr.starts_with("erd: tests/missing: "), "{stderr}");
}

#[test]
fn a_file_root_is_named_in_the_error() {
    let stderr = Erd::new()
        .args(["tests/data", "tests/data/nemesis.txt"])
        .fails(1);
    assert_eq!(stderr, "erd: tests/data/nemesis.txt: Not a directory\n");
}

#[test]
fn level_zero_prints_a_root_without_visible_entries() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join(".hidden"), "x").unwrap();
    let root = dir.path().to_str().unwrap();
    let name = dir.path().file_name().unwrap().to_str().unwrap();

    assert_eq!(utils::run_cmd(&["--summarize", root]), name);
    assert!(Erd::new().arg(root).fails(1).contains("No entries to show"));
}

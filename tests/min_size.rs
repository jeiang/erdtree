use indoc::indoc;
use std::fs;
use tempfile::TempDir;
use utils::Erd;

mod utils;

#[test]
fn hides_small_files_and_keeps_totals() {
    let out = utils::run_cmd(&["--min-size", "150", "tests/data"]);
    assert_eq!(
        out,
        indoc!(
            "
             161 B ┌─ nemesis.txt
             446 B │  ┌─ lipsum.txt
             446 B ├─ lipsum
             308 B │  ┌─ polaris.txt
             308 B ├─ dream_cycle
            1241 B data

            3 directories, 6 files"
        )
        .trim()
    );
}

#[test]
fn directories_below_threshold_are_hidden_with_their_contents() {
    let out = utils::run_cmd(&["--min-size", "400", "tests/data"]);
    assert_eq!(
        out,
        indoc!(
            "
             446 B    ┌─ lipsum.txt
             446 B ┌─ lipsum
            1241 B data

            3 directories, 6 files"
        )
        .trim()
    );
}

#[test]
fn threshold_is_inclusive() {
    let out = utils::run_cmd(&["--min-size", "446", "tests/data"]);
    assert!(out.contains("lipsum.txt"), "{out}");
    assert!(!out.contains("dream_cycle"), "{out}");
}

#[test]
fn zero_hides_nothing() {
    let all = utils::run_cmd(&["tests/data"]);
    assert_eq!(utils::run_cmd(&["--min-size", "0", "tests/data"]), all);
}

#[test]
fn unit_suffixes_are_applied() {
    // 1 KB = 1000 bytes hides everything below the 1241 B root; 1 KiB = 1024 likewise.
    for size in ["1KB", "1kb", "1KiB", "1000B", "1000"] {
        let err = Erd::new().args(["--min-size", size, "tests/data"]).fails(1);
        assert!(err.contains("No entries to show"), "{size}: {err}");
    }

    let out = utils::run_cmd(&["--min-size", "300B", "tests/data"]);
    assert!(out.contains("lipsum.txt"), "{out}");
    assert!(!out.contains("nemesis.txt"), "{out}");
}

#[test]
fn level_is_display_only() {
    let out = utils::run_cmd(&["--min-size", "150", "--level", "1", "tests/data"]);
    assert_eq!(
        out,
        indoc!(
            "
             161 B ┌─ nemesis.txt
             446 B ├─ lipsum
             308 B ├─ dream_cycle
            1241 B data

            3 directories, 6 files"
        )
        .trim()
    );
}

#[test]
fn dirs_only() {
    let out = utils::run_cmd(&["--min-size", "300", "--dirs-only", "tests/data"]);
    assert_eq!(
        out,
        indoc!(
            "
             446 B ┌─ lipsum
             308 B ├─ dream_cycle
            1241 B data

            3 directories"
        )
        .trim()
    );
}

#[test]
fn flat_layout() {
    let out = utils::run_cmd(&["--min-size", "150", "--layout", "flat", "tests/data"]);
    assert_eq!(
        out,
        indoc!(
            "
             161 B   nemesis.txt
             446 B   lipsum/lipsum.txt
             446 B   lipsum
             308 B   dream_cycle/polaris.txt
             308 B   dream_cycle
            1241 B   data

            3 directories, 6 files"
        )
        .trim()
    );
}

#[test]
fn inverted_layout() {
    let out = utils::run_cmd(&["--min-size", "400", "--layout", "inverted", "tests/data"]);
    assert_eq!(
        out,
        indoc!(
            "
            1241 B data
             446 B └─ lipsum
             446 B    └─ lipsum.txt

            3 directories, 6 files"
        )
        .trim()
    );
}

#[test]
fn nothing_to_show_is_an_error() {
    let err = Erd::new()
        .args(["--min-size", "1PB", "tests/data"])
        .fails(1);
    assert!(err.contains("No entries to show"), "{err}");
}

#[test]
fn rejects_invalid_values() {
    for bad in ["abc", "KB", "1.5KB", "5XB", ""] {
        let err = Erd::new().args(["--min-size", bad, "tests/data"]).fails(2);
        assert!(err.contains("--min-size"), "{bad:?}: {err}");
    }
}

#[test]
fn conflicts_with_suppress_size() {
    let err = Erd::new()
        .args(["--min-size", "1", "--suppress-size", "tests/data"])
        .fails(2);
    assert!(err.contains("cannot be used with"), "{err}");
}

#[test]
fn conflicts_with_suppress_size_across_config() {
    let err = Erd::new()
        .config("min-size = \"1\"\n")
        .args(["--suppress-size", "tests/data"])
        .fails(2);
    assert!(err.contains("cannot be used with"), "{err}");
}

#[test]
fn units_rejected_for_non_byte_metrics() {
    for metric in ["line", "word"] {
        let err = Erd::new()
            .args(["--disk-usage", metric, "--min-size", "1KB", "tests/data"])
            .fails(2);
        assert!(err.contains("units are only supported"), "{metric}: {err}");
    }
}

#[test]
fn bare_number_applies_to_line_count() {
    assert_eq!(
        utils::run_cmd(&["--disk-usage", "line", "--min-size", "5", "tests/data"]),
        indoc!(
            "
             6    ┌─ cassildas_song.md
             6 ┌─ the_yellow_king
            10 │  ┌─ polaris.txt
            10 ├─ dream_cycle
            24 data

            3 directories, 6 files"
        )
        .trim()
    );
}

#[test]
fn config_key() {
    let config = "min-size = \"300B\"\ndisk-usage = \"logical\"\n";
    let out = Erd::new().config(config).arg("tests/data").ok().trimmed();
    assert!(out.contains("lipsum.txt"), "{out}");
    assert!(!out.contains("nemesis.txt"), "{out}");
    assert!(out.ends_with("3 directories, 6 files"), "{out}");

    // The command line wins over the config.
    let out = Erd::new()
        .config(config)
        .args(["--min-size", "0", "tests/data"])
        .ok()
        .stdout();
    assert!(out.contains("nemesis.txt"), "{out}");
}

/// The hidden entry has the widest size (`1023 B` against `1.0 KiB`), so it must not widen the
/// size column.
#[test]
fn hidden_widest_entry_does_not_widen_sizes() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("t");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("a"), [0; 1024]).unwrap();
    fs::write(root.join("b"), [0; 1023]).unwrap();
    let root = root.to_string_lossy().into_owned();

    let all = utils::run_cmd(&["--human", &root]);
    assert!(all.starts_with("1023   B ┌─ b"), "{all}");

    let out = utils::run_cmd(&["--human", "--min-size", "1024", &root]);
    assert_eq!(
        out,
        indoc!(
            "
            1.0 KiB ┌─ a
            2.0 KiB t

            2 files"
        )
        .trim()
    );
}

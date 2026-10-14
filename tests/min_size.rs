use indoc::indoc;
use std::{fs, process::Command};
use tempfile::TempDir;

mod utils;

/// Runs `erd` directly (not through cargo) with an arbitrary config and arguments.
fn run_erd(config: Option<&str>, args: &[&str]) -> std::process::Output {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join(".erdtree.toml");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_erd"));

    cmd.env_remove("ERDTREE_TOML_PATH")
        .env_remove("ERDTREE_CONFIG_PATH")
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join("xdg"))
        .env("APPDATA", dir.path().join("appdata"));

    if let Some(config) = config {
        fs::write(&path, config).unwrap();
        cmd.env("ERDTREE_TOML_PATH", &path);
    }

    cmd.args(["--threads", "1", "--color", "none", "--sort", "name"])
        .args(args)
        .output()
        .unwrap()
}

fn stdout(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn stderr(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

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
        let out = run_erd(
            None,
            &[
                "--no-config",
                "--disk-usage",
                "logical",
                "--min-size",
                size,
                "tests/data",
            ],
        );
        assert!(!out.status.success(), "{size}");
        assert!(
            stderr(&out).contains("No entries to show"),
            "{size}: {}",
            stderr(&out)
        );
    }

    let out = run_erd(
        None,
        &[
            "--no-config",
            "--disk-usage",
            "logical",
            "--min-size",
            "0.3KB",
            "tests/data",
        ],
    );
    assert!(!out.status.success());

    let out = run_erd(
        None,
        &[
            "--no-config",
            "--disk-usage",
            "logical",
            "--min-size",
            "300B",
            "tests/data",
        ],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("lipsum.txt"));
    assert!(!stdout(&out).contains("nemesis.txt"));
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
    let out = run_erd(None, &["--no-config", "--min-size", "1PB", "tests/data"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("No entries to show"));
}

#[test]
fn rejects_invalid_values() {
    for bad in ["abc", "KB", "1.5KB", "5XB", ""] {
        let out = run_erd(None, &["--no-config", "--min-size", bad, "tests/data"]);
        assert_eq!(out.status.code(), Some(2), "{bad:?}");
        assert!(
            stderr(&out).contains("--min-size"),
            "{bad:?}: {}",
            stderr(&out)
        );
    }
}

#[test]
fn conflicts_with_suppress_size() {
    let out = run_erd(
        None,
        &[
            "--no-config",
            "--min-size",
            "1",
            "--suppress-size",
            "tests/data",
        ],
    );
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stderr(&out).contains("cannot be used with"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn conflicts_with_suppress_size_across_config() {
    let out = run_erd(
        Some("min-size = \"1\"\n"),
        &["--suppress-size", "tests/data"],
    );
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stderr(&out).contains("cannot be used with"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn units_rejected_for_non_byte_metrics() {
    for metric in ["line", "word"] {
        let out = run_erd(
            None,
            &[
                "--no-config",
                "--disk-usage",
                metric,
                "--min-size",
                "1KB",
                "tests/data",
            ],
        );
        assert_eq!(out.status.code(), Some(2), "{metric}");
        assert!(
            stderr(&out).contains("units are only supported"),
            "{metric}: {}",
            stderr(&out)
        );
    }
}

#[test]
fn bare_number_applies_to_line_count() {
    let out = run_erd(
        None,
        &[
            "--no-config",
            "--disk-usage",
            "line",
            "--min-size",
            "5",
            "tests/data",
        ],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
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
    let out = run_erd(
        Some("min-size = \"300B\"\ndisk-usage = \"logical\"\n"),
        &["tests/data"],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("lipsum.txt"));
    assert!(!stdout(&out).contains("nemesis.txt"));
    assert!(
        stdout(&out).ends_with("3 directories, 6 files"),
        "{}",
        stdout(&out)
    );

    // The command line wins over the config.
    let out = run_erd(
        Some("min-size = \"300B\"\ndisk-usage = \"logical\"\n"),
        &["--min-size", "0", "tests/data"],
    );
    assert!(stdout(&out).contains("nemesis.txt"));
}

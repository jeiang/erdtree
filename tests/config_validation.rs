//! Argument relations (`requires`, mutual exclusion) are validated once, on the merged
//! command-line and config arguments.

use utils::Erd;

mod utils;

/// Stdout of a successful `erd` with `config` as its config file.
fn with_config(config: &str, args: &[&str]) -> String {
    Erd::new().config(config).args(args).ok().stdout()
}

#[test]
fn config_glob_with_cli_pattern() {
    let out = with_config("glob = true\n", &["--pattern", "*.txt", "tests/data"]);
    assert!(out.contains("lipsum.txt"), "{out}");
    assert!(!out.contains("cassildas_song.md"), "{out}");
}

#[test]
fn config_glob_with_config_pattern() {
    let out = with_config("glob = true\npattern = \"*.txt\"\n", &["tests/data"]);
    assert!(out.contains("lipsum.txt"), "{out}");
    assert!(!out.contains("cassildas_song.md"), "{out}");
}

#[test]
fn cli_glob_with_config_pattern() {
    let out = with_config("pattern = \"*.txt\"\n", &["--glob", "tests/data"]);
    assert!(out.contains("lipsum.txt"), "{out}");
    assert!(!out.contains("cassildas_song.md"), "{out}");
}

/// `lipsum|nemesis` matches the directory `lipsum` and the file `nemesis.txt`; only the
/// directory is kept.
#[test]
fn config_file_type_with_cli_pattern() {
    let out = with_config(
        "file-type = \"dir\"\n",
        &["--pattern", "lipsum|nemesis", "tests/data"],
    );
    assert!(out.contains("lipsum.txt"), "{out}");
    assert!(!out.contains("nemesis.txt"), "{out}");
}

#[test]
fn config_glob_without_pattern_errors() {
    let err = Erd::new()
        .config("glob = true\n")
        .arg("tests/data")
        .fails(2);
    assert!(
        err.contains("required arguments were not provided"),
        "{err}"
    );
    assert!(err.contains("--pattern"), "{err}");
    assert!(err.contains("Usage: erd"), "{err}");
}

/// An unknown config key reads as a usage error of `erd`.
#[test]
fn config_unknown_key_usage_names_erd() {
    let err = Erd::new()
        .config("bogus = true\n")
        .arg("tests/data")
        .fails(1);
    assert!(err.contains("unexpected argument '--bogus'"), "{err}");
    let usage = format!(
        "Usage: erd{} [OPTIONS] [DIR]...",
        std::env::consts::EXE_SUFFIX
    );
    assert!(err.contains(&usage), "{err}");
}

#[test]
fn config_conflicting_glob_and_iglob_errors() {
    let err = Erd::new()
        .config("glob = true\niglob = true\npattern = \"x\"\n")
        .arg("tests/data")
        .fails(2);
    assert!(err.contains("cannot be used with"), "{err}");
}

#[cfg(unix)]
#[test]
fn config_octal_with_cli_long() {
    let out = with_config("octal = true\n", &["--long", "tests/data"]);
    let row = out.lines().find(|l| l.ends_with("nemesis.txt")).unwrap();
    assert!(row.starts_with("0644 "), "{out}");
}

#[test]
fn config_no_git_with_cli_hidden() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("r");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join("f"), "").unwrap();

    let out = with_config("no-git = true\n", &["--hidden", root.to_str().unwrap()]);
    assert!(out.contains("─ f"), "{out}");
    assert!(!out.contains(".git"), "{out}");
}

#[test]
fn config_invalid_value_is_a_config_error() {
    let err = Erd::new()
        .config("sort = \"bogus\"\n")
        .arg("tests/data")
        .fails(1);
    assert!(
        err.contains("A configuration file was found but failed to parse"),
        "{err}"
    );
}

#[test]
fn named_table_glob_with_cli_pattern() {
    let out = with_config(
        "[g]\nglob = true\n",
        &["-c", "g", "--pattern", "*.txt", "tests/data"],
    );
    assert!(out.contains("lipsum.txt"), "{out}");
    assert!(!out.contains("cassildas_song.md"), "{out}");
}

#[test]
fn cli_only_missing_pattern_still_errors() {
    let err = Erd::new().args(["--glob", "tests/data"]).fails(2);
    assert!(err.starts_with("error: "), "{err}");
    assert!(err.contains("--pattern"), "{err}");
    assert!(err.contains("Usage: erd"), "{err}");
}

#[test]
fn config_exclude_array() {
    let out = with_config("exclude = [\"lipsum\", \"dream_cycle\"]\n", &["tests/data"]);
    assert!(
        !out.contains("lipsum") && !out.contains("dream_cycle"),
        "{out}"
    );
    assert!(out.contains("the_yellow_king"), "{out}");
}

#[test]
fn cli_exclude_overrides_config_exclude() {
    let out = with_config(
        "exclude = [\"lipsum\", \"dream_cycle\"]\n",
        &["--exclude", "the_yellow_king", "tests/data"],
    );
    assert!(
        out.contains("lipsum") && out.contains("dream_cycle"),
        "{out}"
    );
    assert!(!out.contains("the_yellow_king"), "{out}");
}

#[test]
fn config_exclude_non_string_element_errors() {
    let err = Erd::new()
        .config("exclude = [1]\n")
        .arg("tests/data")
        .fails(1);
    assert_eq!(err, "erd: 'exclude' has a type that is invalid\n");
}

#[test]
fn config_negative_integer_errors() {
    let err = Erd::new().config("level = -1\n").arg("tests/data").fails(1);
    assert_eq!(
        err,
        "erd: 'level' is required to be a pointer-sized unsigned integer type\n"
    );
}

#[test]
fn config_value_applies_and_cli_value_wins() {
    let level1 = with_config("", &["--level", "1", "tests/data"]);
    let level2 = with_config("", &["--level", "2", "tests/data"]);
    assert_ne!(level1, level2);

    assert_eq!(with_config("level = 1\n", &["tests/data"]), level1);
    assert_eq!(
        with_config("level = 1\n", &["--level", "2", "tests/data"]),
        level2
    );
}

#[test]
fn usage_errors_and_help_are_unchanged_with_and_without_config() {
    for config in [None, Some("level = 1\n")] {
        let erd = || match config {
            Some(config) => Erd::new().config(config),
            None => Erd::new(),
        };

        let err = erd().arg("--bogus").fails(2);
        assert!(err.contains("unexpected argument"), "{err}");

        let out = erd().arg("--help").ok().stdout();
        assert!(out.contains("Usage: erd"), "{out}");

        let out = erd().arg("--version").ok().stdout();
        assert!(out.starts_with("erdtree "), "{out}");
    }
}

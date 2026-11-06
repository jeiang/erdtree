//! Argument relations (`requires`, mutual exclusion) are validated once, on the merged
//! command-line and config arguments.

use std::{
    fs,
    process::{Command, Output},
};
use tempfile::TempDir;

/// Runs `erd` with `config` as `.erdtree.toml` and `args` appended.
fn run_with_toml(config: &str, args: &[&str]) -> Output {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join(".erdtree.toml");
    fs::write(&path, config).unwrap();

    Command::new(env!("CARGO_BIN_EXE_erd"))
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path().join("xdg"))
        .env("APPDATA", dir.path().join("appdata"))
        .env("ERDTREE_TOML_PATH", &path)
        .args(["--threads", "1", "--color", "none"])
        .args(args)
        .output()
        .unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn config_glob_with_cli_pattern() {
    let out = run_with_toml("glob = true\n", &["--pattern", "*.txt", "tests/data"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("lipsum.txt"));
    assert!(!stdout(&out).contains("cassildas_song.md"));
}

#[test]
fn config_glob_with_config_pattern() {
    let out = run_with_toml("glob = true\npattern = \"*.txt\"\n", &["tests/data"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("lipsum.txt"));
}

#[test]
fn cli_glob_with_config_pattern() {
    let out = run_with_toml("pattern = \"*.txt\"\n", &["--glob", "tests/data"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!stdout(&out).contains("cassildas_song.md"));
}

#[test]
fn config_file_type_with_cli_pattern() {
    let out = run_with_toml(
        "file-type = \"file\"\n",
        &["--pattern", "lipsum", "tests/data"],
    );
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn config_glob_without_pattern_errors() {
    let out = run_with_toml("glob = true\n", &["tests/data"]);
    assert_eq!(out.status.code(), Some(2));
    let err = stderr(&out);
    assert!(
        err.contains("required arguments were not provided"),
        "{err}"
    );
    assert!(err.contains("--pattern"), "{err}");
    assert!(err.contains("Usage: erd"), "{err}");
}

#[test]
fn config_conflicting_glob_and_iglob_errors() {
    let out = run_with_toml(
        "glob = true\niglob = true\npattern = \"x\"\n",
        &["tests/data"],
    );
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("cannot be used with"),
        "{}",
        stderr(&out)
    );
}

#[cfg(unix)]
#[test]
fn config_octal_with_cli_long() {
    let out = run_with_toml("octal = true\n", &["--long", "tests/data"]);
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn config_no_git_with_cli_hidden() {
    let out = run_with_toml("no-git = true\n", &["--hidden", "tests/data"]);
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn config_invalid_value_is_a_config_error() {
    let out = run_with_toml("sort = \"bogus\"\n", &["tests/data"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("A configuration file was found but failed to parse"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn named_table_glob_with_cli_pattern() {
    let out = run_with_toml(
        "[g]\nglob = true\n",
        &["-c", "g", "--pattern", "*.txt", "tests/data"],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!stdout(&out).contains("cassildas_song.md"));
}

#[test]
fn cli_only_missing_pattern_still_errors() {
    let out = Command::new(env!("CARGO_BIN_EXE_erd"))
        .arg("--no-config")
        .args(["--glob", "tests/data"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.starts_with("error: "), "{err}");
    assert!(err.contains("--pattern"), "{err}");
    assert!(err.contains("Usage: erd"), "{err}");
}

#[test]
fn config_exclude_array() {
    let out = run_with_toml("exclude = [\"lipsum\", \"dream_cycle\"]\n", &["tests/data"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let out = stdout(&out);
    assert!(
        !out.contains("lipsum") && !out.contains("dream_cycle"),
        "{out}"
    );
    assert!(out.contains("the_yellow_king"), "{out}");
}

#[test]
fn cli_exclude_overrides_config_exclude() {
    let out = run_with_toml(
        "exclude = [\"lipsum\", \"dream_cycle\"]\n",
        &["--exclude", "the_yellow_king", "tests/data"],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let out = stdout(&out);
    assert!(
        out.contains("lipsum") && out.contains("dream_cycle"),
        "{out}"
    );
    assert!(!out.contains("the_yellow_king"), "{out}");
}

#[test]
fn config_exclude_non_string_element_errors() {
    let out = run_with_toml("exclude = [1]\n", &["tests/data"]);
    assert!(!out.status.success());
}

#[test]
fn config_value_applies_and_cli_value_wins() {
    let level1 = stdout(&run_with_toml("", &["--level", "1", "tests/data"]));
    let level2 = stdout(&run_with_toml("", &["--level", "2", "tests/data"]));
    assert_ne!(level1, level2);

    let from_config = run_with_toml("level = 1\n", &["tests/data"]);
    assert_eq!(stdout(&from_config), level1);

    let overridden = run_with_toml("level = 1\n", &["--level", "2", "tests/data"]);
    assert_eq!(stdout(&overridden), level2);
}

#[test]
fn usage_errors_and_help_are_unchanged_with_and_without_config() {
    for config in [None, Some("level = 1\n")] {
        let run = |args: &[&str]| match config {
            Some(config) => run_with_toml(config, args),
            None => Command::new(env!("CARGO_BIN_EXE_erd"))
                .arg("--no-config")
                .args(args)
                .output()
                .unwrap(),
        };

        let out = run(&["--bogus"]);
        assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
        assert!(
            stderr(&out).contains("unexpected argument"),
            "{}",
            stderr(&out)
        );

        let out = run(&["--help"]);
        assert!(out.status.success(), "{}", stderr(&out));
        assert!(stdout(&out).contains("Usage: erd"), "{}", stdout(&out));

        let out = run(&["--version"]);
        assert!(out.status.success(), "{}", stderr(&out));
        assert!(stdout(&out).starts_with("erdtree "), "{}", stdout(&out));
    }
}

//! `.erdtree.toml` discovery: a missing config is not an error, and the documented lookup order
//! is honored.

use std::{
    path::Path,
    process::{Command, Output},
};
use tempfile::TempDir;

/// Runs `erd tests/data` with `HOME`/`XDG_CONFIG_HOME` pointed at `home` and `home/xdg`.
fn run_in_home(home: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_erd"))
        .env_remove("ERDTREE_TOML_PATH")
        .env_remove("ERDTREE_CONFIG_PATH")
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("xdg"))
        .args(["--threads", "1", "--color", "none", "tests/data"])
        .output()
        .unwrap()
}

#[cfg(unix)]
fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

#[test]
fn no_config_file_is_not_an_error() {
    let home = TempDir::new().unwrap();
    let out = run_in_home(home.path());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("nemesis.txt"));
}

#[cfg(unix)]
#[test]
fn home_dotfile_is_found() {
    let home = TempDir::new().unwrap();
    write(&home.path().join(".erdtree.toml"), "dirs-only = true\n");

    let out = run_in_home(home.path());
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!String::from_utf8_lossy(&out.stdout).contains("nemesis.txt"));
}

#[cfg(unix)]
#[test]
fn xdg_takes_precedence_over_home() {
    let home = TempDir::new().unwrap();
    write(
        &home.path().join("xdg/erdtree/.erdtree.toml"),
        "dirs-only = true\n",
    );
    // Would fail to parse if it were read.
    write(&home.path().join(".erdtree.toml"), "sort = \"bogus\"\n");

    let out = run_in_home(home.path());
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!String::from_utf8_lossy(&out.stdout).contains("nemesis.txt"));
}

#[cfg(unix)]
#[test]
fn malformed_config_is_an_error() {
    let home = TempDir::new().unwrap();
    write(&home.path().join(".erdtree.toml"), "dirs-only = [[\n");

    let out = run_in_home(home.path());
    assert!(!out.status.success());
}

#[cfg(unix)]
#[test]
fn erdtree_dir_accepts_undotted_names_in_order() {
    let home = TempDir::new().unwrap();
    let dir = home.path().join(".config/erdtree");
    write(&dir.join("config.toml"), "sort = \"bogus\"\n");
    write(&dir.join("erdtree.toml"), "dirs-only = true\n");

    // `erdtree.toml` wins over `config.toml`, which would fail to parse if it were read.
    let out = run_in_home(home.path());
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!String::from_utf8_lossy(&out.stdout).contains("nemesis.txt"));

    // `config.toml` alone is found.
    std::fs::remove_file(dir.join("erdtree.toml")).unwrap();
    write(&dir.join("config.toml"), "dirs-only = true\n");
    let out = run_in_home(home.path());
    assert!(out.status.success());
    assert!(!String::from_utf8_lossy(&out.stdout).contains("nemesis.txt"));
}

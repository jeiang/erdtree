//! `.erdtree.toml` discovery: a missing config is not an error, and the documented lookup order
//! is honored.

use std::{fs, path::Path};
use utils::{Erd, Run};

mod utils;

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// Runs `erd tests/data` with the config files `files` (relative to the isolated home).
fn discover(files: &[(&str, &str)]) -> Run {
    let erd = Erd::new().discover_config().arg("tests/data");
    for (path, contents) in files {
        write(&erd.home().join(path), contents);
    }
    erd.run()
}

/// The output of `dirs-only = true`: directories only, and no file in the summary.
#[cfg(unix)]
#[track_caller]
fn assert_dirs_only(run: &Run) {
    assert!(run.status.success(), "{}", run.stderr());
    let out = run.trimmed();
    assert!(out.contains("─ lipsum\n"), "{out}");
    assert!(!out.contains("nemesis.txt"), "{out}");
    assert!(out.ends_with("3 directories"), "{out}");
}

#[test]
fn no_config_file_is_not_an_error() {
    let run = discover(&[]);
    assert!(run.status.success(), "{}", run.stderr());
    assert!(run.stdout().contains("nemesis.txt"));
}

#[cfg(unix)]
#[test]
fn home_dotfile_is_found() {
    assert_dirs_only(&discover(&[(".erdtree.toml", "dirs-only = true\n")]));
}

#[cfg(unix)]
#[test]
fn xdg_takes_precedence_over_home() {
    assert_dirs_only(&discover(&[
        ("xdg/erdtree/.erdtree.toml", "dirs-only = true\n"),
        // Would fail to parse if it were read.
        (".erdtree.toml", "sort = \"bogus\"\n"),
    ]));
}

#[cfg(unix)]
#[test]
fn malformed_config_is_an_error() {
    let run = discover(&[(".erdtree.toml", "dirs-only = [[\n")]);
    assert_eq!(run.code(), Some(1));
    let err = run.stderr();
    assert!(
        err.starts_with("erd: The configuration file is improperly formatted"),
        "{err}"
    );
    assert!(err.contains(".erdtree.toml"), "{err}");
}

#[cfg(unix)]
#[test]
fn erdtree_dir_accepts_undotted_names_in_order() {
    // `erdtree.toml` wins over `config.toml`, which would fail to parse if it were read.
    assert_dirs_only(&discover(&[
        (".config/erdtree/config.toml", "sort = \"bogus\"\n"),
        (".config/erdtree/erdtree.toml", "dirs-only = true\n"),
    ]));

    assert_dirs_only(&discover(&[(
        ".config/erdtree/config.toml",
        "dirs-only = true\n",
    )]));
}

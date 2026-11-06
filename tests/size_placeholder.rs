#![cfg(unix)]

use std::{fs, process::Command};
use tempfile::TempDir;

mod utils;

/// The rows of `args` over a FIFO `p`, which has no size, and a file `f`, which has one.
fn rows(dir: &TempDir, args: &[&str]) -> (String, String) {
    let mut all = args.to_vec();
    all.push(dir.path().to_str().unwrap());

    let out = utils::run_cmd_untrimmed(&all);
    let row = |name: &str| {
        out.lines()
            .find(|l| l.ends_with(&format!("\u{2500} {name}")))
            .unwrap_or_else(|| panic!("no row {name} in {out}"))
            .to_string()
    };
    (row("p"), row("f"))
}

/// Width of the size column of `row`: everything before the tree prefix.
fn size_width(row: &str) -> usize {
    row.find(['\u{250c}', '\u{251c}', '\u{2514}']).unwrap()
}

#[test]
fn placeholder_is_as_wide_as_sizes() {
    let dir = TempDir::new().unwrap();
    assert!(
        Command::new("mkfifo")
            .arg(dir.path().join("p"))
            .status()
            .unwrap()
            .success()
    );
    fs::write(dir.path().join("f"), "one two\nthree\n").unwrap();

    for args in [
        &["--unit", "bin"][..],
        &["--unit", "si"],
        &["-H", "--unit", "bin"],
        &["-H", "--unit", "si"],
        &["--disk-usage", "line"],
        &["--disk-usage", "word"],
        &["--disk-usage", "block"],
    ] {
        let (placeholder, sized) = rows(&dir, args);
        assert!(
            placeholder.trim_start().starts_with('-'),
            "{args:?}: {placeholder}"
        );
        assert_eq!(
            size_width(&placeholder),
            size_width(&sized),
            "{args:?}:\n{placeholder}\n{sized}"
        );
    }
}

/// Sizes that are all zero still get a size column, so rows without a size show the
/// placeholder in it instead of losing the column.
#[test]
fn placeholder_shows_when_every_size_is_zero() {
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join("sub")).unwrap();
    fs::write(dir.path().join("sub").join("empty"), "").unwrap();
    assert!(
        Command::new("mkfifo")
            .arg(dir.path().join("p"))
            .status()
            .unwrap()
            .success()
    );
    let root = dir.path().file_name().unwrap().to_str().unwrap();

    let cases: [(&[&str], &str); 4] = [
        (&[], "0 B    ┌─ empty\n  - ┌─ sub\n  - ├─ p\n  - ROOT"),
        (
            &["-H"],
            "0   B    ┌─ empty\n    - ┌─ sub\n    - ├─ p\n    - ROOT",
        ),
        (
            &["--disk-usage", "line"],
            "0    ┌─ empty\n- ┌─ sub\n- ├─ p\n- ROOT",
        ),
        (
            &["--layout", "flat"],
            "0 B   sub/empty\n  -   sub\n  -   p\n  -   ROOT",
        ),
    ];

    for (args, expected) in cases {
        let mut all = args.to_vec();
        all.push(dir.path().to_str().unwrap());
        let out = utils::run_cmd(&all);
        let tree = out.split("\n\n").next().unwrap();
        assert_eq!(tree, expected.replace("ROOT", root), "{args:?}");
    }
}

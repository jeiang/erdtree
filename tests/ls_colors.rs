mod utils;

use utils::run_cmd_raw;

fn colored(ls_colors: &str) -> String {
    let out = run_cmd_raw(
        &["--color", "force", "tests/data"],
        &[("LS_COLORS", ls_colors)],
    );
    String::from_utf8(out).unwrap()
}

#[test]
fn file_background_color_is_honored() {
    let out = colored("fi=30;43");
    assert!(
        out.contains("\u{1b}[1;43;30m"),
        "missing background: {out:?}"
    );
}

#[test]
fn file_attributes_are_honored() {
    let out = colored("fi=4;31;42");
    assert!(out.contains("\u{1b}[1;4;42;31m"), "missing style: {out:?}");
}

#[test]
fn file_without_background_is_unchanged() {
    let out = colored("fi=31");
    assert!(
        out.contains("\u{1b}[1;31mnemesis.txt"),
        "unexpected: {out:?}"
    );
    assert!(
        !out.contains("\u{1b}[1;4"),
        "unexpected background: {out:?}"
    );
}

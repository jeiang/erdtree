//! `NO_COLOR` (<https://no-color.org>): only a non-empty value disables colour, and
//! `--color force` overrides it.

mod utils;

fn has_escapes(color: &str, no_color: &str) -> bool {
    utils::Erd::new()
        .args(["--color", color, "tests/data"])
        .env("NO_COLOR", no_color)
        .ok()
        .raw()
        .contains('\x1b')
}

#[test]
fn empty_no_color_keeps_color_none_plain() {
    assert!(!has_escapes("none", ""));
}

#[test]
fn empty_no_color_is_ignored() {
    assert!(has_escapes("force", ""));
}

#[test]
fn force_wins_over_non_empty_no_color() {
    assert!(has_escapes("force", "1"));
}

#[test]
fn non_empty_no_color_keeps_color_none_plain() {
    assert!(!has_escapes("none", "1"));
}

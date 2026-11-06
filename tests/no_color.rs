//! `NO_COLOR` (<https://no-color.org>): only a non-empty value disables colour.

mod utils;

fn has_escapes(color: &str, no_color: &str) -> bool {
    utils::run_cmd_raw(&["--color", color, "tests/data"], &[("NO_COLOR", no_color)]).contains(&0x1b)
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
fn non_empty_no_color_wins_over_force() {
    assert!(!has_escapes("force", "1"));
}

mod utils;

/// The SGR parameters of the escape right before `nemesis.txt` with `LS_COLORS=ls_colors`,
/// sorted.
fn nemesis_style(ls_colors: &str) -> Vec<u8> {
    let out = utils::Erd::new()
        .args(["--color", "force", "tests/data"])
        .env("LS_COLORS", ls_colors)
        .ok()
        .raw();
    let before = &out[..out.find("nemesis.txt").expect("nemesis.txt")];
    let params = before
        .strip_suffix('m')
        .and_then(|s| s.rsplit_once("\x1b["))
        .unwrap_or_else(|| panic!("no style before nemesis.txt: {out:?}"))
        .1;
    let mut params: Vec<u8> = params.split(';').map(|p| p.parse().unwrap()).collect();
    params.sort_unstable();
    params
}

fn is_background(param: &u8) -> bool {
    (40..=47).contains(param)
}

#[test]
fn file_background_color_is_honored() {
    let style = nemesis_style("fi=30;43");
    assert!(style.contains(&30) && style.contains(&43), "{style:?}");
}

#[test]
fn file_attributes_are_honored() {
    let style = nemesis_style("fi=4;31;42");
    for param in [4, 31, 42] {
        assert!(style.contains(&param), "{style:?}");
    }
}

#[test]
fn file_without_background_is_unchanged() {
    let style = nemesis_style("fi=31");
    assert!(style.contains(&31), "{style:?}");
    assert!(!style.iter().any(is_background), "{style:?}");
}

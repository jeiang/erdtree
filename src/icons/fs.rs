use std::{ffi::OsStr, path::Path};

/// How a colored icon is painted.
#[derive(Debug, PartialEq, Eq)]
pub enum Paint {
    /// With the bold form of the style of the file, if that style is not plain.
    FileStyle,

    /// With this 8-bit color.
    Fixed(u8),
}

/// Looks up the icon glyph and how to paint it.
///
/// The precedent from highest to lowest in terms of which parameters determine the icon used
/// is as followed: file-type, file-extension, and then file-name. If an icon cannot be
/// computed the fall-back default icon is used. Without color the [`Paint`] does not apply.
///
/// `target` is the path of a symlink's target, whose extension is used for a symlink; otherwise the
/// extension of the entry's name is.
pub fn lookup(
    is_dir: bool,
    is_symlink: bool,
    target: Option<&Path>,
    file_name: &OsStr,
) -> (&'static str, Paint) {
    if let Some(icon) = super::icon_from_file_type(is_dir, is_symlink) {
        return (icon, Paint::FileStyle);
    }

    let ext = target.map_or_else(|| name_extension(file_name), Path::extension);
    if let Some((code, icon)) = ext.and_then(super::icon_from_ext) {
        return (icon, Paint::Fixed(code));
    }

    if let Some(icon) = super::icon_from_file_name(file_name) {
        return (icon, Paint::FileStyle);
    }

    let (code, icon) = super::DEFAULT_ICON;
    (icon, Paint::Fixed(code))
}

/// The extension of a file name, as [`Path::extension`] finds it for a path of one component:
/// the text after the last dot, unless the dot starts the name or the name is `..`.
pub fn name_extension(name: &OsStr) -> Option<&OsStr> {
    let bytes = name.as_encoded_bytes();
    let dot = bytes
        .iter()
        .rposition(|&b| b == b'.')
        .filter(|&at| at > 0 && bytes != b"..")?;

    // SAFETY: a split at an ASCII dot of an encoded name is valid for `OsStr`.
    Some(unsafe { OsStr::from_encoded_bytes_unchecked(&bytes[dot + 1..]) })
}

/// Appends `icon` painted with the 8-bit color `code`, as `Color::Fixed(code).paint(icon)` shows
/// it.
pub fn push_fixed(out: &mut Vec<u8>, code: u8, icon: &str) {
    out.extend_from_slice(b"\x1b[38;5;");
    crate::render::bytes::push_u64(out, u64::from(code));
    out.push(b'm');
    out.extend_from_slice(icon.as_bytes());
    out.extend_from_slice(b"\x1b[0m");
}

#[cfg(test)]
mod test {
    use super::*;
    use nu_ansi_term::Color;

    #[test]
    fn fixed_paint_matches_nu_ansi_term() {
        for code in [0, 5, 66, 99, 100, 255] {
            let mut out = vec![];
            push_fixed(&mut out, code, "\u{e7a8}");
            assert_eq!(
                String::from_utf8(out).unwrap(),
                Color::Fixed(code).paint("\u{e7a8}").to_string()
            );
        }
    }

    #[test]
    fn extensions_match_path() {
        for name in [
            "",
            ".",
            "..",
            "...",
            "a",
            ".a",
            "..a",
            "a.",
            "a..",
            "a.b",
            ".a.b",
            "a.b.c",
            "a.rs",
            "Cargo.lock",
            ".bashrc",
            "x.\u{e9}",
            "\u{e9}.rs",
            "a b.txt",
        ] {
            assert_eq!(
                name_extension(OsStr::new(name)),
                Path::new(name).extension(),
                "{name:?}"
            );
        }
    }

    #[test]
    fn precedence() {
        let name = OsStr::new("Cargo.lock");

        assert_eq!(
            lookup(true, false, Some(Path::new("x.rs")), name).1,
            Paint::FileStyle
        );
        assert_eq!(
            lookup(false, false, Some(Path::new("x.rs")), name),
            ("\u{e7a8}", Paint::Fixed(180))
        );
        assert_eq!(
            lookup(false, false, None, name),
            ("\u{f13e}", Paint::Fixed(250))
        );
        assert_eq!(
            lookup(false, false, None, OsStr::new("Dockerfile")),
            ("\u{f308}", Paint::FileStyle)
        );
        assert_eq!(
            lookup(false, false, None, OsStr::new("nothing")),
            ("\u{f15b}", Paint::Fixed(66))
        );
    }
}

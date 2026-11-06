use super::{DEFAULT_ICON, EXT_ICON_MAP, NAME_ICON_MAP, Rank};
use crate::fs::stat::Kind;
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
/// Directories and symlinks have icons of their own kind; other entries take the icon of their
/// name ranked before the extension, of their extension, of their name ranked after the
/// extension, or the default icon, in that order. Without color the [`Paint`] does not apply.
///
/// `target` is the path of a symlink's target, whose extension is used if given; otherwise the
/// extension of the entry's name is.
pub fn lookup(kind: Kind, target: Option<&Path>, file_name: &OsStr) -> (&'static str, Paint) {
    match kind {
        Kind::Dir => return ("\u{f413}", Paint::FileStyle),
        Kind::Symlink => return ("\u{f482}", Paint::FileStyle),
        _ => {},
    }

    let name = NAME_ICON_MAP.get(file_name).copied();
    if let Some((Rank::BeforeExt(code), icon)) = name {
        return (icon, Paint::Fixed(code));
    }

    let ext = target.map_or_else(|| name_extension(file_name), Path::extension);
    if let Some(&(code, icon)) = ext.and_then(|ext| EXT_ICON_MAP.get(ext)) {
        return (icon, Paint::Fixed(code));
    }

    if let Some((Rank::AfterExt, icon)) = name {
        return (icon, Paint::FileStyle);
    }

    let (code, icon) = DEFAULT_ICON;
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

#[cfg(test)]
mod test {
    use super::*;

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
        let rs = Some(Path::new("x.rs"));

        assert_eq!(
            lookup(Kind::Dir, rs, OsStr::new("x.rs")),
            ("\u{f413}", Paint::FileStyle)
        );
        assert_eq!(
            lookup(Kind::Symlink, rs, OsStr::new("x.rs")),
            ("\u{f482}", Paint::FileStyle)
        );
        // The extension of the target wins over a name ranked after it.
        assert_eq!(
            lookup(Kind::File, rs, OsStr::new("Dockerfile")),
            ("\u{e7a8}", Paint::Fixed(180))
        );
        assert_eq!(
            lookup(Kind::File, None, OsStr::new("Dockerfile")),
            ("\u{f308}", Paint::FileStyle)
        );
        assert_eq!(
            lookup(Kind::File, None, OsStr::new("nothing")),
            ("\u{f15b}", Paint::Fixed(66))
        );
    }

    /// Names ranked before the extension win over it.
    #[test]
    fn names_before_ext() {
        for (name, icon) in [
            ("config.ru", ("\u{e791}", Paint::Fixed(52))),
            ("test.js", ("\u{e60c}", Paint::Fixed(173))),
            ("Makefile", ("\u{e779}", Paint::Fixed(66))),
            ("GNUmakefile", ("\u{e779}", Paint::Fixed(66))),
            ("makefile", ("\u{e779}", Paint::Fixed(66))),
            ("other.js", ("\u{e60c}", Paint::Fixed(185))),
        ] {
            assert_eq!(lookup(Kind::File, None, OsStr::new(name)), icon, "{name}");
        }
    }

    /// A name ranked after the extension whose own extension has an icon could never match.
    #[test]
    fn names_after_ext_are_reachable() {
        for (name, rank, _) in super::super::NAME_ICONS {
            let name = OsStr::new(name);
            if matches!(rank, Rank::AfterExt) {
                assert!(
                    name_extension(name).is_none_or(|ext| !EXT_ICON_MAP.contains_key(ext)),
                    "{name:?}"
                );
            }
        }
    }
}

use nu_ansi_term::{AnsiGenericString, Style};
use std::{borrow::Cow, ffi::OsStr, fs::FileType, path::Path};

/// Computes a plain, colorless icon with given parameters.
///
/// The precedent from highest to lowest in terms of which parameters determine the icon used
/// is as followed: file-type, file-extension, and then file-name. If an icon cannot be
/// computed the fall-back default icon is used.
///
/// `ext_source` is the path whose extension is used: the symlink target for a symlink, else the
/// entry's name.
pub fn compute(file_type: FileType, ext_source: &Path, file_name: &OsStr) -> Cow<'static, str> {
    if let Some(icon) = super::icon_from_file_type(file_type) {
        return Cow::from(icon);
    }

    if let Some((_, icon)) = ext_source.extension().and_then(super::icon_from_ext) {
        return Cow::from(icon);
    }

    if let Some(icon) = super::icon_from_file_name(file_name) {
        return Cow::from(icon);
    }

    Cow::from(super::get_default_icon().1)
}

/// Computes a plain, colored icon with given parameters. See [compute] for more details.
pub fn compute_with_color(
    file_type: FileType,
    ext_source: &Path,
    file_name: &OsStr,
    style: Option<Style>,
) -> Cow<'static, str> {
    let paint_icon = |icon| match style {
        Some(style) if !style.is_plain() => {
            let ansi_string: AnsiGenericString<str> = style.bold().paint(icon);
            let styled_icon = ansi_string.to_string();
            Cow::from(styled_icon)
        },
        _ => icon,
    };

    if let Some(icon) = super::icon_from_file_type(file_type) {
        return paint_icon(Cow::from(icon));
    }

    if let Some(attrs) = ext_source.extension().and_then(super::icon_from_ext) {
        return Cow::from(super::col(attrs.0, attrs.1));
    }

    if let Some(icon) = super::icon_from_file_name(file_name) {
        return paint_icon(Cow::from(icon));
    }

    let (code, icon) = super::get_default_icon();
    Cow::from(super::col(code, icon))
}

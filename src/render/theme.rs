use crate::{
    styles::{self, ThemesMap},
    tree::node::Node,
};
use nu_ansi_term::Color;
use std::fmt;

type Theme = Box<dyn FnMut(Node<'_>) -> &'static ThemesMap>;

/// Box-drawing pieces of a theme, looked up once instead of per row.
pub struct Glyphs {
    pub vt: &'static str,
    pub vtrt: &'static str,
    pub uprt: &'static str,
    pub drt: &'static str,
}

impl Glyphs {
    fn of(theme: &'static ThemesMap) -> Self {
        let get = |key| theme.get(key).map_or("", String::as_str);
        Self {
            vt: get("vt"),
            vtrt: get("vtrt"),
            uprt: get("uprt"),
            drt: get("drt"),
        }
    }
}

/// [`Glyphs`] of the regular and the link theme.
pub struct GlyphCache {
    tree: (&'static ThemesMap, Glyphs),
    link: Option<(&'static ThemesMap, Glyphs)>,
}

impl GlyphCache {
    pub fn new() -> Self {
        let tree = styles::get_tree_theme().unwrap();
        let link = styles::get_link_theme().ok();
        Self {
            tree: (tree, Glyphs::of(tree)),
            link: link.map(|link| (link, Glyphs::of(link))),
        }
    }

    /// Glyphs of `theme`, which must come from a theme getter of this module.
    pub fn get(&self, theme: &'static ThemesMap) -> &Glyphs {
        match &self.link {
            Some((link, glyphs)) if std::ptr::eq(theme, *link) => glyphs,
            _ => &self.tree.1,
        }
    }
}

/// Returns a closure that retrieves the regular theme.
pub fn regular_theme_getter() -> Theme {
    Box::new(|_node| styles::get_tree_theme().unwrap())
}

/// Returns a closure that can smartly determine when a symlink is being followed and when it is
/// not being followed. When a symlink is being followed, all of its descendents should have tree
/// branches that are colored differently.
pub fn link_theme_getter() -> Theme {
    let mut link_depth = None;

    Box::new(move |node| {
        let current_depth = node.depth();

        if let Some(ldepth) = link_depth
            && current_depth == ldepth
        {
            link_depth = None;
        }

        if link_depth.is_some() || node.is_symlink() {
            if node.is_dir() && link_depth.is_none() {
                link_depth = Some(current_depth);
            }
            styles::get_link_theme().unwrap()
        } else {
            styles::get_tree_theme().unwrap()
        }
    })
}

/// Writes the file name of `node` with its style. A symlink is followed by an arrow and its
/// target. With `classify`, the [`Node::indicator`] follows the name.
pub fn write_file_name(f: &mut fmt::Formatter<'_>, node: Node<'_>, classify: bool) -> fmt::Result {
    let name = node.file_name().to_string_lossy();
    let style = node.style();
    let ind = if classify {
        node.indicator().unwrap_or_default()
    } else {
        ""
    };

    let Some(target_name) = node.symlink_target_file_name() else {
        if let Some(style) = style.filter(|style| !style.is_plain()) {
            let bold = style.bold();
            return write!(f, "{}{name}{}{ind}", bold.prefix(), bold.suffix());
        }

        return write!(f, "{name}{ind}");
    };

    let target = target_name.to_string_lossy();

    if let Some(color) = style {
        let red = Color::Red.normal();
        return write!(
            f,
            "{}{name}{}{ind} {}\u{2192} {target}{}",
            color.prefix(),
            color.suffix(),
            red.prefix(),
            red.suffix()
        );
    }

    write!(f, "{name}{ind} \u{2192} {target}")
}

/// Styles the symbolic notation of file permissions.
#[cfg(unix)]
pub fn style_sym_permissions(node: Node<'_>) -> String {
    use crate::fs::permissions::FileModeXAttrs;

    let perms = node.mode().expect("Expected permissions to be initialized");

    let symb = if node.has_xattrs() {
        let perm_xattr = FileModeXAttrs(&perms);
        format!("{perm_xattr}")
    } else {
        // extra whitespace to align with permissions with extended attrs
        format!("{perms} ")
    };

    if let Ok(theme) = styles::get_permissions_theme() {
        symb.chars()
            .filter_map(|ch| {
                theme.get(&ch).map(|color| {
                    let chstr = ch.to_string();
                    color.paint(chstr).to_string()
                })
            })
            .collect()
    } else {
        symb
    }
}

/// Styles the octal notation of file permissions.
#[cfg(unix)]
pub fn style_oct_permissions(node: Node<'_>) -> String {
    let perms = node.mode().expect("Expected permissions to be initialized");
    let oct = format!("{perms:04o}");

    if let Ok(style) = styles::get_octal_permissions_style() {
        style.paint(oct).to_string()
    } else {
        oct
    }
}

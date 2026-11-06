use crate::{
    styles::{self, ThemesMap},
    tree::node::Node,
};
use nu_ansi_term::Style;

#[cfg(unix)]
use crate::fs::permissions::FileMode;

/// Tracks whether a node lies below a followed symlink, whose branches use the link theme.
pub struct LinkTracker {
    depth: Option<usize>,
}

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
    tree: Glyphs,
    link: Glyphs,
}

impl GlyphCache {
    pub fn new() -> Self {
        Self {
            tree: Glyphs::of(styles::get_tree_theme().unwrap()),
            link: Glyphs::of(styles::get_link_theme().unwrap()),
        }
    }

    /// The pieces of the prefixes of the regular theme; `inverted` selects the glyph of a last
    /// sibling of the layout with the root on top.
    pub fn prefix_glyphs(&self, inverted: bool) -> super::prefix::Glyphs {
        let glyphs = &self.tree;
        let bytes = |s: &str| s.as_bytes().into();

        super::prefix::Glyphs {
            vt: bytes(glyphs.vt),
            vtrt: bytes(glyphs.vtrt),
            last: bytes(if inverted { glyphs.uprt } else { glyphs.drt }),
        }
    }

    /// Glyphs of the link theme if `link`, else of the regular theme.
    pub const fn get(&self, link: bool) -> &Glyphs {
        if link { &self.link } else { &self.tree }
    }
}

impl LinkTracker {
    pub const fn new() -> Self {
        Self { depth: None }
    }

    /// Whether `node` gets the link theme: it is a symlink or lies below a followed symlinked
    /// directory. Nodes must arrive in render order.
    pub fn is_link(&mut self, node: Node<'_>) -> bool {
        let current_depth = node.depth();

        if let Some(ldepth) = self.depth
            && current_depth == ldepth
        {
            self.depth = None;
        }

        if self.depth.is_some() || node.is_symlink() {
            if node.is_dir() && self.depth.is_none() {
                self.depth = Some(current_depth);
            }
            true
        } else {
            false
        }
    }
}

/// The escape sequences that open and close `style`, built once so that painting a string is
/// two copies around it.
pub struct Esc {
    pub pre: Box<[u8]>,
    pub suf: Box<[u8]>,
}

impl Esc {
    /// The sequences of `style`; both are empty for a plain style.
    pub fn new(style: Style) -> Self {
        Self {
            pre: style.prefix().to_string().into_bytes().into(),
            suf: style.suffix().to_string().into_bytes().into(),
        }
    }
}

/// An [`Esc`] for each style of a file, as is and in bold.
pub struct Painter {
    pub plain: Esc,
    pub bold: Esc,
}

/// The [`Painter`]s of the file styles seen so far. `LS_COLORS` yields few distinct styles, so a
/// linear search that starts at the last hit beats hashing.
#[derive(Default)]
pub struct StyleCache {
    seen: Vec<(Style, Painter)>,
    last: usize,
}

/// Distinct styles kept; later ones are rebuilt per use, which is slow but correct.
const STYLE_CACHE_CAP: usize = 256;

impl StyleCache {
    /// The painter of `style`.
    pub fn get(&mut self, style: Style) -> &Painter {
        let hit = if self.seen.get(self.last).is_some_and(|(s, _)| *s == style) {
            Some(self.last)
        } else {
            self.seen.iter().position(|(s, _)| *s == style)
        };

        let at = hit.unwrap_or_else(|| {
            let painter = Painter {
                plain: Esc::new(style),
                bold: Esc::new(style.bold()),
            };

            if self.seen.len() >= STYLE_CACHE_CAP {
                self.seen.truncate(STYLE_CACHE_CAP - 1);
            }
            self.seen.push((style, painter));
            self.seen.len() - 1
        });

        self.last = at;
        &self.seen[at].1
    }
}

/// Styles the symbolic notation of file permissions.
#[cfg(unix)]
pub fn style_sym_permissions(perms: &FileMode, has_xattrs: bool) -> String {
    use crate::fs::permissions::FileModeXAttrs;

    let symb = if has_xattrs {
        let perm_xattr = FileModeXAttrs(perms);
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
pub fn style_oct_permissions(perms: &FileMode) -> String {
    let oct = format!("{perms:04o}");

    if let Ok(style) = styles::get_octal_permissions_style() {
        style.paint(oct).to_string()
    } else {
        oct
    }
}

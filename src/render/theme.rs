use crate::{styles, tree::node::Node};
use nu_ansi_term::Style;

/// Tracks whether a node lies below a followed symlink, whose branches use the link theme.
pub struct LinkTracker {
    depth: Option<usize>,
}

/// The pieces of the prefixes of the tree theme; `inverted` selects the glyph of a last sibling
/// of the layout with the root on top.
pub fn prefix_glyphs(inverted: bool) -> super::prefix::Glyphs {
    let glyphs = &styles::theme().tree;
    let bytes = |s: &str| s.as_bytes().into();

    super::prefix::Glyphs {
        vt: bytes(&glyphs.vt),
        vtrt: bytes(&glyphs.vtrt),
        last: bytes(if inverted { &glyphs.uprt } else { &glyphs.drt }),
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
pub fn style_sym_permissions(mode: u32, has_xattrs: bool) -> String {
    // A trailing space aligns entries without extended attributes with those with them.
    let symb: String = crate::fs::permissions::symbolic(mode)
        .iter()
        .map(|&b| char::from(b))
        .chain([if has_xattrs { '@' } else { ' ' }])
        .collect();

    if styles::colors().is_some() {
        symb.chars()
            .filter_map(|ch| {
                styles::Long::permission(ch).map(|color| color.paint(ch.to_string()).to_string())
            })
            .collect()
    } else {
        symb
    }
}

/// Styles the octal notation of file permissions.
#[cfg(unix)]
pub fn style_oct_permissions(mode: u32) -> String {
    let oct = crate::fs::permissions::octal(mode);

    if let Some(colors) = styles::colors() {
        colors.long.octal.paint(oct).to_string()
    } else {
        oct
    }
}

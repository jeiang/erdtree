//! Box-drawing prefixes of tree rows, computed from a row's ancestors alone so that any worker
//! can start at any row.

use crate::{styles, tree::Tree};

/// The themes of the prefixes: the tree theme, and with `--follow` the link theme of symlinks
/// and of everything below a symlink.
pub struct Themes {
    tree: &'static styles::Glyphs,
    link: Option<&'static styles::Glyphs>,
}

impl Themes {
    /// The glyphs of the current theme; `follow` adds the link theme.
    pub fn new(follow: bool) -> Self {
        let theme = styles::theme();

        Self {
            tree: &theme.tree,
            link: follow.then_some(&theme.link),
        }
    }

    /// The set of the link theme if `link`, else of the tree theme.
    const fn get(&self, link: bool) -> &styles::Glyphs {
        match self.link {
            Some(set) if link => set,
            _ => self.tree,
        }
    }

    /// Whether node `id` of `tree` is in the link theme, given whether its parent is: it is a
    /// symlink or lies below one. Always `false` without the link theme.
    fn inside(&self, tree: &Tree, id: u32, parent_inside: bool) -> bool {
        self.link.is_some() && (parent_inside || tree.node(id).is_symlink())
    }
}

/// Prefixes of consecutive rows share most of their bytes, so the cursor keeps the prefix of
/// the ancestors of the last row and only changes its tail. A row's prefix is, for each ancestor
/// below the root, outermost first, blank if the ancestor is the last of its siblings and a
/// vertical line otherwise, then the glyph of the row itself; each piece is in the link theme
/// if its node is a symlink or lies below one (the root never is). A fresh or stale cursor is
/// always correct: ancestors are compared by identity.
#[derive(Default)]
pub struct Cursor {
    /// `chain[k]` is the ancestor at depth `k + 1`.
    chain: Vec<u32>,

    /// `inside[k]` is whether `chain[k]` is in the link theme.
    inside: Vec<bool>,

    /// `ends[k]` is the end of the segment of `chain[k]` in `bytes`.
    ends: Vec<usize>,
    bytes: Vec<u8>,
    fresh: Vec<u32>,
}

impl Cursor {
    /// Appends the prefix of node `id`, which is at `depth`, to `out`; `inverted` puts the root
    /// on top. The root has none.
    pub fn write(
        &mut self,
        tree: &Tree,
        id: u32,
        depth: usize,
        glyphs: &Themes,
        inverted: bool,
        out: &mut Vec<u8>,
    ) {
        if depth == 0 {
            return;
        }

        // Ancestors of `id` at depth `depth - 1` down to the first one the cursor holds.
        self.fresh.clear();
        let mut cur = tree.parent(id);
        let mut cur_depth = depth - 1;

        while cur_depth >= 1 {
            if self.chain.get(cur_depth - 1) == Some(&cur) {
                break;
            }
            self.fresh.push(cur);
            cur = tree.parent(cur);
            cur_depth -= 1;
        }

        self.chain.truncate(cur_depth);
        self.inside.truncate(cur_depth);
        self.ends.truncate(cur_depth);
        self.bytes
            .truncate(cur_depth.checked_sub(1).map_or(0, |k| self.ends[k]));

        let mut inside = self.inside.last().copied().unwrap_or(false);

        for &ancestor in self.fresh.iter().rev() {
            inside = glyphs.inside(tree, ancestor, inside);
            let segment = if tree.is_last(ancestor) {
                styles::SEP.as_bytes()
            } else {
                &glyphs.get(inside).vt
            };
            self.chain.push(ancestor);
            self.inside.push(inside);
            self.bytes.extend_from_slice(segment);
            self.ends.push(self.bytes.len());
        }

        // Deeper leftovers of an earlier row are not part of this prefix.
        let parent = depth - 1;
        let upto = parent.checked_sub(1).map_or(0, |k| self.ends[k]);
        let parent_inside = parent.checked_sub(1).is_some_and(|k| self.inside[k]);
        let set = glyphs.get(glyphs.inside(tree, id, parent_inside));

        out.extend_from_slice(&self.bytes[..upto]);
        out.extend_from_slice(if !tree.is_last(id) {
            &set.vtrt
        } else if inverted {
            &set.uprt
        } else {
            &set.drt
        });
    }
}

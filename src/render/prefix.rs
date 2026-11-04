//! Box-drawing prefixes of tree rows, computed from a row's ancestors alone so that any worker
//! can start at any row.

use crate::{styles, tree::Tree};

/// The pieces a prefix is made of.
pub struct Glyphs {
    /// Continues the line of an ancestor that has later siblings.
    pub vt: Box<[u8]>,

    /// Own glyph of a node that has later siblings.
    pub vtrt: Box<[u8]>,

    /// Own glyph of the last sibling.
    pub last: Box<[u8]>,
}

/// Prefixes of consecutive rows share most of their bytes, so the cursor keeps the prefix of
/// the ancestors of the last row and only changes its tail. A row's prefix is, for each ancestor
/// below the root, outermost first, blank if the ancestor is the last of its siblings and a
/// vertical line otherwise, then the glyph of the row itself. A fresh or stale cursor is always
/// correct: ancestors are compared by identity.
#[derive(Default)]
pub struct Cursor {
    /// `chain[k]` is the ancestor at depth `k + 1`.
    chain: Vec<u32>,

    /// `ends[k]` is the end of the segment of `chain[k]` in `bytes`.
    ends: Vec<usize>,
    bytes: Vec<u8>,
    fresh: Vec<u32>,
}

impl Cursor {
    /// Appends the prefix of node `id`, which is at `depth`, to `out`. The root has none.
    pub fn write(
        &mut self,
        tree: &Tree,
        id: u32,
        depth: usize,
        glyphs: &Glyphs,
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
        self.ends.truncate(cur_depth);
        self.bytes
            .truncate(cur_depth.checked_sub(1).map_or(0, |k| self.ends[k]));

        for &ancestor in self.fresh.iter().rev() {
            let segment = if tree.is_last(ancestor) {
                styles::SEP.as_bytes()
            } else {
                &glyphs.vt
            };
            self.chain.push(ancestor);
            self.bytes.extend_from_slice(segment);
            self.ends.push(self.bytes.len());
        }

        // Deeper leftovers of an earlier row are not part of this prefix.
        let upto = depth.checked_sub(2).map_or(0, |k| self.ends[k]);
        out.extend_from_slice(&self.bytes[..upto]);
        out.extend_from_slice(if tree.is_last(id) {
            &glyphs.last
        } else {
            &glyphs.vtrt
        });
    }
}

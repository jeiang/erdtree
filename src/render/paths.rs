//! Paths of directories for the flat layouts, built once so that a row is a copy of its parent's
//! path and its own name.

use super::bytes::push_lossy;
use crate::tree::{Tree, topo::ix};
use std::path::MAIN_SEPARATOR;

/// For every shown directory, its path below the root followed by a separator, converted
/// lossily. Converting a path component by component gives the same bytes as converting the
/// whole path, because the separator is ASCII and never part of a multi-byte sequence.
pub struct DirPaths {
    arena: Vec<u8>,

    /// Start and end in `arena` of the prefix of each node; zero (empty) for the root and for
    /// nodes that are not directories.
    spans: Vec<usize>,
}

impl DirPaths {
    /// The prefixes of the directories of `tree` down to `level`, found with `threads` threads.
    pub fn new(tree: &Tree, level: usize, threads: usize) -> Self {
        // Entries per thread below which a thread costs more than it saves.
        const MIN_SHARE: usize = 1 << 16;

        // The shown directories in pre-order, so parents come before their children, and the
        // number of nodes. Most entries are not directories, and finding the others is a scan of
        // scattered columns.
        let scan = |ids: &[u32]| {
            let nodes = ids.iter().map(|&id| ix(id) + 1).max().unwrap_or(0);
            let dirs: Vec<u32> = ids
                .iter()
                .copied()
                .filter(|&id| {
                    let node = tree.node(id);
                    let kind = node.file_type();
                    (kind.is_dir() || kind.is_symlink()) && node.depth() < level
                })
                .collect();
            (nodes, dirs)
        };

        let order = tree.order();
        let share = order
            .len()
            .div_ceil(threads.clamp(1, order.len() / MIN_SHARE + 1));
        let parts: Vec<(usize, Vec<u32>)> = std::thread::scope(|scope| {
            // Every thread starts before the first is joined.
            #[allow(clippy::needless_collect)]
            let workers: Vec<_> = order
                .chunks(share.max(1))
                .map(|ids| scope.spawn(move || scan(ids)))
                .collect();
            workers
                .into_iter()
                .map(|w| w.join().unwrap_or_else(|e| std::panic::resume_unwind(e)))
                .collect()
        });

        let nodes = parts.iter().map(|part| part.0).max().unwrap_or(0);
        // Zeroed memory costs nothing until touched, and most nodes are not directories.
        let mut spans = vec![0_usize; 2 * nodes];
        let mut arena = Vec::new();

        // The root has the empty prefix.
        for &id in parts
            .iter()
            .flat_map(|part| &part.1)
            .filter(|&&id| id != tree.root())
        {
            let at = 2 * ix(tree.parent(id));
            let (start, end) = (spans[at], spans[at + 1]);
            let begin = arena.len();
            arena.extend_from_within(start..end);
            push_lossy(&mut arena, tree.node(id).file_name());
            arena.push(MAIN_SEPARATOR as u8);
            spans[2 * ix(id)..2 * ix(id) + 2].copy_from_slice(&[begin, arena.len()]);
        }

        Self { arena, spans }
    }

    /// Hints that the span of directory `id` is needed soon.
    #[inline]
    pub fn prefetch_span(&self, id: u32) {
        super::bytes::prefetch(self.spans.get(2 * ix(id)));
    }

    /// Hints that the prefix of directory `id` is needed soon; after [`Self::prefetch_span`].
    #[inline]
    pub fn prefetch_prefix(&self, id: u32) {
        if let Some(&start) = self.spans.get(2 * ix(id)) {
            super::bytes::prefetch(self.arena.get(start));
        }
    }

    /// The prefix of the entries of directory `id`.
    #[inline]
    pub fn prefix(&self, id: u32) -> &[u8] {
        let at = 2 * ix(id);
        &self.arena[self.spans[at]..self.spans[at + 1]]
    }
}

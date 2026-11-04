//! Pure algorithms over the tree topology. Nodes are `u32` indices into parallel arrays; the
//! root's parent is [`NONE`]. Nothing here performs I/O, allocates per node beyond the output
//! arrays, or recurses, so these functions are cheap on trees with millions of nodes and small
//! enough to verify exhaustively for bounded sizes with Kani (see `proofs` below).

use std::cmp::Ordering;

/// Parent of the root.
pub const NONE: u32 = u32::MAX;

/// `usize` from a node index.
#[inline]
pub const fn ix(i: u32) -> usize {
    i as usize
}

/// Children of every node in compressed sparse row form: the children of `p` are
/// `list[start[p]..start[p + 1]]`, in increasing index order until sorted with
/// [`Children::sort_each`].
pub struct Children {
    start: Vec<u32>,
    list: Vec<u32>,
}

impl Children {
    /// Groups nodes by parent (a counting sort). Nodes whose parent is [`NONE`] are roots and
    /// appear in no list.
    pub fn build(parent: &[u32]) -> Self {
        let n = parent.len();
        let mut start = vec![0_u32; n + 1];

        for &p in parent {
            if p != NONE {
                start[ix(p) + 1] += 1;
            }
        }

        for i in 0..n {
            start[i + 1] += start[i];
        }

        let mut fill = start.clone();
        let mut list = vec![0_u32; ix(start[n])];

        for (child, &p) in parent.iter().enumerate() {
            if p != NONE {
                let slot = &mut fill[ix(p)];
                list[ix(*slot)] = u32::try_from(child).unwrap_or(NONE);
                *slot += 1;
            }
        }

        Self { start, list }
    }

    /// Children of `p`.
    #[inline]
    pub fn of(&self, p: u32) -> &[u32] {
        &self.list[ix(self.start[ix(p)])..ix(self.start[ix(p) + 1])]
    }

    /// Sorts every child list with `cmp`, splitting the lists over `threads` threads in chunks
    /// of about equal numbers of children.
    pub fn sort_each(&mut self, threads: usize, cmp: impl Fn(u32, u32) -> Ordering + Sync) {
        let parents = self.start.len().saturating_sub(1);
        let total = self.list.len();
        let threads = threads.max(1);
        let start = &self.start;

        // Parent ranges whose children make up about `total / threads` entries each.
        let mut bounds: Vec<usize> = (0..=threads)
            .map(|t| {
                let target = u32::try_from(total * t / threads).unwrap_or(u32::MAX);
                start[..parents].partition_point(|&s| s < target)
            })
            .collect();
        bounds[threads] = parents;
        bounds.dedup();

        let mut rest = self.list.as_mut_slice();
        let mut taken = 0;

        std::thread::scope(|s| {
            for w in bounds.windows(2) {
                let (first, last) = (w[0], w[1]);
                let end = ix(start[last]);
                let (mine, tail) = rest.split_at_mut(end - taken);
                rest = tail;
                let base = taken;
                taken = end;
                let cmp = &cmp;

                s.spawn(move || {
                    for p in first..last {
                        let range = ix(start[p]) - base..ix(start[p + 1]) - base;
                        mine[range].sort_by(|&a, &b| cmp(a, b));
                    }
                });
            }
        });
    }
}

/// Pre-order (parent before children, siblings in child-list order) of the nodes reachable from
/// `root` through `alive` nodes. Dead nodes and their subtrees are skipped.
pub fn preorder(children: &Children, root: u32, alive: &[bool]) -> Vec<u32> {
    let mut order = Vec::with_capacity(alive.len());
    let mut stack = vec![root];

    while let Some(node) = stack.pop() {
        if !alive[ix(node)] {
            continue;
        }

        order.push(node);
        stack.extend(children.of(node).iter().rev().copied());
    }

    order
}

/// For every node in `order` (a pre-order), whether it is the last node among the alive
/// children of its parent. The root is its own last sibling.
pub fn last_alive_child(children: &Children, order: &[u32], alive: &[bool]) -> Vec<bool> {
    let mut last = vec![false; alive.len()];

    for &node in order {
        if let Some(&child) = children.of(node).iter().rev().find(|&&c| alive[ix(c)]) {
            last[ix(child)] = true;
        }
    }

    if let Some(&root) = order.first() {
        last[ix(root)] = true;
    }

    last
}

/// Marks hard-link duplicates among `n` nodes. `linked` lists the nodes whose inode has more
/// than one link, with that inode as key; among the nodes sharing a key only the least by `rank`
/// keeps `counted[i] == true`. All other nodes are counted. `linked` is reordered.
pub fn dedupe<K: Ord>(
    n: usize,
    linked: &mut [(u32, K)],
    mut rank: impl FnMut(u32, u32) -> Ordering,
) -> Vec<bool> {
    let mut counted = vec![true; n];

    linked.sort_by(|(a, ka), (b, kb)| ka.cmp(kb).then_with(|| rank(*a, *b)));

    for pair in linked.windows(2) {
        if pair[0].1 == pair[1].1 {
            counted[ix(pair[1].0)] = false;
        }
    }

    counted
}

/// Directory sizes: each directory gets the sum of the sizes of its `counted` children, or
/// `None` when that sum is zero; other nodes keep their size. `order` must be a pre-order of
/// every node. Sums saturate instead of overflowing.
pub fn aggregate(
    order: &[u32],
    parent: &[u32],
    counted: &[bool],
    is_dir: &[bool],
    size: &mut [Option<u64>],
) {
    let mut acc = vec![0_u64; size.len()];

    for &node in order.iter().rev() {
        let i = ix(node);

        if is_dir[i] {
            size[i] = (acc[i] > 0).then_some(acc[i]);
        }

        let p = parent[i];
        if p != NONE && counted[i] {
            acc[ix(p)] = acc[ix(p)].saturating_add(size[i].unwrap_or(0));
        }
    }
}

/// `--prune`: kills every alive non-root directory without an alive non-directory descendant.
/// `order` must be a pre-order of the alive nodes.
pub fn prune(order: &[u32], parent: &[u32], is_dir: &[bool], alive: &mut [bool]) {
    let mut has_file = vec![false; alive.len()];

    for &node in order.iter().rev() {
        let i = ix(node);

        if !is_dir[i] {
            has_file[i] = true;
        }

        let p = parent[i];
        if p == NONE {
            continue;
        }

        if has_file[i] {
            has_file[ix(p)] = true;
        } else {
            alive[i] = false;
        }
    }
}

/// `--dirs-only`: kills every non-root node that is not a directory.
pub fn dirs_only(parent: &[u32], is_dir: &[bool], alive: &mut [bool]) {
    for (i, a) in alive.iter_mut().enumerate() {
        if parent[i] != NONE && !is_dir[i] {
            *a = false;
        }
    }
}

/// `--min-size`: kills every alive non-root node smaller than `threshold` that has no alive
/// descendant of at least `threshold`. `order` must be a pre-order of the alive nodes. Returns
/// the killed nodes.
pub fn hide_small(
    order: &[u32],
    parent: &[u32],
    size: &[Option<u64>],
    threshold: u64,
    alive: &mut [bool],
) -> Vec<u32> {
    let mut shown = vec![false; alive.len()];
    let mut hidden = vec![];

    for &node in order.iter().rev() {
        let i = ix(node);

        if size[i].unwrap_or(0) >= threshold {
            shown[i] = true;
        }

        let p = parent[i];
        if p == NONE {
            continue;
        }

        if shown[i] {
            shown[ix(p)] = true;
        } else {
            alive[i] = false;
            hidden.push(node);
        }
    }

    hidden
}

/// Global index of record `index` of a sink, given each sink's offset (the total length of the
/// sinks before it).
#[inline]
pub const fn global_index(offsets: &[u32], sink: u32, index: u32) -> u32 {
    offsets[ix(sink)] + index
}

#[cfg(kani)]
mod proofs {
    //! Bounded proofs: every property holds for every tree with up to `N` nodes, every size
    //! assignment and every hard-link grouping.

    use super::*;

    const N: usize = 5;

    /// An arbitrary tree with `n <= N` nodes rooted at 0: every parent is a valid index and
    /// every node reaches the root.
    fn any_tree() -> Vec<u32> {
        let n: usize = kani::any();
        kani::assume(n >= 1 && n <= N);

        let mut parent = vec![NONE; n];
        for i in 1..n {
            let p: u32 = kani::any();
            kani::assume(ix(p) < n && ix(p) != i);
            parent[i] = p;
        }

        for i in 0..n {
            let mut cur = i;
            let mut steps = 0;
            while cur != 0 && steps < n {
                cur = ix(parent[cur]);
                steps += 1;
            }
            kani::assume(cur == 0);
        }

        parent
    }

    fn any_vec<T: kani::Arbitrary>(n: usize) -> Vec<T> {
        (0..n).map(|_| kani::any()).collect()
    }

    fn is_ancestor(parent: &[u32], a: u32, mut node: u32) -> bool {
        for _ in 0..parent.len() {
            if node == a {
                return true;
            }
            if parent[ix(node)] == NONE {
                return false;
            }
            node = parent[ix(node)];
        }
        node == a
    }

    /// Every non-root node is listed exactly once, under its parent.
    #[kani::proof]
    #[kani::unwind(7)]
    fn children_partition_nodes() {
        let parent = any_tree();
        let children = Children::build(&parent);

        for p in 0..parent.len() {
            for &c in children.of(p as u32) {
                assert_eq!(parent[ix(c)], p as u32);
            }
        }

        let listed: usize = (0..parent.len()).map(|p| children.of(p as u32).len()).sum();
        assert_eq!(listed, parent.len() - 1);
    }

    /// The pre-order lists exactly the alive nodes whose ancestors are all alive, each once,
    /// root first, every node after its parent, and every subtree contiguous.
    #[kani::proof]
    #[kani::unwind(7)]
    fn preorder_is_a_valid_preorder() {
        let parent = any_tree();
        let n = parent.len();
        let alive: Vec<bool> = any_vec(n);
        kani::assume(alive[0]);
        let children = Children::build(&parent);
        let order = preorder(&children, 0, &alive);

        let mut pos = vec![usize::MAX; n];
        for (k, &node) in order.iter().enumerate() {
            assert_eq!(pos[ix(node)], usize::MAX, "listed twice");
            pos[ix(node)] = k;
        }

        assert_eq!(order[0], 0);

        for i in 0..n {
            let reachable = (0..n).all(|a| !is_ancestor(&parent, a as u32, i as u32) || alive[a]);
            assert_eq!(pos[i] != usize::MAX, reachable);

            if pos[i] != usize::MAX && i != 0 {
                assert!(pos[ix(parent[i])] < pos[i]);
            }
        }

        // Subtrees are contiguous: everything between a node and one of its listed
        // descendants is also its descendant.
        for (k, &a) in order.iter().enumerate() {
            for (m, &d) in order.iter().enumerate().skip(k) {
                if is_ancestor(&parent, a, d) {
                    for &between in &order[k..=m] {
                        assert!(is_ancestor(&parent, a, between));
                    }
                }
            }
        }
    }

    /// Exactly one alive child per parent with alive children is marked last, and it is the
    /// last alive one.
    #[kani::proof]
    #[kani::unwind(7)]
    fn last_child_is_unique() {
        let parent = any_tree();
        let n = parent.len();
        let alive: Vec<bool> = any_vec(n);
        kani::assume(alive[0]);
        let children = Children::build(&parent);
        let order = preorder(&children, 0, &alive);
        let last = last_alive_child(&children, &order, &alive);

        for &p in &order {
            let alive_children: Vec<u32> = children
                .of(p)
                .iter()
                .copied()
                .filter(|&c| alive[ix(c)])
                .collect();
            let marked = alive_children.iter().filter(|&&c| last[ix(c)]).count();
            assert_eq!(marked, usize::from(!alive_children.is_empty()));
            if let Some(&l) = alive_children.last() {
                assert!(last[ix(l)]);
            }
        }
    }

    /// Each key group keeps exactly one counted member, the least by rank; unkeyed nodes are
    /// counted.
    #[kani::proof]
    #[kani::unwind(7)]
    fn dedupe_counts_each_inode_once() {
        let n: usize = kani::any();
        kani::assume(n <= N);
        let key: Vec<Option<u8>> = (0..n)
            .map(|_| {
                let k: Option<u8> = kani::any();
                kani::assume(k.is_none_or(|k| k < 3));
                k
            })
            .collect();

        let mut linked: Vec<(u32, u8)> = (0..n)
            .filter_map(|i| key[i].map(|k| (i as u32, k)))
            .collect();
        let counted = dedupe(n, &mut linked, |a, b| a.cmp(&b));

        for i in 0..n {
            match key[i] {
                None => assert!(counted[i]),
                Some(k) => {
                    let first = (0..n).find(|&j| key[j] == Some(k)).unwrap();
                    assert_eq!(counted[i], i == first);
                },
            }
        }
    }

    /// A directory's size is the sum of the sizes of the files below it that reach it through
    /// counted nodes only (or `None` when zero); files keep their sizes.
    #[kani::proof]
    #[kani::unwind(7)]
    fn aggregate_sums_counted_files() {
        let parent = any_tree();
        let n = parent.len();
        let counted: Vec<bool> = any_vec(n);
        let is_dir: Vec<bool> = any_vec(n);
        let files: Vec<Option<u64>> = any_vec(n);

        // Files have no children; directories start without a size.
        for i in 0..n {
            if i != 0 {
                kani::assume(is_dir[ix(parent[i])]);
            }
            if is_dir[i] {
                kani::assume(files[i].is_none());
            } else {
                kani::assume(files[i].is_none_or(|s| s < 1 << 60));
            }
        }

        let children = Children::build(&parent);
        let order = preorder(&children, 0, &vec![true; n]);
        let mut size = files.clone();
        aggregate(&order, &parent, &counted, &is_dir, &mut size);

        for d in 0..n {
            if !is_dir[d] {
                assert_eq!(size[d], files[d]);
                continue;
            }

            let mut expected = 0_u64;
            for f in 0..n {
                if is_dir[f] || f == d || !is_ancestor(&parent, d as u32, f as u32) {
                    continue;
                }
                let mut cur = f;
                let mut ok = true;
                while cur != d {
                    ok &= counted[cur];
                    cur = ix(parent[cur]);
                }
                if ok {
                    expected += files[f].unwrap_or(0);
                }
            }

            assert_eq!(size[d], (expected > 0).then_some(expected));
        }
    }

    /// After pruning, the alive non-root directories are exactly those with an alive file
    /// below them, and no file is removed.
    #[kani::proof]
    #[kani::unwind(7)]
    fn prune_removes_exactly_empty_dirs() {
        let parent = any_tree();
        let n = parent.len();
        let is_dir: Vec<bool> = any_vec(n);
        for i in 1..n {
            kani::assume(is_dir[ix(parent[i])]);
        }

        let children = Children::build(&parent);
        let before = vec![true; n];
        let order = preorder(&children, 0, &before);
        let mut alive = before.clone();
        prune(&order, &parent, &is_dir, &mut alive);

        assert!(alive[0]);
        for i in 1..n {
            let has_file = (0..n).any(|f| !is_dir[f] && is_ancestor(&parent, i as u32, f as u32));
            assert_eq!(alive[i], has_file);
        }
    }

    /// `--dirs-only` keeps the root and the directories, and the kept set stays connected.
    #[kani::proof]
    #[kani::unwind(7)]
    fn dirs_only_keeps_connected_dirs() {
        let parent = any_tree();
        let n = parent.len();
        let is_dir: Vec<bool> = any_vec(n);
        for i in 1..n {
            kani::assume(is_dir[ix(parent[i])]);
        }

        let mut alive = vec![true; n];
        dirs_only(&parent, &is_dir, &mut alive);

        for i in 0..n {
            assert_eq!(alive[i], i == 0 || is_dir[i]);
            if alive[i] && i != 0 {
                assert!(alive[ix(parent[i])]);
            }
        }
    }

    /// `--min-size` keeps exactly the nodes that reach the threshold or have a kept
    /// descendant, plus the root, and reports every other node as hidden.
    #[kani::proof]
    #[kani::unwind(7)]
    fn hide_small_keeps_large_and_their_ancestors() {
        let parent = any_tree();
        let n = parent.len();
        let size: Vec<Option<u64>> = any_vec(n);
        let threshold: u64 = kani::any();

        let children = Children::build(&parent);
        let before = vec![true; n];
        let order = preorder(&children, 0, &before);
        let mut alive = before.clone();
        let hidden = hide_small(&order, &parent, &size, threshold, &mut alive);

        assert!(alive[0]);
        for i in 1..n {
            let large_below = (0..n).any(|d| {
                size[d].unwrap_or(0) >= threshold && is_ancestor(&parent, i as u32, d as u32)
            });
            assert_eq!(alive[i], large_below);
            assert_eq!(hidden.contains(&(i as u32)), !large_below);
        }
    }

    /// Sink offsets map every record to a distinct global index below the total.
    #[kani::proof]
    #[kani::unwind(5)]
    fn global_indices_are_distinct() {
        let lens: [u32; 3] = [kani::any(), kani::any(), kani::any()];
        kani::assume(lens.iter().all(|&l| l <= 4));
        let offsets = [0, lens[0], lens[0] + lens[1]];

        let (s1, i1, s2, i2): (u32, u32, u32, u32) = kani::any();
        kani::assume(s1 < 3 && s2 < 3 && i1 < lens[ix(s1)] && i2 < lens[ix(s2)]);

        let g1 = global_index(&offsets, s1, i1);
        let g2 = global_index(&offsets, s2, i2);
        assert!(g1 < lens.iter().sum::<u32>());
        assert_eq!(g1 == g2, (s1, i1) == (s2, i2));
    }
}

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
        // At most `n` children in total. Allocating `n` (a length known from the input size)
        // rather than `start[n]` keeps the allocation size concrete for Kani.
        let mut list = vec![0_u32; n];

        for (child, &p) in parent.iter().enumerate() {
            if p != NONE {
                let slot = &mut fill[ix(p)];
                list[ix(*slot)] = u32::try_from(child).unwrap_or(NONE);
                *slot += 1;
            }
        }

        list.truncate(ix(start[n]));

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
    // Every node is pushed at most once (by its parent), so the stack never grows past this.
    let mut stack = Vec::with_capacity(alive.len());
    stack.push(root);

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
    let mut hidden = Vec::with_capacity(alive.len());

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

/// Specifications of the functions above, checked two ways: exhaustively for every tree of up
/// to [`spec::MAX_TEST`] nodes by `cargo test`, and symbolically by Kani (`cargo kani`, see
/// `proofs`) for every tree of up to 3 nodes and every size, threshold and inode grouping. Both
/// cover every root position.
#[cfg(any(test, kani))]
// The specifications index several per-node arrays by node number.
#[allow(clippy::needless_range_loop)]
mod spec {
    use super::*;

    /// Largest tree checked by the exhaustive tests.
    pub const MAX_TEST: usize = 6;

    /// Node index from a position.
    fn id(i: usize) -> u32 {
        u32::try_from(i).unwrap_or(NONE)
    }

    /// Whether `parent` is a tree rooted at `root`: only the root has no parent, every other
    /// parent is another valid node, and every node reaches the root.
    pub fn is_tree(parent: &[u32], root: u32) -> bool {
        let n = parent.len();
        let r = ix(root);

        if r >= n || parent[r] != NONE {
            return false;
        }

        (0..n).all(|i| i == r || ix(parent[i]) < n && ix(parent[i]) != i)
            && (0..n).all(|i| {
                let mut cur = i;
                let mut steps = 0;
                while cur != r && steps < n {
                    cur = ix(parent[cur]);
                    steps += 1;
                }
                cur == r
            })
    }

    /// Whether only directories have children.
    pub fn only_dirs_have_children(parent: &[u32], is_dir: &[bool]) -> bool {
        parent.iter().all(|&p| p == NONE || is_dir[ix(p)])
    }

    /// Whether `a` is `node` or one of its ancestors.
    /// Ancestor sets as bitmasks: bit `a` of `anc[i]` is set when `a` is `i` or one of its
    /// ancestors. Computed once per tree (`n` rounds of propagation, enough for any depth), so
    /// each query is a shift instead of a walk up the parents. Supports up to 16 nodes.
    pub fn ancestors(parent: &[u32]) -> Vec<u16> {
        let n = parent.len();
        let mut anc: Vec<u16> = (0..n).map(|i| 1 << i).collect();

        for _ in 0..n {
            for i in 0..n {
                if parent[i] != NONE {
                    anc[i] |= anc[ix(parent[i])];
                }
            }
        }

        anc
    }

    /// Whether `a` is `node` or one of its ancestors, given [`ancestors`].
    const fn has_ancestor(anc: &[u16], node: usize, a: usize) -> bool {
        anc[node] >> a & 1 == 1
    }

    /// Whether `a` is `node` or one of its ancestors, by walking up the parents. Reference for
    /// [`ancestors`] in the tests.
    #[cfg(test)]
    pub fn is_ancestor(parent: &[u32], a: u32, mut node: u32) -> bool {
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

    fn all_alive(n: usize) -> Vec<bool> {
        vec![true; n]
    }

    /// Every non-root node is listed exactly once, under its parent.
    pub fn children_partition_nodes(parent: &[u32]) {
        let children = Children::build(parent);
        let n = parent.len();

        for p in 0..n {
            for &c in children.of(id(p)) {
                assert_eq!(parent[ix(c)], id(p));
            }
        }

        let listed: usize = (0..n).map(|p| children.of(id(p)).len()).sum();
        assert_eq!(listed, n - 1);
    }

    /// Whether `order` lists every node exactly once, root first, each node after its parent.
    /// `aggregate`, `prune` and `hide_small` require such an order; every pre-order of the whole
    /// tree is one, which `preorder_is_a_valid_preorder` checks.
    pub fn is_parent_first_order(parent: &[u32], root: u32, order: &[u32]) -> bool {
        let n = parent.len();

        if order.len() != n || order.first() != Some(&root) {
            return false;
        }

        let mut pos = vec![usize::MAX; n];
        for (k, &node) in order.iter().enumerate() {
            if ix(node) >= n || pos[ix(node)] != usize::MAX {
                return false;
            }
            pos[ix(node)] = k;
        }

        (0..n).all(|i| parent[i] == NONE || pos[ix(parent[i])] < pos[i])
    }

    /// `sort_each` sorts every child list by the comparator and keeps its members, for any
    /// number of threads.
    #[cfg(test)]
    pub fn sort_each_sorts_every_list(parent: &[u32], threads: usize) {
        let original = Children::build(parent);
        let mut sorted = Children::build(parent);
        sorted.sort_each(threads, |a, b| b.cmp(&a));

        for p in 0..parent.len() {
            let mut expected = original.of(id(p)).to_vec();
            expected.sort_by(|a, b| b.cmp(a));
            assert_eq!(sorted.of(id(p)), expected.as_slice());
        }
    }

    /// The pre-order lists exactly the alive nodes whose ancestors are all alive, each once,
    /// root first, every node after its parent, siblings in child-list order, and every subtree
    /// contiguous.
    pub fn preorder_is_a_valid_preorder(
        parent: &[u32],
        children: &Children,
        root: u32,
        alive: &[bool],
    ) {
        let n = parent.len();
        let order = preorder(children, root, alive);

        let mut pos = vec![usize::MAX; n];
        for (k, &node) in order.iter().enumerate() {
            assert_eq!(pos[ix(node)], usize::MAX, "listed twice");
            pos[ix(node)] = k;
        }

        assert_eq!(order[0], root);
        let anc = ancestors(parent);

        for i in 0..n {
            let reachable = (0..n).all(|a| !has_ancestor(&anc, i, a) || alive[a]);
            assert_eq!(pos[i] != usize::MAX, reachable);

            if pos[i] != usize::MAX && id(i) != root {
                assert!(pos[ix(parent[i])] < pos[i]);
            }
        }

        // Siblings keep their child-list order.
        for p in 0..n {
            let mut previous = None;
            for &c in children.of(id(p)) {
                let k = pos[ix(c)];
                if k != usize::MAX {
                    assert!(previous.is_none_or(|prev| prev < k));
                    previous = Some(k);
                }
            }
        }

        assert!(subtrees_contiguous(&anc, &pos));

        // With every node alive, the pre-order is an order the size and filter passes accept,
        // which their proofs assume.
        if alive.iter().all(|&a| a) {
            assert!(is_parent_first_order(parent, root, &order));
        }
    }

    /// Whether every listed subtree occupies consecutive positions: the listed descendants of
    /// `a` (including `a`) all fall in `pos[a] .. pos[a] + count`, where `count` is their
    /// number. Positions are distinct, so that window is exactly filled. `pos` is
    /// `usize::MAX` for unlisted nodes.
    pub fn subtrees_contiguous(anc: &[u16], pos: &[usize]) -> bool {
        let n = pos.len();

        (0..n).filter(|&a| pos[a] != usize::MAX).all(|a| {
            let below = |d: usize| pos[d] != usize::MAX && has_ancestor(anc, d, a);
            let count = (0..n).filter(|&d| below(d)).count();
            (0..n)
                .filter(|&d| below(d))
                .all(|d| pos[a] <= pos[d] && pos[d] < pos[a] + count)
        })
    }

    /// Reference for [`subtrees_contiguous`]: everything listed between a node and one of its
    /// listed descendants is also its descendant.
    #[cfg(test)]
    pub fn subtrees_contiguous_reference(parent: &[u32], order: &[u32]) -> bool {
        order.iter().enumerate().all(|(k, &a)| {
            order.iter().enumerate().skip(k).all(|(m, &d)| {
                !is_ancestor(parent, a, d)
                    || order[k..=m]
                        .iter()
                        .all(|&between| is_ancestor(parent, a, between))
            })
        })
    }

    /// Exactly one alive child of each listed node with alive children is marked last, and it
    /// is the last alive one.
    pub fn last_child_is_unique(parent: &[u32], root: u32, alive: &[bool]) {
        let children = Children::build(parent);
        let order = preorder(&children, root, alive);
        let last = last_alive_child(&children, &order, alive);

        for &p in &order {
            let mut marked = 0;
            let mut last_alive = None;
            for &c in children.of(p) {
                if alive[ix(c)] {
                    marked += usize::from(last[ix(c)]);
                    last_alive = Some(c);
                }
            }
            assert_eq!(marked, usize::from(last_alive.is_some()));
            if let Some(l) = last_alive {
                assert!(last[ix(l)]);
            }
        }
    }

    /// Each key group keeps exactly one counted member, the least by rank (here the index);
    /// nodes without a key are counted.
    ///
    /// `linked` lists the nodes with a key, in increasing node order, as production builds it
    /// from its sparse list of multiply-linked inodes.
    pub fn dedupe_counts_each_inode_once(n: usize, linked: &[(u32, u8)]) {
        let mut key = vec![None; n];
        for &(i, k) in linked {
            key[ix(i)] = Some(k);
        }

        let mut work = linked.to_vec();
        let counted = dedupe(n, &mut work, |a, b| a.cmp(&b));

        for i in 0..n {
            match key[i] {
                None => assert!(counted[i]),
                Some(k) => {
                    let first = (0..n).find(|&j| key[j] == Some(k));
                    assert_eq!(counted[i], first == Some(i));
                },
            }
        }
    }

    /// A directory's size is the sum of the sizes of the files below it that reach it through
    /// counted nodes only, saturating at `u64::MAX`, or `None` when that sum is zero; files keep
    /// their sizes. Holds for every `order` accepted by [`is_parent_first_order`].
    pub fn aggregate_sums_counted_files(
        parent: &[u32],
        order: &[u32],
        counted: &[bool],
        is_dir: &[bool],
        files: &[Option<u64>],
    ) {
        let n = parent.len();
        let mut size = files.to_vec();
        aggregate(order, parent, counted, is_dir, &mut size);
        let anc = ancestors(parent);

        for d in 0..n {
            if !is_dir[d] {
                assert_eq!(size[d], files[d]);
                continue;
            }

            let mut expected = 0_u128;
            for f in 0..n {
                if is_dir[f] || f == d || !has_ancestor(&anc, f, d) {
                    continue;
                }
                // The path from `f` up to `d`, excluding `d`, is the ancestors of `f` that are
                // not ancestors of `d`; all of them must be counted.
                let reaches = (0..n)
                    .all(|m| !has_ancestor(&anc, f, m) || has_ancestor(&anc, d, m) || counted[m]);
                if reaches {
                    expected += u128::from(files[f].unwrap_or(0));
                }
            }

            let expected = u64::try_from(expected).unwrap_or(u64::MAX);
            assert_eq!(size[d], (expected > 0).then_some(expected));
        }
    }

    /// After pruning, the alive non-root nodes are exactly the files and the directories with
    /// a file below them. Holds for every `order` accepted by [`is_parent_first_order`].
    pub fn prune_removes_exactly_empty_dirs(
        parent: &[u32],
        root: u32,
        order: &[u32],
        is_dir: &[bool],
    ) {
        let n = parent.len();
        let mut alive = all_alive(n);
        prune(order, parent, is_dir, &mut alive);

        let anc = ancestors(parent);

        assert!(alive[ix(root)]);
        for i in (0..n).filter(|&i| id(i) != root) {
            let has_file = (0..n).any(|f| !is_dir[f] && has_ancestor(&anc, f, i));
            assert_eq!(alive[i], has_file);
        }
    }

    /// `--dirs-only` keeps the root and the directories, and the kept set stays connected.
    pub fn dirs_only_keeps_connected_dirs(parent: &[u32], root: u32, is_dir: &[bool]) {
        let n = parent.len();
        let mut alive = all_alive(n);
        dirs_only(parent, is_dir, &mut alive);

        for i in 0..n {
            assert_eq!(alive[i], id(i) == root || is_dir[i]);
            if alive[i] && id(i) != root {
                assert!(alive[ix(parent[i])]);
            }
        }
    }

    /// `--min-size` keeps the root and exactly the nodes with a node of at least `threshold`
    /// in their subtree, and reports every other node as hidden. Holds for every `order`
    /// accepted by [`is_parent_first_order`].
    pub fn hide_small_keeps_large_and_their_ancestors(
        parent: &[u32],
        root: u32,
        order: &[u32],
        size: &[Option<u64>],
        threshold: u64,
    ) {
        let n = parent.len();
        let mut alive = all_alive(n);
        let hidden = hide_small(order, parent, size, threshold, &mut alive);
        let anc = ancestors(parent);

        // How often each node is reported hidden.
        let mut reported = vec![0_usize; n];
        for &h in &hidden {
            reported[ix(h)] += 1;
        }

        assert!(alive[ix(root)]);
        assert_eq!(reported[ix(root)], 0);
        for i in (0..n).filter(|&i| id(i) != root) {
            let large_below =
                (0..n).any(|d| size[d].unwrap_or(0) >= threshold && has_ancestor(&anc, d, i));
            assert_eq!(alive[i], large_below);
            assert_eq!(reported[i], usize::from(!large_below));
        }
    }

    /// Sink offsets map every record to a distinct global index below the total.
    pub fn global_indices_are_distinct(lens: [u32; 3], a: (u32, u32), b: (u32, u32)) {
        let offsets = [0, lens[0], lens[0] + lens[1]];
        let total: u32 = lens.iter().sum();
        let g1 = global_index(&offsets, a.0, a.1);
        let g2 = global_index(&offsets, b.0, b.1);
        assert!(g1 < total);
        assert_eq!(g1 == g2, a == b);
    }
}

#[cfg(test)]
mod test {
    use super::{NONE, spec};

    /// Every tree with `n` nodes and any root, as (parent array, root). Production puts the
    /// root last, so every root position is checked.
    fn trees(n: usize) -> Vec<(Vec<u32>, u32)> {
        let mut out = vec![];
        let combos = n.pow(u32::try_from(n.saturating_sub(1)).unwrap());

        for r in 0..n {
            let root = u32::try_from(r).unwrap();

            for mut code in 0..combos {
                let mut parent = vec![NONE; n];
                for (i, p) in parent.iter_mut().enumerate() {
                    if i != r {
                        *p = u32::try_from(code % n).unwrap();
                        code /= n;
                    }
                }
                if spec::is_tree(&parent, root) {
                    out.push((parent, root));
                }
            }
        }

        out
    }

    /// Nodes in breadth-first order from `root`: another parent-first order.
    fn breadth_first(children: &super::Children, root: u32) -> Vec<u32> {
        let mut order = vec![root];
        let mut k = 0;
        while k < order.len() {
            order.extend_from_slice(children.of(order[k]));
            k += 1;
        }
        order
    }

    /// The `n` low bits of `mask`.
    fn bits(mask: usize, n: usize) -> Vec<bool> {
        (0..n).map(|i| mask >> i & 1 == 1).collect()
    }

    /// The ancestor bitsets and the interval form of contiguity used by the specs agree with
    /// their direct definitions, so the cheaper specs are not weaker.
    #[test]
    fn spec_helpers_match_references() {
        for n in 1..=spec::MAX_TEST {
            for (parent, _) in trees(n) {
                let anc = spec::ancestors(&parent);
                for (node, bits) in anc.iter().enumerate() {
                    for a in 0..n {
                        let node32 = u32::try_from(node).unwrap();
                        let a32 = u32::try_from(a).unwrap();
                        assert_eq!(bits >> a & 1 == 1, spec::is_ancestor(&parent, a32, node32));
                    }
                }
            }
        }

        // Every ordering of every subset of nodes, for trees of up to 4 nodes.
        for n in 1..=4 {
            for (parent, _) in trees(n) {
                let anc = spec::ancestors(&parent);
                for subset in 0_usize..1 << n {
                    let nodes: Vec<u32> = (0..n)
                        .filter(|&i| subset >> i & 1 == 1)
                        .map(|i| u32::try_from(i).unwrap())
                        .collect();
                    for order in permutations(&nodes) {
                        let mut pos = vec![usize::MAX; n];
                        for (k, &node) in order.iter().enumerate() {
                            pos[super::ix(node)] = k;
                        }

                        // The specs assert parents first separately; on other orders the
                        // interval form is stricter (it also rejects a descendant listed
                        // before its ancestor).
                        let ancestors_first = (0..n).all(|d| {
                            (0..n).all(|a| {
                                pos[a] == usize::MAX
                                    || pos[d] == usize::MAX
                                    || anc[d] >> a & 1 == 0
                                    || pos[a] <= pos[d]
                            })
                        });
                        if !ancestors_first {
                            assert!(!spec::subtrees_contiguous(&anc, &pos));
                            continue;
                        }

                        assert_eq!(
                            spec::subtrees_contiguous(&anc, &pos),
                            spec::subtrees_contiguous_reference(&parent, &order)
                        );
                    }
                }
            }
        }
    }

    /// Every ordering of `items`.
    fn permutations(items: &[u32]) -> Vec<Vec<u32>> {
        if items.is_empty() {
            return vec![vec![]];
        }

        (0..items.len())
            .flat_map(|k| {
                let mut rest = items.to_vec();
                let first = rest.remove(k);
                permutations(&rest).into_iter().map(move |mut tail| {
                    tail.insert(0, first);
                    tail
                })
            })
            .collect()
    }

    #[test]
    fn trees_are_all_rooted_labeled_trees() {
        // There are n^(n-1) rooted labeled trees on n nodes (Cayley).
        for n in 1..=spec::MAX_TEST {
            assert_eq!(trees(n).len(), n.pow(u32::try_from(n - 1).unwrap()));
        }
    }

    #[test]
    fn exhaustive_small_trees() {
        use super::Children;

        for n in 1..=spec::MAX_TEST {
            for (parent, root) in trees(n) {
                let r = super::ix(root);
                spec::children_partition_nodes(&parent);

                // Child lists as built, and sorted (descending) as production sorts them
                // before the pre-order.
                let built = Children::build(&parent);
                let mut sorted = Children::build(&parent);
                sorted.sort_each(2, |a, b| b.cmp(&a));

                // Masks with the root set: alive flags for the pre-order.
                for mask in (0..1 << n).filter(|m| m >> r & 1 == 1) {
                    let alive = bits(mask, n);
                    spec::preorder_is_a_valid_preorder(&parent, &built, root, &alive);
                    spec::preorder_is_a_valid_preorder(&parent, &sorted, root, &alive);
                    spec::last_child_is_unique(&parent, root, &alive);
                }

                // Two different parent-first orders: the pre-order production uses, and
                // breadth-first.
                let everyone = vec![true; n];
                let orders = [
                    super::preorder(&sorted, root, &everyone),
                    breadth_first(&built, root),
                ];
                for order in &orders {
                    assert!(spec::is_parent_first_order(&parent, root, order));
                }

                for mask in 0..1 << n {
                    let is_dir = &bits(mask, n);
                    if !spec::only_dirs_have_children(&parent, is_dir) {
                        continue;
                    }

                    for order in &orders {
                        spec::prune_removes_exactly_empty_dirs(&parent, root, order, is_dir);
                    }
                    spec::dirs_only_keeps_connected_dirs(&parent, root, is_dir);

                    // Distinct powers of two make every subset sum distinct; values near
                    // u64::MAX exercise saturation.
                    let small: Vec<Option<u64>> =
                        (0..n).map(|i| (!is_dir[i]).then_some(1 << i)).collect();
                    let huge: Vec<Option<u64>> = (0..n)
                        .map(|i| (!is_dir[i]).then_some(u64::MAX - (1 << i)))
                        .collect();

                    // Whether the root is counted does not matter (it has no parent), so it
                    // stays counted.
                    for counted in (0..1 << n).filter(|m| m >> r & 1 == 1) {
                        let counted = bits(counted, n);
                        for order in &orders {
                            for files in [&small, &huge] {
                                spec::aggregate_sums_counted_files(
                                    &parent, order, &counted, is_dir, files,
                                );
                            }
                        }
                    }

                    // Sizes of 2^i, with and without the directories sized; thresholds at
                    // every distinct boundary.
                    let all_sized: Vec<Option<u64>> = (0..n).map(|i| Some(1 << i)).collect();
                    let files_sized: Vec<Option<u64>> =
                        (0..n).map(|i| (!is_dir[i]).then_some(1 << i)).collect();
                    let thresholds = (0..=n).map(|i| 1_u64 << i).chain([0]);
                    for threshold in thresholds {
                        for sizes in [&all_sized, &files_sized] {
                            for order in &orders {
                                spec::hide_small_keeps_large_and_their_ancestors(
                                    &parent, root, order, sizes, threshold,
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn exhaustive_sort_each() {
        for n in 1..=spec::MAX_TEST {
            for (parent, _) in trees(n) {
                for threads in 1..=3 {
                    spec::sort_each_sorts_every_list(&parent, threads);
                }
            }
        }
    }

    #[test]
    fn exhaustive_dedupe() {
        for n in 0..=spec::MAX_TEST {
            // Every assignment of no key or one of `n` keys to each node; `n` keys allow every
            // grouping, including all distinct.
            let choices = n + 1;
            for code in 0..choices.pow(u32::try_from(n).unwrap()) {
                let linked: Vec<(u32, u8)> = (0..n)
                    .filter_map(|i| {
                        let choice = code / choices.pow(u32::try_from(i).unwrap()) % choices;
                        (choice > 0)
                            .then(|| (u32::try_from(i).unwrap(), u8::try_from(choice - 1).unwrap()))
                    })
                    .collect();
                spec::dedupe_counts_each_inode_once(n, &linked);
            }
        }
    }

    #[test]
    fn exhaustive_global_indices() {
        for lens in [[0, 2, 3], [3, 0, 1], [1, 1, 1], [4, 4, 4]] {
            for a in (0..3).flat_map(|s| (0..lens[s as usize]).map(move |i| (s, i))) {
                for b in (0..3).flat_map(|s| (0..lens[s as usize]).map(move |i| (s, i))) {
                    spec::global_indices_are_distinct(lens, a, b);
                }
            }
        }
    }
}

#[cfg(kani)]
mod proofs {
    //! Kani harnesses: the [`spec`] properties for symbolic inputs, one concrete tree size at a
    //! time so allocations have concrete lengths.

    use super::{NONE, ix, spec};

    /// An arbitrary tree with `N` nodes and any root. A symbolic depth per node witnesses
    /// that the parent array is a tree: the root has depth 0 and every other node is one deeper
    /// than its parent, so parent chains cannot cycle and must end at the root. This accepts
    /// exactly the trees `spec::is_tree` accepts, without unrolling a walk to the root for each
    /// node.
    fn any_tree<const N: usize>() -> ([u32; N], u32) {
        let root: u8 = kani::any();
        kani::assume(usize::from(root) < N);
        let root = u32::from(root);

        let mut parent = [NONE; N];
        let depth: [u8; N] = kani::any();

        for i in 0..N {
            if i == ix(root) {
                kani::assume(depth[i] == 0);
            } else {
                let p: u8 = kani::any();
                kani::assume(usize::from(p) < N && usize::from(p) != i);
                parent[i] = u32::from(p);
                kani::assume(usize::from(depth[usize::from(p)]) < N);
                kani::assume(depth[i] == depth[usize::from(p)] + 1);
            }
        }

        (parent, root)
    }

    fn children_partition_nodes<const N: usize>() {
        spec::children_partition_nodes(&any_tree::<N>().0);
    }

    fn preorder_is_a_valid_preorder<const N: usize>() {
        let (parent, root) = any_tree::<N>();
        let alive: [bool; N] = kani::any();
        kani::assume(alive[ix(root)]);
        let children = super::Children::build(&parent);
        spec::preorder_is_a_valid_preorder(&parent, &children, root, &alive);
    }

    fn last_child_is_unique<const N: usize>() {
        let (parent, root) = any_tree::<N>();
        let alive: [bool; N] = kani::any();
        kani::assume(alive[ix(root)]);
        spec::last_child_is_unique(&parent, root, &alive);
    }

    /// `M` of the `N` nodes are linked: their ids are symbolic and strictly increasing, their
    /// keys symbolic. The harnesses for `M` from 0 to `N` together cover every key assignment,
    /// and the concrete length keeps std's sort from being unrolled over every possible length.
    fn dedupe_counts_each_inode_once<const N: usize, const M: usize>() {
        let mut linked = [(0_u32, 0_u8); M];
        for j in 0..M {
            let node: u8 = kani::any();
            // Keys are only compared, so values below N cover every grouping.
            let key: u8 = kani::any();
            kani::assume(usize::from(node) < N && usize::from(key) < N);
            kani::assume(j == 0 || u32::from(node) > linked[j - 1].0);
            linked[j] = (u32::from(node), key);
        }
        spec::dedupe_counts_each_inode_once(N, &linked);
    }

    /// Any order of all nodes with each parent first (what `aggregate`, `prune` and
    /// `hide_small` require). The pre-order harness proves that `preorder` produces one, so these
    /// harnesses need not model `Children::build` and `preorder`, and they hold for every such
    /// order.
    fn any_parent_first_order<const N: usize>(parent: &[u32; N], root: u32) -> [u32; N] {
        let order: [u8; N] = kani::any();
        let order = order.map(u32::from);
        kani::assume(spec::is_parent_first_order(parent, root, &order));
        order
    }

    fn aggregate_sums_counted_files<const N: usize>() {
        let (parent, root) = any_tree::<N>();
        let order = any_parent_first_order(&parent, root);
        let counted: [bool; N] = kani::any();
        let is_dir: [bool; N] = kani::any();
        let files: [Option<u64>; N] = kani::any();
        kani::assume(spec::only_dirs_have_children(&parent, &is_dir));
        for i in 0..N {
            // Directories start without a size; files may have any size.
            kani::assume(!is_dir[i] || files[i].is_none());
        }
        spec::aggregate_sums_counted_files(&parent, &order, &counted, &is_dir, &files);
    }

    fn prune_removes_exactly_empty_dirs<const N: usize>() {
        let (parent, root) = any_tree::<N>();
        let is_dir: [bool; N] = kani::any();
        kani::assume(spec::only_dirs_have_children(&parent, &is_dir));
        let order = any_parent_first_order(&parent, root);
        spec::prune_removes_exactly_empty_dirs(&parent, root, &order, &is_dir);
    }

    fn dirs_only_keeps_connected_dirs<const N: usize>() {
        let (parent, root) = any_tree::<N>();
        let is_dir: [bool; N] = kani::any();
        kani::assume(spec::only_dirs_have_children(&parent, &is_dir));
        spec::dirs_only_keeps_connected_dirs(&parent, root, &is_dir);
    }

    fn hide_small_keeps_large_and_their_ancestors<const N: usize>() {
        let (parent, root) = any_tree::<N>();
        let size: [Option<u64>; N] = kani::any();
        let order = any_parent_first_order(&parent, root);
        spec::hide_small_keeps_large_and_their_ancestors(&parent, root, &order, &size, kani::any());
    }

    /// One harness per property and tree size, so each CBMC problem stays small and
    /// `cargo kani -j` can run them in parallel. Loops run at most `N + 1` times; the unwind
    /// bound is the smallest that passes the unwinding checks, which stay enabled, so a bound
    /// that is too small fails instead of passing silently.
    macro_rules! harnesses {
        ($($name:ident: $check:ident, [$($size:ident = $n:literal, unwind $u:literal);*];)*) => {
            $(
                mod $name {
                    use super::*;

                    $(
                        #[kani::proof]
                        #[kani::unwind($u)]
                        fn $size() {
                            $check::<$n>();
                        }
                    )*
                }
            )*
        };
    }

    // `preorder` and `last_child` take only a tree shape and a set of alive nodes, which the
    // exhaustive tests enumerate completely up to `spec::MAX_TEST` nodes. Under Kani their
    // pre-order has a symbolic length, which costs about 8 GB at 2 nodes, so they keep a
    // 1-node harness as a smoke check of the real code paths only.
    harnesses! {
        children: children_partition_nodes, [n1 = 1, unwind 3; n2 = 2, unwind 4; n3 = 3, unwind 5];
        preorder: preorder_is_a_valid_preorder, [n1 = 1, unwind 3];
        last_child: last_child_is_unique, [n1 = 1, unwind 3];
        aggregate: aggregate_sums_counted_files, [n1 = 1, unwind 3; n2 = 2, unwind 4; n3 = 3, unwind 5];
        prune: prune_removes_exactly_empty_dirs, [n1 = 1, unwind 3; n2 = 2, unwind 4; n3 = 3, unwind 5];
        dirs_only: dirs_only_keeps_connected_dirs, [n1 = 1, unwind 3; n2 = 2, unwind 4; n3 = 3, unwind 5];
        hide_small: hide_small_keeps_large_and_their_ancestors, [n1 = 1, unwind 3; n2 = 2, unwind 4; n3 = 3, unwind 5];
    }

    /// `dedupe` for `N` nodes of which `M` are linked.
    mod dedupe {
        use super::*;

        macro_rules! cases {
            ($($name:ident = $n:literal, $m:literal;)*) => {
                $(
                    #[kani::proof]
                    #[kani::unwind(5)]
                    fn $name() {
                        dedupe_counts_each_inode_once::<$n, $m>();
                    }
                )*
            };
        }

        cases! {
            n1_m0 = 1, 0; n1_m1 = 1, 1;
            n2_m0 = 2, 0; n2_m1 = 2, 1; n2_m2 = 2, 2;
            n3_m0 = 3, 0; n3_m1 = 3, 1; n3_m2 = 3, 2; n3_m3 = 3, 3;
        }
    }

    #[kani::proof]
    #[kani::unwind(5)]
    fn global_indices() {
        let lens: [u32; 3] = kani::any();
        kani::assume(lens.iter().all(|&l| l <= 4));
        let a: (u32, u32) = kani::any();
        let b: (u32, u32) = kani::any();
        kani::assume(a.0 < 3 && b.0 < 3 && a.1 < lens[a.0 as usize] && b.1 < lens[b.0 as usize]);
        spec::global_indices_are_distinct(lens, a, b);
    }
}

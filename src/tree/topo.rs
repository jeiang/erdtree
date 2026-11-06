//! Pure algorithms over the tree topology. Nodes are `u32` indices into parallel arrays; the
//! root's parent is [`NONE`]. Nothing here performs I/O, allocates per node beyond the output
//! arrays, or recurses, so these functions are cheap on trees with millions of nodes and small
//! enough to verify exhaustively for bounded sizes with Kani (see `proofs` below).

use super::{
    id::{NONE, id, ix},
    psort::{self, Key, sort_keyed_seq},
};
use crate::par::{run_parts, split};
use std::cmp::Ordering;

/// Lists with more entries than this are sorted by [`psort::sort_keyed`] with all threads;
/// shorter ones by one thread each.
const BIG_RUN: usize = 1 << 15;

/// Child lists totalling at most this many entries are sorted on the calling thread.
const INLINE_MAX: usize = 1 << 14;

/// The range of the list that holds the children of `p`, given the end of every range.
#[inline]
const fn range_of(end: &[u32], p: u32) -> std::ops::Range<usize> {
    let start = if p == 0 { 0 } else { end[ix(p) - 1] };
    ix(start)..ix(end[ix(p)])
}

/// The nodes `start..end` all have the parent `parent`.
#[derive(Clone, Copy, Debug)]
pub struct Run {
    pub start: u32,
    pub end: u32,
    pub parent: u32,
}

#[cfg(any(test, kani))]
impl Run {
    /// The run of nodes `i..j` of parent `p`.
    fn new(p: u32, i: usize, j: usize) -> Self {
        Self {
            start: id(i),
            end: id(j),
            parent: p,
        }
    }
}

#[cfg(any(test, kani))]
/// Calls `visit(p, i, j)` for each maximal run `parent[i..j]` of equal parents `p` other than [`NONE`].
fn for_each_run(parent: &[u32], mut visit: impl FnMut(u32, usize, usize)) {
    let len = parent.len();
    let mut from = 0;

    while from < len {
        let owner = parent[from];
        let mut to = from + 1;

        while to < len && parent[to] == owner {
            to += 1;
        }

        if owner != NONE {
            visit(owner, from, to);
        }

        from = to;
    }
}

/// Children of every node: the children of `p` are `list[start(p)..end[p]]`, in increasing
/// index order until sorted with [`Children::sort_each`]. The children of one parent are
/// consecutive in `list`, different parents' ranges are disjoint, and the ranges follow each
/// other in the order of the parents, so a parent's range starts where the previous one ends.
///
/// The walk appends the entries of a directory in few runs of equal parents, so the lists are
/// built by copying those runs, not by placing every node.
pub struct Children {
    /// End of each parent's range in `list`.
    end: Vec<u32>,
    list: Vec<u32>,

    /// Parents with at least two children, in increasing order of start.
    multi: Vec<u32>,
}

impl Children {
    #[cfg(any(test, kani))]
    /// Groups nodes by parent. Nodes whose parent is [`NONE`] are roots and appear in no list.
    pub fn build(parent: &[u32]) -> Self {
        let mut runs = vec![];
        for_each_run(parent, |p, i, j| runs.push(Run::new(p, i, j)));
        Self::from_runs(parent.len(), &runs)
    }

    /// Groups `n` nodes by parent, given the runs of equal parents: each node with a parent is
    /// in exactly one run. A run's nodes are consecutive, and a parent may have several runs.
    pub fn from_runs(n: usize, runs: &[Run]) -> Self {
        let mut end = vec![0_u32; n];

        for run in runs {
            end[ix(run.parent)] += run.end - run.start;
        }

        // Turn the counts into the start of each range.
        let mut multi = vec![];
        let mut sum = 0_u32;
        for (p, slot) in end.iter_mut().enumerate() {
            let count = *slot;
            if count >= 2 {
                multi.push(id(p));
            }
            *slot = sum;
            sum += count;
        }

        // At most `n` children in total. Allocating `n` (a length known from the input size)
        // rather than `sum` keeps the allocation size concrete for Kani.
        let mut list = vec![0_u32; n];

        // Fill the ranges in index order; each `end` moves from the start to the end of its range.
        for run in runs {
            let at = ix(end[ix(run.parent)]);
            let len = ix(run.end - run.start);
            for (slot, child) in list[at..at + len].iter_mut().zip(run.start..run.end) {
                *slot = child;
            }
            end[ix(run.parent)] += run.end - run.start;
        }

        list.truncate(ix(sum));

        Self { end, list, multi }
    }

    #[inline]
    pub fn of(&self, p: u32) -> &[u32] {
        &self.list[range_of(&self.end, p)]
    }

    /// Calls `f(p, kids)` for each parent `p` in increasing order, with those of its children
    /// that sit at positions `lo..hi` of the lists (parents with none are skipped).
    fn for_each_list(&self, lo: usize, hi: usize, mut f: impl FnMut(u32, &[u32])) {
        let mut p = self.end.partition_point(|&e| ix(e) <= lo);

        while p < self.end.len() {
            let range = range_of(&self.end, id(p));
            if range.start >= hi {
                break;
            }
            if !range.is_empty() {
                f(id(p), &self.list[range.start.max(lo)..range.end.min(hi)]);
            }
            p += 1;
        }
    }

    /// Sorts the child list of every parent `p` with `wanted(p)`; other lists keep their order.
    /// Lists are ordered by `key`, and by `cmp` among equal keys, with up to `threads` threads.
    ///
    /// The key/comparator contract is that of [`psort::sort_keyed`].
    /// Small inputs are sorted on the calling thread; lists longer than the others are sorted
    /// by all threads together.
    pub fn sort_each(
        &mut self,
        threads: usize,
        wanted: impl Fn(u32) -> bool,
        key: impl Fn(u32) -> Key + Sync,
        cmp: impl Fn(u32, u32) -> Ordering + Sync,
    ) {
        self.sort_each_with(threads, INLINE_MAX, BIG_RUN, wanted, key, cmp);
    }

    /// [`Self::sort_each`] with the size thresholds as parameters: totals up to `inline_max`
    /// entries are sorted on the calling thread, lists longer than `big_run` one at a time by
    /// [`psort::sort_keyed`] with all threads.
    fn sort_each_with(
        &mut self,
        threads: usize,
        inline_max: usize,
        big_run: usize,
        wanted: impl Fn(u32) -> bool,
        key: impl Fn(u32) -> Key + Sync,
        cmp: impl Fn(u32, u32) -> Ordering + Sync,
    ) {
        let end = &self.end;
        let range = |p: u32| range_of(end, p);
        let len = |p: u32| range(p).len();
        let work: Vec<u32> = self.multi.iter().copied().filter(|&p| wanted(p)).collect();
        let total: usize = work.iter().map(|&p| len(p)).sum();
        let threads = threads.max(1);
        let (key, cmp) = (&key, &cmp);

        if threads == 1 || total <= inline_max {
            let mut buf = vec![];
            for &p in &work {
                sort_keyed_seq(&mut self.list[range(p)], &mut buf, key, cmp);
            }
            return;
        }

        let (big, small): (Vec<u32>, Vec<u32>) = work.iter().partition(|&&p| len(p) > big_run);
        let small_total: usize = small.iter().map(|&p| len(p)).sum();

        // Consecutive lists, in `list` order, of about equal total length per thread.
        let target = small_total.div_ceil(threads).max(1);
        let mut groups: Vec<&[u32]> = vec![];
        let (mut first, mut sum) = (0, 0);
        for (k, &p) in small.iter().enumerate() {
            sum += len(p);
            if sum >= target {
                groups.push(&small[first..=k]);
                first = k + 1;
                sum = 0;
            }
        }
        if first < small.len() {
            groups.push(&small[first..]);
        }

        let mut rest = self.list.as_mut_slice();
        let mut taken = 0;

        let mut jobs = vec![];
        for group in groups {
            let (lo, hi) = match (group.first(), group.last()) {
                (Some(&f), Some(&l)) => (range(f).start, range(l).end),
                _ => continue,
            };
            let (_, tail) = rest.split_at_mut(lo - taken);
            let (slice, tail) = tail.split_at_mut(hi - lo);
            rest = tail;
            taken = hi;
            jobs.push((group, lo, slice));
        }
        run_parts(jobs, |(group, lo, slice)| {
            let mut buf = vec![];
            for &p in group {
                let r = range(p);
                sort_keyed_seq(&mut slice[r.start - lo..r.end - lo], &mut buf, key, cmp);
            }
        });

        for &p in &big {
            psort::sort_keyed(&mut self.list[range(p)], threads, key, cmp);
        }
    }
}

/// Pre-order (parent before children, siblings in child-list order) of the nodes reachable from
/// `root` through `alive` nodes, down to depth `max_depth` (`depth` holds every node's distance
/// from the root, which is 0). Dead nodes and their subtrees are skipped, and so are the
/// children of nodes at `max_depth`.
pub fn preorder(
    children: &Children,
    root: u32,
    alive: &[bool],
    depth: &[u32],
    max_depth: u32,
) -> Vec<u32> {
    let mut order = Vec::with_capacity(alive.len());
    // Every node is pushed at most once (by its parent), so the stack never grows past this.
    let mut stack = Vec::with_capacity(alive.len());
    stack.push(root);

    while let Some(node) = stack.pop() {
        if !alive[ix(node)] {
            continue;
        }

        order.push(node);

        if depth[ix(node)] < max_depth {
            stack.extend(children.of(node).iter().rev().copied());
        }
    }

    order
}

/// Trees with at least this many nodes find their last children with several threads.
const PAR_LAST_MIN: usize = 1 << 18;

/// For every node in `order` (a pre-order), whether it is the last node among the alive children
/// of its parent. The root is its own last sibling. Other nodes may be marked too. Uses up to
/// `threads` threads when `order` is long.
pub fn last_alive_child(
    children: &Children,
    order: &[u32],
    alive: &[bool],
    threads: usize,
) -> Vec<bool> {
    last_alive_child_with(children, order, alive, threads, PAR_LAST_MIN)
}

/// [`last_alive_child`] with the size from which several threads are used as a parameter.
fn last_alive_child_with(
    children: &Children,
    order: &[u32],
    alive: &[bool],
    threads: usize,
    par_min: usize,
) -> Vec<bool> {
    let n = alive.len();
    let mut last = vec![false; n];
    let last_of = |p: u32| {
        children
            .of(p)
            .iter()
            .rev()
            .find(|&&c| alive[ix(c)])
            .copied()
    };

    if order.len() < n / 4 {
        // Few nodes are listed: look at those parents only.
        for child in order.iter().filter_map(|&p| last_of(p)) {
            last[ix(child)] = true;
        }
    } else {
        // Most nodes are listed: go over all the parents, a share per thread.
        let parts = if threads < 2 || n < par_min {
            1
        } else {
            threads
        };
        let part =
            |t: usize| -> Vec<u32> { split(n, parts, t).filter_map(|p| last_of(id(p))).collect() };

        let found = run_parts(0..parts, part);

        for &child in found.iter().flatten() {
            last[ix(child)] = true;
        }
    }

    if let Some(&root) = order.first() {
        last[ix(root)] = true;
    }

    last
}

/// Finds hard-link duplicates. `linked` lists the nodes whose inode has more than one link, with
/// that inode as key; among the nodes sharing a key only the least by `rank` (the first of
/// equals, in `linked` order) counts. Returns the nodes that do not count, in increasing order;
/// all other nodes count. `linked` is reordered. Sorting uses up to `threads` threads for long
/// lists, and `rank` runs once for each node beyond the first of its key.
pub fn dedupe<K: Ord + Copy + Send + Sync>(
    linked: &mut [(u32, K)],
    threads: usize,
    mut rank: impl FnMut(u32, u32) -> Ordering,
) -> Vec<u32> {
    let mut uncounted = vec![];

    // By key, then node: a total order, so the unstable sort has one result.
    psort::sort_by(linked, threads, |(a, ka), (b, kb)| {
        ka.cmp(kb).then_with(|| a.cmp(b))
    });

    // `best` is the least node so far in the current key group.
    let mut best = 0;

    for m in 1..linked.len() {
        if linked[m].1 != linked[m - 1].1 {
            best = m;
        } else if rank(linked[m].0, linked[best].0).is_lt() {
            uncounted.push(linked[best].0);
            best = m;
        } else {
            uncounted.push(linked[m].0);
        }
    }

    uncounted.sort_unstable();
    uncounted
}

/// Nodes ordered by depth (a counting sort, nodes of equal depth in index order), or `None`
/// when a depth is at least the number of nodes, which no tree has. If every node is deeper
/// than its parent, as with distances from the root, this is a parent-first order of all
/// nodes (see `spec::is_parent_first_order`), and cheaper than a pre-order.
pub fn by_depth(depth: &[u32]) -> Option<Vec<u32>> {
    let n = depth.len();
    let mut next = vec![0_u32; n + 1];

    for &d in depth {
        if ix(d) >= n {
            return None;
        }
        next[ix(d) + 1] += 1;
    }

    for i in 0..n {
        next[i + 1] += next[i];
    }

    let mut order = vec![0_u32; n];

    for (i, &d) in depth.iter().enumerate() {
        let slot = &mut next[ix(d)];
        order[ix(*slot)] = id(i);
        *slot += 1;
    }

    Some(order)
}

/// Trees with at least this many child-list entries are aggregated by several threads.
const PAR_AGG_MIN: usize = 1 << 17;

/// Directory sizes: each directory gets the sum of the sizes of its children that count (all but
/// the `uncounted` nodes, in increasing order), or `None` when that sum is zero; other nodes
/// keep their size. Only directories have children (`is_dir`) and they must start without a
/// size. `depth` holds every node's distance from the root. Sums saturate instead of
/// overflowing, so the order of the additions does not matter.
///
/// The sums over each directory's files need no particular order, so threads share them out by
/// position in the child lists. What is left is adding the sizes of directories with children to
/// their parents, deepest first, which takes one step per directory.
pub fn aggregate(
    children: &Children,
    parent: &[u32],
    depth: &[u32],
    is_dir: &[bool],
    uncounted: &[u32],
    size: &mut [Option<u64>],
    threads: usize,
) {
    aggregate_with(
        children,
        parent,
        depth,
        is_dir,
        uncounted,
        size,
        threads,
        PAR_AGG_MIN,
    );
}

/// [`aggregate`] with the threshold for using several threads as a parameter.
#[allow(clippy::too_many_arguments)] // The inputs are separate arrays; the threshold is for tests.
fn aggregate_with(
    children: &Children,
    parent: &[u32],
    depth: &[u32],
    is_dir: &[bool],
    uncounted: &[u32],
    size: &mut [Option<u64>],
    threads: usize,
    par_min: usize,
) {
    let mask = if uncounted.is_empty() {
        vec![]
    } else {
        let mut mask = vec![false; size.len()];
        for &i in uncounted {
            mask[ix(i)] = true;
        }
        mask
    };
    let counted = |i: usize| mask.is_empty() || !mask[i];

    // Sums of the sizes of the files in each list, for the list positions `lo..hi`. A directory
    // is listed once per part that holds some of its children.
    let total = children.list.len();
    let parts = if threads < 2 || total < par_min {
        1
    } else {
        threads
    };
    let sizes: &[Option<u64>] = size;
    let part = |t: usize| {
        let mut sums = vec![];
        let range = split(total, parts, t);
        children.for_each_list(range.start, range.end, |p, kids| {
            let mut sum = 0_u64;
            for &c in kids {
                let c = ix(c);
                if !is_dir[c]
                    && counted(c)
                    && let Some(s) = sizes[c].filter(|&s| s > 0)
                {
                    sum = sum.saturating_add(s);
                }
            }
            sums.push((p, sum));
        });
        sums
    };

    let sums: Vec<(u32, u64)> = run_parts(0..parts, part).into_iter().flatten().collect();

    // The directories with children, once each, in increasing order.
    let mut dirs: Vec<u32> = Vec::with_capacity(sums.len());
    for &(p, sum) in &sums {
        if dirs.last() != Some(&p) {
            dirs.push(p);
        }
        if sum > 0 {
            let total = &mut size[ix(p)];
            *total = Some(total.unwrap_or(0).saturating_add(sum));
        }
    }
    drop(sums);

    // Deepest first, so a directory is complete when it is added to its parent. A directory
    // has at most as many ancestors as there are directories, so the depths are small numbers.
    let deepest = dirs.iter().map(|&p| ix(depth[ix(p)])).max().unwrap_or(0);
    if deepest < dirs.len() {
        let mut next = vec![0_usize; deepest + 2];
        for &p in &dirs {
            next[deepest - ix(depth[ix(p)]) + 1] += 1;
        }
        for i in 0..=deepest {
            next[i + 1] += next[i];
        }
        let mut ordered = vec![0_u32; dirs.len()];
        for &p in &dirs {
            let slot = &mut next[deepest - ix(depth[ix(p)])];
            ordered[*slot] = p;
            *slot += 1;
        }
        dirs = ordered;
    } else {
        dirs.sort_unstable_by_key(|&p| std::cmp::Reverse(depth[ix(p)]));
    }

    for &d in &dirs {
        let i = ix(d);
        let up = parent[i];

        if up != NONE
            && counted(i)
            && let Some(s) = size[i].filter(|&s| s > 0)
        {
            let total = &mut size[ix(up)];
            *total = Some(total.unwrap_or(0).saturating_add(s));
        }
    }
}

/// `--prune`: kills every alive non-root directory without an alive non-directory descendant.
/// Every node must be alive, and `order` must list every node, each after its parent.
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
/// descendant of at least `threshold`. `order` must be a parent-first order of the alive nodes.
/// Returns the killed nodes.
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

/// Specifications of the functions above, checked two ways: exhaustively for every tree of up
/// to [`spec::MAX_TEST`] nodes by `cargo test`, and symbolically by Kani (`cargo kani`, see
/// `proofs`) for every tree of up to 3 nodes and every size, threshold, order and inode
/// grouping, except the pre-order and last-child properties, which Kani checks only for 1 node.
/// Both cover every root position.
#[cfg(any(test, kani))]
// The specifications index several per-node arrays by node number.
#[allow(clippy::needless_range_loop)]
mod spec {
    use super::*;

    /// Largest tree checked by the exhaustive tests.
    pub const MAX_TEST: usize = 6;

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
    /// `prune` (called while every node is alive) requires such an order; every pre-order
    /// of the whole tree is one, which `preorder_is_a_valid_preorder` checks.
    pub fn is_parent_first_order(parent: &[u32], root: u32, order: &[u32]) -> bool {
        is_parent_first_order_of(parent, root, &all_alive(parent.len()), order)
    }

    /// Whether `alive` is connected (the root is alive and so is the parent of every alive
    /// node) and `order` lists exactly the alive nodes, root first, each after its parent.
    /// `hide_small` gets such an order, filtered from the parent-first order after pruning
    /// and `--dirs-only`.
    pub fn is_parent_first_order_of(
        parent: &[u32],
        root: u32,
        alive: &[bool],
        order: &[u32],
    ) -> bool {
        let n = parent.len();

        if order.first() != Some(&root) {
            return false;
        }

        let mut pos = vec![usize::MAX; n];
        for (k, &node) in order.iter().enumerate() {
            if ix(node) >= n || !alive[ix(node)] || pos[ix(node)] != usize::MAX {
                return false;
            }
            pos[ix(node)] = k;
        }

        (0..n).all(|i| {
            !alive[i]
                || pos[i] != usize::MAX
                    && (parent[i] == NONE || alive[ix(parent[i])] && pos[ix(parent[i])] < pos[i])
        })
    }

    /// `sort_each` sorts every wanted child list by the comparator and keeps its members, and
    /// leaves the other lists alone, for any number of threads, whichever way the lists are
    /// split between threads: `inline_max` and `big_run` choose the sequential, grouped and
    /// whole-list paths. The comparator is descending order and the key a coarse
    /// version of it, so equal keys need the comparator to decide.
    #[cfg(test)]
    pub fn sort_each_sorts_every_list(
        parent: &[u32],
        threads: usize,
        inline_max: usize,
        big_run: usize,
    ) {
        let original = Children::build(parent);
        // Descending, as keys: a larger node has a smaller key, two nodes share a key.
        let key = |i: u32| (u64::from(!i / 2), 0);

        for wanted_mod in [1, 2] {
            let mut sorted = Children::build(parent);
            sorted.sort_each_with(
                threads,
                inline_max,
                big_run,
                |p| p % wanted_mod == 0,
                key,
                |a, b| b.cmp(&a),
            );

            for p in 0..parent.len() {
                let mut expected = original.of(id(p)).to_vec();
                if id(p).is_multiple_of(wanted_mod) {
                    expected.sort_by(|a, b| b.cmp(a));
                }
                assert_eq!(sorted.of(id(p)), expected.as_slice());
            }
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
        max_depth: u32,
    ) {
        let n = parent.len();
        let depth = depths(parent);
        let order = preorder(children, root, alive, &depth, max_depth);

        let mut pos = vec![usize::MAX; n];
        for (k, &node) in order.iter().enumerate() {
            assert_eq!(pos[ix(node)], usize::MAX, "listed twice");
            pos[ix(node)] = k;
        }

        assert_eq!(order[0], root);
        let anc = ancestors(parent);

        for i in 0..n {
            let reachable = (0..n).all(|a| !has_ancestor(&anc, i, a) || alive[a]);
            assert_eq!(pos[i] != usize::MAX, reachable && depth[i] <= max_depth);

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

        // The listed nodes are connected, and the pre-order is a parent-first order of them:
        // an order `hide_small` accepts. With every node listed it is also one `prune`
        // accepts; its proof assumes this. `aggregate` takes no order.
        let listed: Vec<bool> = (0..n).map(|i| pos[i] != usize::MAX).collect();
        assert!(is_parent_first_order_of(parent, root, &listed, &order));
        if listed.iter().all(|&l| l) {
            assert!(is_parent_first_order(parent, root, &order));
        }
    }

    /// Distance of every node from the root (which has depth 0), the `depth` column of a
    /// walked tree. Supports up to 16 nodes.
    pub fn depths(parent: &[u32]) -> Vec<u32> {
        ancestors(parent)
            .iter()
            .map(|a| a.count_ones().saturating_sub(1))
            .collect()
    }

    /// `by_depth` lists every node once, by increasing depth, which makes it a parent-first
    /// order of all nodes; and it refuses depths no tree has.
    pub fn by_depth_is_parent_first(parent: &[u32], root: u32) {
        let depth = depths(parent);
        let order = by_depth(&depth).unwrap();

        assert!(is_parent_first_order(parent, root, &order));
        assert!(order.windows(2).all(|w| depth[ix(w[0])] <= depth[ix(w[1])]));

        let mut too_deep = depth;
        too_deep[0] = id(parent.len());
        assert!(by_depth(&too_deep).is_none());
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
    /// is the last alive one, for any number of threads.
    pub fn last_child_is_unique(parent: &[u32], root: u32, alive: &[bool], threads: usize) {
        let children = Children::build(parent);
        let order = preorder(&children, root, alive, &depths(parent), u32::MAX);
        let last = last_alive_child_with(&children, &order, alive, threads, 0);
        assert!(last[ix(root)]);

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

    /// Each key group keeps exactly one counted member, the least by rank (the index, or with
    /// `reverse` the opposite); nodes without a key are counted.
    ///
    /// `linked` lists the nodes with a key, in increasing node order, as production builds it
    /// from its sparse list of multiply-linked inodes.
    pub fn dedupe_counts_each_inode_once(n: usize, linked: &[(u32, u8)], reverse: bool) {
        let mut key = vec![None; n];
        for &(i, k) in linked {
            key[ix(i)] = Some(k);
        }

        let mut work = linked.to_vec();
        let uncounted = dedupe(
            &mut work,
            1,
            |a, b| {
                if reverse { b.cmp(&a) } else { a.cmp(&b) }
            },
        );
        assert!(uncounted.windows(2).all(|w| w[0] < w[1]));
        let counted: Vec<bool> = (0..n).map(|i| !uncounted.contains(&id(i))).collect();

        for i in 0..n {
            match key[i] {
                None => assert!(counted[i]),
                Some(k) => {
                    let kept = if reverse {
                        (0..n).rev().find(|&j| key[j] == Some(k))
                    } else {
                        (0..n).find(|&j| key[j] == Some(k))
                    };
                    assert_eq!(counted[i], kept == Some(i));
                },
            }
        }
    }

    /// A directory's size is the sum of the sizes of the files below it that reach it through
    /// counted nodes only, saturating at `u64::MAX`, or `None` when that sum is zero; files keep
    /// their sizes. Holds for any number of threads, with `par_min` choosing whether the sums
    /// over the child lists are shared out. `files` has no size for directories.
    pub fn aggregate_sums_counted_files(
        parent: &[u32],
        counted: &[bool],
        is_dir: &[bool],
        files: &[Option<u64>],
        threads: usize,
        par_min: usize,
    ) {
        let n = parent.len();
        let mut size = files.to_vec();
        let uncounted: Vec<u32> = (0..n).filter(|&i| !counted[i]).map(id).collect();
        aggregate_with(
            &Children::build(parent),
            parent,
            &depths(parent),
            is_dir,
            &uncounted,
            &mut size,
            threads,
            par_min,
        );
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

    /// `--min-size`, starting from the nodes `alive_before`: keeps the root and exactly the
    /// alive nodes with an alive node of at least `threshold` in their subtree, reports every
    /// other alive node as hidden, and leaves dead nodes dead. Holds for every `order` accepted
    /// by [`is_parent_first_order_of`].
    pub fn hide_small_keeps_large_and_their_ancestors(
        parent: &[u32],
        root: u32,
        alive_before: &[bool],
        order: &[u32],
        size: &[Option<u64>],
        threshold: u64,
    ) {
        let n = parent.len();
        let mut alive = alive_before.to_vec();
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
            if !alive_before[i] {
                assert!(!alive[i]);
                assert_eq!(reported[i], 0);
                continue;
            }
            let large_below = (0..n).any(|d| {
                alive_before[d] && size[d].unwrap_or(0) >= threshold && has_ancestor(&anc, d, i)
            });
            assert_eq!(alive[i], large_below);
            assert_eq!(reported[i], usize::from(!large_below));
        }
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
        use std::sync::atomic::{AtomicUsize, Ordering};

        // One thread per core takes the next tree: a single thread needs about a minute in
        // debug builds.
        let all: Vec<_> = (1..=spec::MAX_TEST).flat_map(trees).collect();
        let next = AtomicUsize::new(0);
        let threads = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
        std::thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(|| {
                    while let Some((parent, root)) = all.get(next.fetch_add(1, Ordering::Relaxed)) {
                        check_small_tree(parent, *root);
                    }
                });
            }
        });
    }

    /// Checks every property of `topo` on one tree, with every alive set, directory set and
    /// counted set.
    fn check_small_tree(parent: &[u32], root: u32) {
        use super::Children;

        let n = parent.len();
        let r = super::ix(root);
        spec::children_partition_nodes(parent);

        // Child lists as built, and sorted (descending) as production sorts them
        // before the pre-order.
        let built = Children::build(parent);
        let mut sorted = Children::build(parent);
        sorted.sort_each(2, |_| true, |i| (u64::from(!i), 0), |a, b| b.cmp(&a));
        let depth = spec::depths(parent);

        spec::by_depth_is_parent_first(parent, root);

        // Masks with the root set: alive flags for the pre-order, with and without a
        // depth limit.
        for mask in (0..1 << n).filter(|m| m >> r & 1 == 1) {
            let alive = bits(mask, n);
            for max_depth in (0..=u32::try_from(n).unwrap()).chain([u32::MAX]) {
                spec::preorder_is_a_valid_preorder(parent, &built, root, &alive, max_depth);
                spec::preorder_is_a_valid_preorder(parent, &sorted, root, &alive, max_depth);
            }
            // Starting threads is slow: use them only for small trees.
            for threads in [1, 3] {
                if threads > 1 && n > 4 {
                    continue;
                }
                spec::last_child_is_unique(parent, root, &alive, threads);
            }
        }

        // Two different parent-first orders: the pre-order production uses, and
        // breadth-first.
        let everyone = vec![true; n];
        let orders = [
            super::preorder(&sorted, root, &everyone, &depth, u32::MAX),
            breadth_first(&built, root),
            super::by_depth(&depth).unwrap(),
        ];
        for order in &orders {
            assert!(spec::is_parent_first_order(parent, root, order));
        }

        for mask in 0..1 << n {
            let is_dir = &bits(mask, n);
            if !spec::only_dirs_have_children(parent, is_dir) {
                continue;
            }

            for order in &orders {
                spec::prune_removes_exactly_empty_dirs(parent, root, order, is_dir);
            }
            spec::dirs_only_keeps_connected_dirs(parent, root, is_dir);

            // Distinct powers of two make every subset sum distinct; values near
            // u64::MAX exercise saturation.
            let small: Vec<Option<u64>> = (0..n).map(|i| (!is_dir[i]).then_some(1 << i)).collect();
            let huge: Vec<Option<u64>> = (0..n)
                .map(|i| (!is_dir[i]).then_some(u64::MAX - (1 << i)))
                .collect();

            // Whether the root is counted does not matter (it has no parent), so it
            // stays counted.
            for counted in (0..1 << n).filter(|m| m >> r & 1 == 1) {
                let counted = bits(counted, n);
                // Starting threads is slow: share the sums out only for small trees.
                let few_uncounted = n <= 4;
                for files in [&small, &huge] {
                    for (threads, par_min) in [(1, usize::MAX), (3, 0)] {
                        if threads > 1 && !few_uncounted {
                            continue;
                        }
                        spec::aggregate_sums_counted_files(
                            parent, &counted, is_dir, files, threads, par_min,
                        );
                    }
                }
            }
        }

        // `--min-size` runs on whatever `--prune`, patterns and `--dirs-only` left
        // alive: every connected set containing the root, with its pre-order (and, for
        // the whole tree, also breadth-first). Sizes are 2^i, all set or with the odd
        // nodes unsized; thresholds sit at every distinct boundary.
        let all_sized: Vec<Option<u64>> = (0..n).map(|i| Some(1 << i)).collect();
        let some_unsized: Vec<Option<u64>> =
            (0..n).map(|i| (i % 2 == 0).then_some(1 << i)).collect();
        for mask in (0..1 << n).filter(|m| m >> r & 1 == 1) {
            let alive = bits(mask, n);
            let connected =
                (0..n).all(|i| !alive[i] || parent[i] == NONE || alive[super::ix(parent[i])]);
            if !connected {
                continue;
            }

            let mut alive_orders = vec![super::preorder(&sorted, root, &alive, &depth, u32::MAX)];
            if alive.iter().all(|&a| a) {
                alive_orders.push(breadth_first(&built, root));
                alive_orders.push(super::by_depth(&depth).unwrap());
            }

            for order in &alive_orders {
                assert!(spec::is_parent_first_order_of(parent, root, &alive, order));
                for threshold in (0..=n).map(|i| 1_u64 << i).chain([0]) {
                    for sizes in [&all_sized, &some_unsized] {
                        spec::hide_small_keeps_large_and_their_ancestors(
                            parent, root, &alive, order, sizes, threshold,
                        );
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
                    // Sequential, whole lists in parallel, and lists grouped per thread.
                    for (inline_max, big_run) in [(usize::MAX, usize::MAX), (0, 1), (0, usize::MAX)]
                    {
                        spec::sort_each_sorts_every_list(&parent, threads, inline_max, big_run);
                    }
                }
            }
        }
    }

    /// `Children::build` lists the children of a parent whether they are consecutive or not.
    #[test]
    fn build_handles_runs_and_interleaving() {
        let runs = [NONE, 0, 0, 0, 1, 1, 4, 4];
        let mixed = [NONE, 0, 1, 0, 1, 0, 4, 4];

        for parent in [runs, mixed] {
            let children = super::Children::build(&parent);
            for p in 0..parent.len() {
                let expected: Vec<u32> = (0..parent.len())
                    .filter(|&c| parent[c] == u32::try_from(p).unwrap())
                    .map(|c| u32::try_from(c).unwrap())
                    .collect();
                assert_eq!(children.of(u32::try_from(p).unwrap()), expected.as_slice());
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
                for reverse in [false, true] {
                    spec::dedupe_counts_each_inode_once(n, &linked, reverse);
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
        spec::preorder_is_a_valid_preorder(&parent, &children, root, &alive, kani::any());
    }

    fn by_depth_is_parent_first<const N: usize>() {
        let (parent, root) = any_tree::<N>();
        spec::by_depth_is_parent_first(&parent, root);
    }

    fn last_child_is_unique<const N: usize>() {
        let (parent, root) = any_tree::<N>();
        let alive: [bool; N] = kani::any();
        kani::assume(alive[ix(root)]);
        spec::last_child_is_unique(&parent, root, &alive, 1);
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
        spec::dedupe_counts_each_inode_once(N, &linked, kani::any());
    }

    /// Any order of all nodes with each parent first (what `prune` requires), so
    /// these harnesses need not model `Children::build` and `preorder` and hold for every such
    /// order. That `preorder` produces one is asserted by the pre-order spec, which the
    /// exhaustive tests check up to `spec::MAX_TEST` nodes and Kani only for 1 node.
    fn any_parent_first_order<const N: usize>(parent: &[u32; N], root: u32) -> [u32; N] {
        let order: [u8; N] = kani::any();
        let order = order.map(u32::from);
        kani::assume(spec::is_parent_first_order(parent, root, &order));
        order
    }

    fn aggregate_sums_counted_files<const N: usize>() {
        let (parent, _) = any_tree::<N>();
        let counted: [bool; N] = kani::any();
        let is_dir: [bool; N] = kani::any();
        let files: [Option<u64>; N] = kani::any();
        kani::assume(spec::only_dirs_have_children(&parent, &is_dir));
        for i in 0..N {
            // Directories start without a size; files may have any size.
            kani::assume(!is_dir[i] || files[i].is_none());
        }
        spec::aggregate_sums_counted_files(&parent, &counted, &is_dir, &files, 1, usize::MAX);
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

    /// `hide_small` on `N` nodes of which `K` are alive (a connected set containing the root),
    /// with any parent-first order of the alive nodes. The harnesses for `K` from 1 to `N`
    /// together cover every alive set, and the concrete order length keeps CBMC's problem small.
    fn hide_small_keeps_large_and_their_ancestors<const N: usize, const K: usize>() {
        let (parent, root) = any_tree::<N>();
        let alive: [bool; N] = kani::any();
        kani::assume(alive.iter().filter(|&&a| a).count() == K);
        let order: [u8; K] = kani::any();
        let order = order.map(u32::from);
        kani::assume(spec::is_parent_first_order_of(
            &parent, root, &alive, &order,
        ));
        let size: [Option<u64>; N] = kani::any();
        spec::hide_small_keeps_large_and_their_ancestors(
            &parent,
            root,
            &alive,
            &order,
            &size,
            kani::any(),
        );
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
        by_depth: by_depth_is_parent_first, [n1 = 1, unwind 4; n2 = 2, unwind 5; n3 = 3, unwind 6];
        aggregate: aggregate_sums_counted_files, [n1 = 1, unwind 3; n2 = 2, unwind 4; n3 = 3, unwind 5];
        prune: prune_removes_exactly_empty_dirs, [n1 = 1, unwind 3; n2 = 2, unwind 4; n3 = 3, unwind 5];
        dirs_only: dirs_only_keeps_connected_dirs, [n1 = 1, unwind 3; n2 = 2, unwind 4; n3 = 3, unwind 5];
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

    /// `hide_small` for `N` nodes of which `K` are alive.
    mod hide_small {
        use super::*;

        macro_rules! cases {
            ($($name:ident = $n:literal, $k:literal;)*) => {
                $(
                    #[kani::proof]
                    #[kani::unwind(5)]
                    fn $name() {
                        hide_small_keeps_large_and_their_ancestors::<$n, $k>();
                    }
                )*
            };
        }

        cases! {
            n1_k1 = 1, 1;
            n2_k1 = 2, 1; n2_k2 = 2, 2;
            n3_k1 = 3, 1; n3_k2 = 3, 2; n3_k3 = 3, 3;
        }
    }
}

//! Parallel sort of `Copy` items: sorted chunks (one thread each) merged pairwise, each merge
//! shared out between threads.

use std::{cmp::Ordering, mem::MaybeUninit};

/// A packed sort key: compared as the pair `(hi, lo)`.
pub type Key = (u64, u64);

/// Sorts of at least this many items use several threads.
pub(super) const PAR_SORT_MIN: usize = 1 << 15;

/// A sort key with the node it belongs to.
#[derive(Clone, Copy, Default)]
pub(super) struct Item {
    hi: u64,
    lo: u64,
    node: u32,
}

/// Orders items by key and, only when the keys are equal, by `cmp` on the nodes.
#[inline]
fn item_cmp(a: &Item, b: &Item, cmp: &impl Fn(u32, u32) -> Ordering) -> Ordering {
    a.hi.cmp(&b.hi)
        .then_with(|| a.lo.cmp(&b.lo))
        .then_with(|| cmp(a.node, b.node))
}

/// Calling-thread version of [`sort_keyed`], reusing `buf`.
pub(super) fn sort_keyed_seq(
    ids: &mut [u32],
    buf: &mut Vec<Item>,
    key: &impl Fn(u32) -> Key,
    cmp: &impl Fn(u32, u32) -> Ordering,
) {
    buf.clear();
    buf.extend(ids.iter().map(|&node| {
        let (hi, lo) = key(node);
        Item { hi, lo, node }
    }));
    buf.sort_unstable_by(|a, b| item_cmp(a, b, cmp));

    for (slot, item) in ids.iter_mut().zip(buf.iter()) {
        *slot = item.node;
    }
}

/// Like [`sort_keyed_seq`] with up to `threads` threads when `ids` has at least `par_min`
/// entries: keys are computed and chunks sorted in parallel, then merged.
pub(super) fn sort_keyed_par(
    ids: &mut [u32],
    threads: usize,
    par_min: usize,
    key: &(impl Fn(u32) -> Key + Sync),
    cmp: &(impl Fn(u32, u32) -> Ordering + Sync),
) {
    if threads < 2 || ids.len() < par_min.max(2) {
        sort_keyed_seq(ids, &mut Vec::new(), key, cmp);
        return;
    }

    let len = ids.len();
    let width = len.div_ceil(threads);
    let mut items: Vec<Item> = Vec::with_capacity(len);
    let item_cmp = &|a: &Item, b: &Item| item_cmp(a, b, cmp);

    // Each thread keys and sorts its share of `ids`, written straight into the new memory.
    crate::par::run_parts(
        items
            .spare_capacity_mut()
            .chunks_mut(width)
            .zip(ids.chunks(width)),
        |(out, ids)| {
            for (slot, &node) in out.iter_mut().zip(ids) {
                let (hi, lo) = key(node);
                slot.write(Item { hi, lo, node });
            }
            // SAFETY: every slot of `out` was just written, and `MaybeUninit<Item>` has the
            // layout of `Item`.
            let out = unsafe { &mut *(std::ptr::from_mut(out) as *mut [Item]) };
            out.sort_unstable_by(item_cmp);
        },
    );

    // SAFETY: the threads wrote the first `len` slots.
    unsafe { items.set_len(len) };

    merge_runs(&mut items, width, threads, item_cmp);

    for (slot, item) in ids.iter_mut().zip(&items) {
        *slot = item.node;
    }
}

/// Sorts `ids` by `key`, then `cmp` for equal keys, with several threads for long inputs.
/// `cmp` must be a total order and keys must agree with it: `key(a) < key(b)` implies
/// `cmp(a, b) == Ordering::Less`. Equal keys carry no information.
pub fn sort_keyed(
    ids: &mut [u32],
    threads: usize,
    key: &(impl Fn(u32) -> Key + Sync),
    cmp: &(impl Fn(u32, u32) -> Ordering + Sync),
) {
    sort_keyed_par(ids, threads, PAR_SORT_MIN, key, cmp);
}

/// Sorts `items` by `cmp` (an unstable sort: equal items may swap) with up to `threads` threads.
/// Inputs shorter than `min_len`, and calls with one thread, sort on the calling thread without
/// spawning anything.
pub fn sort_by<T, F>(items: &mut [T], threads: usize, min_len: usize, cmp: F)
where
    T: Copy + Send + Sync,
    F: Fn(&T, &T) -> Ordering + Sync,
{
    let len = items.len();
    let threads = threads.min(len / 2);

    if threads < 2 || len < min_len {
        items.sort_unstable_by(cmp);
        return;
    }

    let width = len.div_ceil(threads);
    let cmp = &cmp;

    crate::par::run_parts(items.chunks_mut(width), |chunk| chunk.sort_unstable_by(cmp));

    merge_runs(items, width, threads, cmp);
}

/// Merges the sorted runs of `items`, each `run` long but the last, into one sorted slice, with
/// up to `threads` threads. Equal items from an earlier run come first.
pub fn merge_runs<T, F>(items: &mut [T], run: usize, threads: usize, cmp: &F)
where
    T: Copy + Send + Sync,
    F: Fn(&T, &T) -> Ordering + Sync,
{
    let len = items.len();
    let mut run = run.max(1);

    if run >= len {
        return;
    }

    // Passes ping-pong between `items` and a scratch buffer, which is written before it is read.
    let mut scratch: Vec<MaybeUninit<T>> = Vec::with_capacity(len);
    // SAFETY: `MaybeUninit` slots need no initialization.
    unsafe { scratch.set_len(len) };
    let mut in_scratch = false;

    while run < len {
        // SAFETY: `MaybeUninit<T>` has the layout of `T`, and overwriting a `Copy` item with a
        // new one needs no drop. The scratch buffer holds initialized items when `in_scratch`:
        // the previous pass wrote all of it.
        let (src, dst) = unsafe {
            let items = std::ptr::from_mut::<[T]>(items);
            let scratch = std::ptr::from_mut::<[MaybeUninit<T>]>(scratch.as_mut_slice());
            if in_scratch {
                (
                    &*(scratch as *const [T]),
                    &mut *(items as *mut [MaybeUninit<T>]),
                )
            } else {
                (&*items, &mut *scratch)
            }
        };
        merge_pass(src, dst, run, threads, cmp);
        in_scratch = !in_scratch;
        run *= 2;
    }

    if in_scratch {
        // SAFETY: the last pass wrote all of the scratch buffer.
        items.copy_from_slice(unsafe {
            &*(std::ptr::from_ref::<[MaybeUninit<T>]>(scratch.as_slice()) as *const [T])
        });
    }
}

/// Merges each pair of adjacent `run`-long sorted runs of `src` into `dst`, sharing the work
/// between up to `threads` threads: a pair is cut into pieces that merge independently.
fn merge_pass<T, F>(src: &[T], dst: &mut [MaybeUninit<T>], run: usize, threads: usize, cmp: &F)
where
    T: Copy + Send + Sync,
    F: Fn(&T, &T) -> Ordering + Sync,
{
    let pairs = src.len().div_ceil(2 * run);
    let pieces = threads.div_ceil(pairs).max(1);
    let mut jobs = vec![];

    for (pair, out) in src.chunks(2 * run).zip(dst.chunks_mut(2 * run)) {
        let (left, right) = pair.split_at(run.min(pair.len()));
        let cuts: Vec<(usize, usize)> = (0..=pieces)
            .map(|k| {
                co_rank(
                    left,
                    right,
                    crate::par::split(pair.len(), pieces, k).start,
                    cmp,
                )
            })
            .collect();
        let mut rest = out;

        for w in cuts.windows(2) {
            let (a, b) = (w[0], w[1]);
            let (piece, tail) = rest.split_at_mut(b.0 - a.0 + b.1 - a.1);
            rest = tail;
            jobs.push((&left[a.0..b.0], &right[a.1..b.1], piece));
        }
    }

    crate::par::run_parts(jobs, |(left, right, out)| {
        merge_piece(left, right, out, cmp);
    });
}

/// How many of the first `d` items of the merge of sorted `left` and `right` come from each,
/// where equal items of `left` come first.
fn co_rank<T, F: Fn(&T, &T) -> Ordering>(
    left: &[T],
    right: &[T],
    d: usize,
    cmp: &F,
) -> (usize, usize) {
    let (mut lo, mut hi) = (d.saturating_sub(right.len()), d.min(left.len()));

    while lo < hi {
        let mid = lo + (hi - lo) / 2;

        if cmp(&right[d - mid - 1], &left[mid]).is_lt() {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }

    (lo, d - lo)
}

/// Merges the sorted `left` and `right` into `out`, which is as long as both.
fn merge_piece<T: Copy, F: Fn(&T, &T) -> Ordering>(
    left: &[T],
    right: &[T],
    out: &mut [MaybeUninit<T>],
    cmp: &F,
) {
    let (mut l, mut r) = (0, 0);

    for slot in out.iter_mut() {
        let take_right = l == left.len() || r < right.len() && cmp(&right[r], &left[l]).is_lt();

        if take_right {
            slot.write(right[r]);
            r += 1;
        } else {
            slot.write(left[l]);
            l += 1;
        }
    }
}

#[cfg(test)]
mod test {
    use super::sort_by;

    /// Every length and thread count gives the same result as the standard sort, with the
    /// parallel path forced by `min_len == 0`.
    #[test]
    fn matches_std_sort() {
        let mut seed = 0x9e37_79b9_u64;
        let mut next = || {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            seed >> 33
        };

        for len in [0, 1, 2, 3, 5, 8, 31, 100, 1001] {
            for threads in 1..=5 {
                // Keys collide on purpose; the payload makes every item distinct.
                let mut items: Vec<(u64, u64)> = (0..len).map(|i| (next() % 7, i)).collect();
                let mut expected = items.clone();
                expected.sort_unstable();
                sort_by(&mut items, threads, 0, Ord::cmp);
                assert_eq!(items, expected, "len {len}, threads {threads}");
            }
        }
    }

    #[test]
    fn keyed_sort_breaks_collisions_with_comparator() {
        for len in [0_u32, 1, 2, 3, 31, 1001] {
            let cmp = |a: u32, b: u32| (a / 7, a % 7).cmp(&(b / 7, b % 7));
            let expected: Vec<u32> = (0..len).collect();
            for threads in 1..=5 {
                let mut ids: Vec<u32> = (0..len).rev().collect();
                super::sort_keyed_par(&mut ids, threads, 0, &|i| (u64::from(i / 7), 0), &cmp);
                assert_eq!(ids, expected, "len {len}, threads {threads}");
            }
        }
    }
}

//! Parallel sort of `Copy` items: sorted chunks (one thread each) merged pairwise, each merge
//! shared out between threads.

use std::{cmp::Ordering, mem::MaybeUninit};

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

    std::thread::scope(|s| {
        for chunk in items.chunks_mut(width) {
            s.spawn(move || chunk.sort_unstable_by(cmp));
        }
    });

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
            .map(|k| co_rank(left, right, pair.len() * k / pieces, cmp))
            .collect();
        let mut rest = out;

        for w in cuts.windows(2) {
            let (a, b) = (w[0], w[1]);
            let (piece, tail) = rest.split_at_mut(b.0 - a.0 + b.1 - a.1);
            rest = tail;
            jobs.push((&left[a.0..b.0], &right[a.1..b.1], piece));
        }
    }

    if jobs.len() == 1 {
        for (left, right, out) in jobs {
            merge_piece(left, right, out, cmp);
        }
        return;
    }

    std::thread::scope(|s| {
        let mut jobs = jobs.into_iter();
        let first = jobs.next();

        for (left, right, out) in jobs {
            s.spawn(move || merge_piece(left, right, out, cmp));
        }

        if let Some((left, right, out)) = first {
            merge_piece(left, right, out, cmp);
        }
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
}

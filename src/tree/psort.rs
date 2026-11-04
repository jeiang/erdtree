//! Parallel sort of `Copy` items: sorted chunks (one thread each) merged pairwise.

use std::cmp::Ordering;

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

    // Merge pairs of sorted runs, ping-ponging between `items` and a scratch copy.
    let mut scratch = items.to_vec();
    let mut from: &mut [T] = items;
    let mut to: &mut [T] = &mut scratch;
    let mut run = width;
    let mut in_scratch = false;

    while run < len {
        merge_pass(from, to, run, cmp);
        std::mem::swap(&mut from, &mut to);
        in_scratch = !in_scratch;
        run *= 2;
    }

    if in_scratch {
        to.copy_from_slice(from);
    }
}

/// Merges each pair of adjacent `run`-long sorted runs of `src` into `dst`; pairs are merged in
/// parallel when there are several.
fn merge_pass<T, F>(src: &[T], dst: &mut [T], run: usize, cmp: &F)
where
    T: Copy + Send + Sync,
    F: Fn(&T, &T) -> Ordering + Sync,
{
    let pairs = src.chunks(2 * run).zip(dst.chunks_mut(2 * run));

    if src.len() <= 2 * run {
        for (a, out) in pairs {
            merge_pair(a, out, run, cmp);
        }
        return;
    }

    std::thread::scope(|s| {
        for (a, out) in pairs {
            s.spawn(move || merge_pair(a, out, run, cmp));
        }
    });
}

/// Merges the sorted halves `pair[..run]` and `pair[run..]` into `out`.
fn merge_pair<T: Copy, F: Fn(&T, &T) -> Ordering>(pair: &[T], out: &mut [T], run: usize, cmp: &F) {
    if pair.len() <= run {
        out.copy_from_slice(pair);
        return;
    }

    let (left, right) = pair.split_at(run);
    let (mut l, mut r) = (0, 0);

    for slot in out.iter_mut() {
        let take_right = l == left.len() || r < right.len() && cmp(&right[r], &left[l]).is_lt();

        if take_right {
            *slot = right[r];
            r += 1;
        } else {
            *slot = left[l];
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

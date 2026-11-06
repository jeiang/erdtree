//! Scoped parallel work, with the first part on the calling thread.

use std::ops::Range;

/// Balanced contiguous partition `k` of `len` items across `parts` parts.
pub const fn split(len: usize, parts: usize, k: usize) -> Range<usize> {
    len * k / parts..len * (k + 1) / parts
}

/// Runs every part, starting all other threads before the calling thread's part, and returns
/// results in order. No parts performs no work; a single part starts no thread.
pub fn run_parts<J: Send, R: Send>(
    parts: impl IntoIterator<Item = J>,
    part: impl Fn(J) -> R + Sync,
) -> Vec<R> {
    let mut parts = parts.into_iter();
    let Some(first) = parts.next() else {
        return vec![];
    };
    std::thread::scope(|s| {
        let part = &part;
        let jobs: Vec<_> = parts.map(|job| s.spawn(move || part(job))).collect();
        let mut all = vec![part(first)];
        for job in jobs {
            all.push(job.join().unwrap_or_else(|e| std::panic::resume_unwind(e)));
        }
        all
    })
}

#[cfg(test)]
mod test {
    use super::{run_parts, split};

    #[test]
    fn splits_cover_every_item_in_order() {
        for len in 0..32 {
            for parts in 1..=8 {
                let ranges: Vec<_> = (0..parts).map(|k| split(len, parts, k)).collect();
                assert_eq!(
                    ranges.iter().cloned().flatten().collect::<Vec<_>>(),
                    (0..len).collect::<Vec<_>>()
                );
                let shortest = ranges.iter().map(std::ops::Range::len).min().unwrap();
                let longest = ranges.iter().map(std::ops::Range::len).max().unwrap();
                assert!(longest - shortest <= 1);
            }
        }
    }

    #[test]
    fn parts_keep_order_and_first_uses_caller() {
        let caller = std::thread::current().id();
        assert_eq!(run_parts(0..0, |k| k), [] as [i32; 0]);
        for parts in 1..=4 {
            let results = run_parts(0..parts, |k| (k, std::thread::current().id()));
            assert_eq!(results[0].1, caller);
            for (k, &(part, thread)) in results.iter().enumerate() {
                assert_eq!(part, k);
                if k != 0 {
                    assert_ne!(thread, caller);
                }
            }
        }
    }
}

use crate::{
    context::{Context, layout},
    sync::unpoisoned,
    tree::Tree,
};
use row::{RowFmt, Scratch};
use std::{
    collections::BTreeMap,
    io::{self, Write},
    ops::Range,
    sync::{Condvar, Mutex},
};

/// Truncation of ANSI escaped rows.
mod ansi;
/// Directory paths of the flat layouts.
pub mod paths;

/// Box-drawing prefixes of tree rows.
pub mod prefix;

/// Concerned with how to write a single row of the output.
pub mod row;

/// Escape sequences of styles, and the styles of the long columns.
pub mod theme;

/// Concerned with how to construct the long output.
#[cfg(unix)]
pub mod long;

/// Which rows a layout prints, in order.
enum Rows {
    /// The shown nodes of the tree in pre-order with the root on top if `inverted`, else in
    /// reverse pre-order with the root at the bottom, with the prefixes worked out from each
    /// row's ancestors.
    Tree {
        inverted: bool,
        glyphs: prefix::Glyphs,
    },

    /// Flat rows of the nodes `ids`, each the path of its parent directory in `paths` and its
    /// name.
    Flat {
        ids: Vec<u32>,
        paths: paths::DirPaths,
    },
}

impl Rows {
    /// The rows of `tree` in the layout of `ctx`.
    fn new(tree: &Tree, ctx: &Context) -> Self {
        let flat = |ids| Self::Flat {
            ids,
            paths: paths::DirPaths::new(tree, ctx.level(), ctx.threads),
        };
        let tree_rows = |inverted| Self::Tree {
            inverted,
            glyphs: prefix::Glyphs::new(ctx.follow),
        };

        match ctx.layout {
            layout::Type::Regular => tree_rows(false),
            layout::Type::Inverted => tree_rows(true),
            layout::Type::Iflat => flat(tree.flat_order(ctx)),
            layout::Type::Flat => {
                let mut ids = tree.flat_order(ctx);
                ids.reverse();
                flat(ids)
            },
        }
    }

    /// Number of rows.
    fn len(&self, tree: &Tree) -> usize {
        match self {
            Self::Tree { .. } => tree.order().len(),
            Self::Flat { ids, .. } => ids.len(),
        }
    }

    /// Appends the rows at `positions`.
    fn write_range(
        &self,
        fmt: &RowFmt<'_>,
        tree: &Tree,
        scratch: &mut Scratch,
        positions: Range<usize>,
        out: &mut Vec<u8>,
    ) {
        match self {
            Self::Tree { inverted, glyphs } => {
                let order = tree.order();
                let last = order.len().saturating_sub(1);

                for k in positions {
                    let id = order[if *inverted { k } else { last - k }];
                    fmt.tree_row(scratch, id, glyphs, *inverted, out);
                }
            },
            Self::Flat { ids, paths } => {
                for k in positions {
                    prefetch_flat(ids, paths, tree, k);
                    fmt.flat_row(scratch, paths, ids[k], out);
                }
            },
        }
    }
}

/// Requests the data of the flat rows after `k` in stages, the rows further ahead first, so that
/// the dependent loads of a flat row (name, parent, parent's path) overlap.
#[inline]
fn prefetch_flat(ids: &[u32], paths: &paths::DirPaths, tree: &Tree, k: usize) {
    const AHEAD: usize = 16;

    let at = |d: usize| ids.get(k + d).copied();

    if let Some(id) = at(AHEAD) {
        tree.prefetch_cols(id);
    }
    if let Some(id) = at(AHEAD / 2) {
        tree.prefetch_name(id);
        paths.prefetch_span(tree.parent(id));
    }
    if let Some(id) = at(AHEAD / 4) {
        paths.prefetch_prefix(tree.parent(id));
    }
}

/// Positions formatted per unit of work. A chunk of rows is a few hundred KB of output, which
/// stays in the cache until it is written.
const CHUNK: usize = 4096;

/// Up to this many positions are formatted on the calling thread; spawning workers costs more
/// than formatting a few thousand rows.
const INLINE_MAX: usize = 2 * CHUNK;

/// Writes the rows of `tree` in the layout of `ctx`, one per line. Rows are formatted in
/// parallel and written in order.
pub fn write(tree: &Tree, ctx: &Context, out: &mut impl Write) -> io::Result<()> {
    let rows = Rows::new(tree, ctx);
    let fmt = RowFmt::new(tree, ctx);

    write_chunked(
        rows.len(tree),
        ctx.threads,
        CHUNK,
        INLINE_MAX,
        &|scratch: &mut Scratch, positions, buf: &mut Vec<u8>| {
            rows.write_range(&fmt, tree, scratch, positions, buf);
        },
        out,
    )
}

/// What the workers and the writer of [`write_chunked`] share.
struct Shared {
    state: Mutex<Pool>,

    /// Signals a returned buffer, or the end.
    free_cv: Condvar,

    /// Signals a finished chunk, or the end.
    done_cv: Condvar,
}

/// Buffers in circulation and chunks in progress.
struct Pool {
    free: Vec<Vec<u8>>,
    done: BTreeMap<usize, Vec<u8>>,

    /// The next chunk to hand out.
    next: usize,
    stop: bool,
}

/// Ends the run if a worker panics, so that the writer does not wait for its chunk forever; the
/// panic itself surfaces when the thread scope ends.
struct PanicGuard<'a>(&'a Shared);

impl Drop for PanicGuard<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            unpoisoned(self.0.state.lock()).stop = true;
            self.0.free_cv.notify_all();
            self.0.done_cv.notify_all();
        }
    }
}

/// Formats the positions `0..n` in chunks of `chunk` with `format` and writes the chunks to `out`
/// in order.
///
/// With one thread, or `n <= inline_max`, everything happens on the calling thread. Otherwise
/// `threads` workers, started once, claim chunks in increasing order; the calling thread writes
/// finished chunks in order, so the write overlaps the formatting of later chunks. A worker takes
/// a buffer before it claims a chunk, so the oldest unwritten chunk always has one and the
/// buffers, which are recycled, bound the memory held by unwritten output.
fn write_chunked<S: Default + Send>(
    n: usize,
    threads: usize,
    chunk: usize,
    inline_max: usize,
    format: &(impl Fn(&mut S, Range<usize>, &mut Vec<u8>) + Sync),
    out: &mut impl Write,
) -> io::Result<()> {
    let chunks = n.div_ceil(chunk);

    if threads <= 1 || n <= inline_max {
        let mut scratch = S::default();
        let mut buf = Vec::new();

        for k in 0..chunks {
            buf.clear();
            format(&mut scratch, k * chunk..((k + 1) * chunk).min(n), &mut buf);
            out.write_all(&buf)?;
        }

        return Ok(());
    }

    let workers = threads.min(chunks);
    let shared = Shared {
        state: Mutex::new(Pool {
            free: (0..workers * 3).map(|_| Vec::new()).collect(),
            done: BTreeMap::new(),
            next: 0,
            stop: false,
        }),
        free_cv: Condvar::new(),
        done_cv: Condvar::new(),
    };
    let shared = &shared;

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(move || {
                let _guard = PanicGuard(shared);
                let mut scratch = S::default();

                loop {
                    let (k, mut buf) = {
                        let mut pool = unpoisoned(shared.state.lock());

                        loop {
                            if pool.stop || pool.next >= chunks {
                                return;
                            }
                            if let Some(buf) = pool.free.pop() {
                                pool.next += 1;
                                break (pool.next - 1, buf);
                            }
                            pool = unpoisoned(shared.free_cv.wait(pool));
                        }
                    };

                    buf.clear();
                    buf.reserve(chunk * 64);
                    format(&mut scratch, k * chunk..((k + 1) * chunk).min(n), &mut buf);

                    unpoisoned(shared.state.lock()).done.insert(k, buf);
                    shared.done_cv.notify_one();
                }
            });
        }

        let mut result = Ok(());

        for want in 0..chunks {
            let finished = {
                let mut pool = unpoisoned(shared.state.lock());

                loop {
                    if let Some(buf) = pool.done.remove(&want) {
                        break Some(buf);
                    }
                    if pool.stop {
                        break None;
                    }
                    pool = unpoisoned(shared.done_cv.wait(pool));
                }
            };

            let Some(buf) = finished else { break };

            if let Err(e) = out.write_all(&buf) {
                result = Err(e);
                break;
            }

            unpoisoned(shared.state.lock()).free.push(buf);
            shared.free_cv.notify_one();
        }

        unpoisoned(shared.state.lock()).stop = true;
        shared.free_cv.notify_all();
        result
    })
}

#[cfg(test)]
mod test {
    use super::*;

    /// Writes the number of each position on its own line.
    fn numbers(_scratch: &mut (), positions: Range<usize>, buf: &mut Vec<u8>) {
        for k in positions {
            buf.extend_from_slice(format!("{k}\n").as_bytes());
        }
    }

    fn expected(n: usize) -> Vec<u8> {
        let mut all = vec![];
        for k in 0..n {
            all.extend_from_slice(k.to_string().as_bytes());
            all.push(b'\n');
        }
        all
    }

    #[test]
    fn chunks_are_written_in_order() {
        for (n, threads, chunk) in [
            (0, 4, 8),
            (1, 4, 8),
            (7, 1, 8),
            (8, 4, 8),
            (9, 4, 8),
            (10_000, 1, 64),
            (10_000, 3, 64),
            (10_000, 8, 7),
            (100_003, 4, 1000),
        ] {
            for inline_max in [0, 100] {
                let mut out = vec![];
                write_chunked(n, threads, chunk, inline_max, &numbers, &mut out).unwrap();
                assert_eq!(out, expected(n), "{n} {threads} {chunk} {inline_max}");
            }
        }
    }

    /// A writer that accepts a few bytes and then fails, like a closed pipe.
    struct Closing(usize);

    impl Write for Closing {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.0 < buf.len() {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            self.0 -= buf.len();
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_failing_writer_stops_the_workers() {
        let err = write_chunked(1_000_000, 4, 100, 0, &numbers, &mut Closing(5_000)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
    }

    #[test]
    #[should_panic(expected = "scoped thread panicked")]
    fn a_panicking_worker_does_not_hang_the_writer() {
        let format = |_scratch: &mut (), positions: Range<usize>, buf: &mut Vec<u8>| {
            assert!(!positions.contains(&5_000), "worker failure");
            numbers(&mut (), positions, buf);
        };
        let _ = write_chunked(100_000, 4, 100, 0, &format, &mut io::sink());
    }
}

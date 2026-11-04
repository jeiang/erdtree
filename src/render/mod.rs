use crate::{context::Context, tree::Tree};
use row::{RowFmt, Scratch};
use std::{
    collections::BTreeMap,
    io::{self, Write},
    marker::PhantomData,
    ops::Range,
    sync::{Condvar, Mutex, MutexGuard, PoisonError},
};

/// Byte-level building blocks of the row writer.
pub mod bytes;

/// Module containing all of the layout variants.
pub mod layout;

/// Directory paths of the flat layouts.
pub mod paths;

/// Box-drawing prefixes of tree rows.
pub mod prefix;

/// Concerned with how to write a single row of the output.
pub mod row;

/// Utility module to fetch the appropriate theme used to paint the box-drawing characters of the
/// output tree.
pub mod theme;

/// Concerned with how to construct the long output.
#[cfg(unix)]
pub mod long;

/// The struct that is generic over T, which is generally expected to be a unit-struct that
/// ultimately determines which variant to use for the output.
pub struct Engine<T> {
    ctx: Context,
    tree: Tree,
    layout: PhantomData<T>,
}

/// The flat output that is similar to `du`, without the ASCII tree.
pub struct Flat;

/// Same as [`Flat`] but the root is at the top of the output.
pub struct FlatInverted;

/// The tree output with the root directory at the bottom of the output.
pub struct Regular;

/// The tree output with the root directory at the top of the output. More like the traditional
/// `tree` command.
pub struct Inverted;

/// An explicit list of rows: the node of each row and, for tree layouts, its box-drawing prefix.
/// Computing it is sequential; layouts use it only where rows depend on each other (flat
/// layouts, and the link theme of `--follow`).
#[derive(Default)]
pub struct Plan {
    ids: Vec<u32>,
    /// Concatenated prefixes; row `k` has `prefixes[prefix_end[k - 1]..prefix_end[k]]`. Empty
    /// for flat layouts.
    prefixes: String,
    prefix_end: Vec<usize>,

    /// The directory paths that flat rows are made of.
    paths: Option<paths::DirPaths>,
}

impl Plan {
    /// A plan with room for `rows` rows.
    pub fn with_capacity(rows: usize, prefixed: bool) -> Self {
        Self {
            ids: Vec::with_capacity(rows),
            // Most prefixes are a few box-drawing segments of 3 to 9 bytes.
            prefixes: String::with_capacity(if prefixed { rows * 24 } else { 0 }),
            prefix_end: Vec::with_capacity(if prefixed { rows } else { 0 }),
            paths: None,
        }
    }

    /// Flat rows of the nodes `ids`, whose paths are made from the directories of `tree` down to
    /// `level`, found with `threads` threads.
    pub fn flat(ids: Vec<u32>, tree: &Tree, level: usize, threads: usize) -> Self {
        Self {
            ids,
            paths: Some(paths::DirPaths::new(tree, level, threads)),
            ..Self::default()
        }
    }

    /// Adds a tree row whose prefix is the concatenation of `parts`.
    pub fn push_with_prefix<'a>(&mut self, id: u32, parts: impl IntoIterator<Item = &'a str>) {
        self.ids.push(id);
        parts
            .into_iter()
            .for_each(|part| self.prefixes.push_str(part));
        self.prefix_end.push(self.prefixes.len());
    }

    /// Requests the data of the rows after `k` in stages, the rows further ahead first, so that
    /// the dependent loads of a flat row (name, parent, parent's path) overlap.
    #[inline]
    fn prefetch(&self, tree: &Tree, k: usize) {
        const AHEAD: usize = 16;

        let Some(paths) = &self.paths else { return };
        let at = |d: usize| self.ids.get(k + d).copied();

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

    fn prefix(&self, k: usize) -> Option<&str> {
        let end = *self.prefix_end.get(k)?;
        let start = k.checked_sub(1).map_or(0, |p| self.prefix_end[p]);
        Some(&self.prefixes[start..end])
    }
}

/// Which rows a layout prints, in order.
pub enum Rows {
    /// The rows of a [`Plan`].
    Plan(Plan),

    /// The shown nodes of the tree in pre-order, or reversed, with the prefixes worked out from
    /// each row's ancestors. Rows deeper than the level are left out.
    Tree {
        reverse: bool,
        glyphs: prefix::Glyphs,
    },
}

impl Rows {
    /// Number of positions to format; a tree layout may skip some of them.
    fn len(&self, tree: &Tree) -> usize {
        match self {
            Self::Plan(plan) => plan.ids.len(),
            Self::Tree { .. } => tree.order().len(),
        }
    }

    /// Appends the rows at `positions`.
    fn write_range(
        &self,
        fmt: &RowFmt<'_>,
        tree: &Tree,
        level: usize,
        scratch: &mut Scratch,
        positions: Range<usize>,
        out: &mut Vec<u8>,
    ) {
        match self {
            Self::Plan(plan) => {
                for k in positions {
                    plan.prefetch(tree, k);
                    let id = plan.ids[k];

                    match plan.prefix(k) {
                        Some(prefix) => fmt.tree_row(scratch, id, out, |_, out| {
                            out.extend_from_slice(prefix.as_bytes());
                        }),
                        None => fmt.flat_row(scratch, plan.paths.as_ref(), id, out),
                    }
                }
            },
            Self::Tree { reverse, glyphs } => {
                let order = tree.order();
                let last = order.len().saturating_sub(1);

                for k in positions {
                    let id = order[if *reverse { last - k } else { k }];
                    let depth = tree.node(id).depth();

                    if depth <= level {
                        fmt.tree_row(scratch, id, out, |cursor, out| {
                            cursor.write(tree, id, depth, glyphs, out);
                        });
                    }
                }
            },
        }
    }
}

/// A layout decides which rows are printed, in which order, and with which prefixes.
pub trait Layout {
    /// The rows of `tree`.
    fn rows(tree: &Tree, ctx: &Context) -> Rows;
}

/// Positions formatted per unit of work. A chunk of rows is a few hundred KB of output, which
/// stays in the cache until it is written.
const CHUNK: usize = 4096;

/// Up to this many positions are formatted on the calling thread; spawning workers costs more
/// than formatting a few thousand rows.
const INLINE_MAX: usize = 2 * CHUNK;

impl<T: Layout> Engine<T> {
    /// Writes every row, one per line. Rows are formatted in parallel and written in order.
    pub fn write(&self, out: &mut impl Write) -> io::Result<()> {
        let tree = &self.tree;
        let rows = T::rows(tree, &self.ctx);
        let fmt = RowFmt::new(tree, &self.ctx);
        let level = self.ctx.level();

        write_chunked(
            rows.len(tree),
            self.ctx.threads.max(1),
            CHUNK,
            INLINE_MAX,
            &|scratch: &mut Scratch, positions, buf: &mut Vec<u8>| {
                rows.write_range(&fmt, tree, level, scratch, positions, buf);
            },
            out,
        )
    }
}

impl<T> Engine<T> {
    /// Initializes a new [Engine].
    pub const fn new(tree: Tree, ctx: Context) -> Self {
        Self {
            ctx,
            tree,
            layout: PhantomData,
        }
    }
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
            lock(&self.0.state).stop = true;
            self.0.free_cv.notify_all();
            self.0.done_cv.notify_all();
        }
    }
}

/// Locks `mutex`; a panicked worker is reported elsewhere, and the data stays consistent.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
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
                        let mut pool = lock(&shared.state);

                        loop {
                            if pool.stop || pool.next >= chunks {
                                return;
                            }
                            if let Some(buf) = pool.free.pop() {
                                pool.next += 1;
                                break (pool.next - 1, buf);
                            }
                            pool = shared
                                .free_cv
                                .wait(pool)
                                .unwrap_or_else(PoisonError::into_inner);
                        }
                    };

                    buf.clear();
                    buf.reserve(chunk * 64);
                    format(&mut scratch, k * chunk..((k + 1) * chunk).min(n), &mut buf);

                    lock(&shared.state).done.insert(k, buf);
                    shared.done_cv.notify_one();
                }
            });
        }

        let mut result = Ok(());

        for want in 0..chunks {
            let finished = {
                let mut pool = lock(&shared.state);

                loop {
                    if let Some(buf) = pool.done.remove(&want) {
                        break Some(buf);
                    }
                    if pool.stop {
                        break None;
                    }
                    pool = shared
                        .done_cv
                        .wait(pool)
                        .unwrap_or_else(PoisonError::into_inner);
                }
            };

            let Some(buf) = finished else { break };

            if let Err(e) = out.write_all(&buf) {
                result = Err(e);
                break;
            }

            lock(&shared.state).free.push(buf);
            shared.free_cv.notify_one();
        }

        lock(&shared.state).stop = true;
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

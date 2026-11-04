use crate::{context::Context, tree::Tree};
use grid::Row;
use std::{
    fmt::Write as _,
    io::{self, Write},
    marker::PhantomData,
};

/// Module containing all of the layout variants.
pub mod layout;

/// Concerned with how to construct a single row in the output grid.
pub mod grid;

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

/// The rows a layout prints, in order: the node of each row and, for tree layouts, its
/// box-drawing prefix. Computing the plan is sequential and cheap; formatting the rows is the
/// expensive part and runs in parallel.
#[derive(Default)]
pub struct Plan {
    ids: Vec<u32>,
    /// Concatenated prefixes; row `k` has `prefixes[prefix_end[k - 1]..prefix_end[k]]`. Empty
    /// for flat layouts.
    prefixes: String,
    prefix_end: Vec<usize>,
}

impl Plan {
    /// A plan with room for `rows` rows.
    pub fn with_capacity(rows: usize, prefixed: bool) -> Self {
        Self {
            ids: Vec::with_capacity(rows),
            // Most prefixes are a few box-drawing segments of 3 to 9 bytes.
            prefixes: String::with_capacity(if prefixed { rows * 24 } else { 0 }),
            prefix_end: Vec::with_capacity(if prefixed { rows } else { 0 }),
        }
    }

    /// Adds a flat row.
    pub fn push(&mut self, id: u32) {
        self.ids.push(id);
    }

    /// Adds a tree row whose prefix is the concatenation of `parts`.
    pub fn push_with_prefix<'a>(&mut self, id: u32, parts: impl IntoIterator<Item = &'a str>) {
        self.ids.push(id);
        parts
            .into_iter()
            .for_each(|part| self.prefixes.push_str(part));
        self.prefix_end.push(self.prefixes.len());
    }

    fn prefix(&self, k: usize) -> Option<&str> {
        let end = *self.prefix_end.get(k)?;
        let start = k.checked_sub(1).map_or(0, |p| self.prefix_end[p]);
        Some(&self.prefixes[start..end])
    }
}

/// A layout decides which rows are printed, in which order, and with which prefixes.
pub trait Layout {
    /// Grid kind of the rows ([`grid::Tree`] or [`grid::Flat`]).
    type Grid;

    /// The rows of `tree`.
    fn plan(tree: &Tree, ctx: &Context) -> Plan;
}

/// Rows formatted per thread per batch; bounds the memory held by unwritten output.
const ROWS_PER_THREAD: usize = 16 * 1024;

impl<T: Layout> Engine<T>
where
    for<'a> Row<'a, T::Grid>: std::fmt::Display,
{
    /// Writes every row, one per line. Rows are formatted in parallel in batches and written in
    /// order.
    pub fn write(&self, out: &mut impl Write) -> io::Result<()> {
        let plan = T::plan(&self.tree, &self.ctx);
        let threads = self.ctx.threads.max(1);
        let batch = ROWS_PER_THREAD * threads;
        let mut bufs = vec![String::new(); threads];

        for first in (0..plan.ids.len()).step_by(batch) {
            let last = (first + batch).min(plan.ids.len());

            std::thread::scope(|s| {
                for (t, buf) in bufs.iter_mut().enumerate() {
                    let from = (first + t * ROWS_PER_THREAD).min(last);
                    let to = (from + ROWS_PER_THREAD).min(last);
                    let plan = &plan;
                    let (tree, ctx) = (&self.tree, &self.ctx);

                    if from == to {
                        buf.clear();
                        continue;
                    }

                    s.spawn(move || {
                        // A local `String`: the headers in `bufs` share a cache line, and
                        // updating them from several threads would serialize the threads.
                        let mut local = std::mem::take(buf);
                        local.clear();
                        for k in from..to {
                            let node = tree.node(plan.ids[k]);
                            let row = Row::<T::Grid>::new(node, ctx, plan.prefix(k));
                            // Writing to a `String` cannot fail.
                            let _ = writeln!(local, "{row}");
                        }
                        *buf = local;
                    });
                }
            });

            for buf in &bufs {
                out.write_all(buf.as_bytes())?;
            }
        }

        Ok(())
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

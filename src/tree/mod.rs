use crate::{
    context::{Context, column},
    disk_usage::file_size::FileSize,
    fs::inode::Inode,
    walk::{Walker, rules},
};
use columns::{Collector, Columns, Probe};
use count::FileCount;
use error::Error;
use node::{Node, cmp::SortKeys};
use std::{
    cmp::Ordering, convert::TryFrom, fs, result::Result as StdResult, sync::atomic::AtomicU64,
};
use topo::{Children, NONE, ix};
use widths::Scope;

/// Struct-of-arrays storage for walked entries.
pub mod columns;

/// Pure algorithms over the tree topology, with Kani proofs.
pub mod topo;

/// Operations to handle and display aggregate file counts based on their type.
pub mod count;

/// Errors related to traversal, [Tree] construction, and the like.
pub mod error;

/// A borrowed view of one entry of a [`Tree`].
pub mod node;

/// Parallel sorting of keyed items.
mod psort;

/// Column widths from maxima over columns.
pub mod widths;

/// Trees with at least this many nodes compute column widths while the child lists are sorted.
const OVERLAP_MIN: usize = 1 << 16;

/// Factor by which the walk may add workers when it waits for the file system.
const WALK_GROWTH: usize = 8;

/// Most walk workers when the walk adds workers.
const WALK_MAX_THREADS: usize = 64;

/// Counts of at least this many nodes use all threads.
const PAR_COUNT_MIN: usize = 1 << 18;

/// The walked file-system hierarchy, stored as columns indexed by node.
pub struct Tree {
    cols: Columns,
    root: u32,
    children: Children,

    /// Nodes still shown after `--prune`, `--dirs-only` and `--min-size`.
    alive: Vec<bool>,

    /// Alive nodes in display pre-order (children sorted), down to the display level unless a
    /// filter needs the whole tree.
    order: Vec<u32>,

    /// Whether a node is the last alive child of its parent.
    last: Vec<bool>,

    /// Bit `i` is set when node `i` is a symlink (has a link target).
    link: Vec<u64>,

    /// Nodes at this depth or deeper have no shown children: the display level, when `order`
    /// stops there. [`u32::MAX`] when `order` goes all the way down.
    shown_depth: u32,

    /// Whether there is nothing to show below the root.
    stump: bool,

    /// Byte-metric display settings, to rebuild [`FileSize`] values.
    size_format: SizeFormat,

    /// Owner and group names by id, sorted by id, for the long view.
    #[cfg(unix)]
    users: Vec<(u32, Option<String>)>,
    #[cfg(unix)]
    groups: Vec<(u32, Option<String>)>,

    /// Counts added to the summary: entries hidden by `--min-size`, and entries skipped by the
    /// walk (`--report-ignored`).
    hidden: FileCount,

    /// The summary count, see [`Self::file_count`].
    count: FileCount,
}

/// How sizes are displayed.
#[derive(Clone, Copy)]
struct SizeFormat {
    disk_usage: crate::disk_usage::file_size::DiskUsage,
    human: bool,
    unit: crate::disk_usage::units::PrefixKind,
}

pub type Result<T> = StdResult<T, Error>;

/// Number of threads for CPU-bound work: `--threads`, or one per core for 0.
fn thread_count(ctx: &Context) -> usize {
    if ctx.threads == 0 {
        std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get)
    } else {
        ctx.threads
    }
}

impl Tree {
    /// Walks the root of `ctx` and builds the [Tree]; also computes the column widths that
    /// rendering needs into the returned [Context]. The tree may be a stump (see
    /// [`Self::is_stump`]). `progress` counts walked entries.
    pub fn try_init(mut ctx: Context, progress: Option<&AtomicU64>) -> Result<(Self, Context)> {
        let mut walker = walker(&ctx)?;
        let probe = Probe::new(&ctx);
        walker.fields = probe.fields();

        let sinks = walker.run(|_| Collector::new(probe), progress);
        let cols = Columns::merge(sinks.into_iter().map(|c| c.columns).collect());

        let mut col_props = column::Properties::from(&ctx);
        let tree = Self::assemble(cols, &ctx, &mut col_props)?;

        ctx.update_column_properties(&col_props);

        if ctx.truncate {
            ctx.set_window_width();
        }

        Ok((tree, ctx))
    }

    /// Builds the topology, sizes, order and filters from the merged columns.
    ///
    /// The `depth` column must hold each node's distance from the root (parent depth plus
    /// one, root 0), as the walk records it: that gives a parent-first order without a
    /// traversal, and cuts the display order off at `--level`.
    fn assemble(
        mut cols: Columns,
        ctx: &Context,
        col_props: &mut column::Properties,
    ) -> Result<Self> {
        let n = cols.len();
        let threads = thread_count(ctx);
        let root = cols
            .parent
            .iter()
            .position(|&p| p == NONE)
            .and_then(|r| u32::try_from(r).ok())
            .ok_or(Error::MissingRoot)?;

        debug_assert!(
            cols.parent
                .iter()
                .zip(&cols.depth)
                .all(|(&p, &d)| p == NONE && d == 0 || p != NONE && cols.depth[ix(p)] + 1 == d),
            "depth is the distance from the root"
        );

        if !cols.link_target.is_sorted_by_key(|(i, _)| *i) {
            cols.link_target.sort_unstable_by_key(|(i, _)| *i);
        }

        let mut link = vec![0_u64; n.div_ceil(64)];
        for &(i, _) in &cols.link_target {
            link[ix(i) / 64] |= 1 << (i % 64);
        }

        let is_dir: Vec<bool> = cols.file_type.iter().map(|kind| kind.is_dir()).collect();
        let children = Children::from_runs(n, &std::mem::take(&mut cols.runs));

        // Nothing is hidden by a filter, so the display order can stop at `--level`.
        let min_size = ctx.min_size.map(|m| m.value).filter(|&v| v > 0);
        let filtered = ctx.prune || ctx.pattern.is_some() || ctx.dirs_only || min_size.is_some();
        let cap = if filtered {
            u32::MAX
        } else {
            u32::try_from(ctx.level()).unwrap_or(u32::MAX)
        };

        let mut tree = Self {
            root,
            children,
            alive: vec![true; n],
            order: vec![],
            last: vec![],
            link,
            shown_depth: cap,
            stump: false,
            size_format: SizeFormat {
                disk_usage: ctx.disk_usage,
                human: ctx.human,
                unit: ctx.unit,
            },
            #[cfg(unix)]
            users: vec![],
            #[cfg(unix)]
            groups: vec![],
            hidden: FileCount::default(),
            count: FileCount::default(),
            cols,
        };

        // Sizes: each inode with several links counts once, in the directory that comes
        // first by path.
        let uncounted = tree.dedupe_hard_links(threads);
        topo::aggregate(
            &tree.children,
            &tree.cols.parent,
            &tree.cols.depth,
            &is_dir,
            &uncounted,
            &mut tree.cols.size,
            threads,
        );
        drop(uncounted);

        tree.sort_children(ctx, col_props, min_size.is_none());
        tree.filter_and_order(ctx, col_props, &is_dir);

        Ok(tree)
    }

    /// Applies `--prune`, patterns, `--dirs-only` and `--min-size`, then builds the display
    /// order and the summary count.
    fn filter_and_order(
        &mut self,
        ctx: &Context,
        col_props: &mut column::Properties,
        is_dir: &[bool],
    ) {
        let tree = self;
        let (root, cap) = (tree.root, tree.shown_depth);
        let n = tree.cols.len();
        let threads = thread_count(ctx);
        let min_size = ctx.min_size.map(|m| m.value).filter(|&v| v > 0);
        let filtered = ctx.prune || ctx.pattern.is_some() || ctx.dirs_only || min_size.is_some();

        // Any parent-first order serves `prune` and `hide_small`.
        let parent_first = (ctx.prune || ctx.pattern.is_some() || min_size.is_some()).then(|| {
            topo::by_depth(&tree.cols.depth).unwrap_or_else(|| {
                topo::preorder(
                    &tree.children,
                    root,
                    &tree.alive,
                    &tree.cols.depth,
                    u32::MAX,
                )
            })
        });
        let parent_first = parent_first.as_deref().unwrap_or_default();

        if ctx.prune || ctx.pattern.is_some() {
            topo::prune(parent_first, &tree.cols.parent, is_dir, &mut tree.alive);
        }

        if ctx.dirs_only {
            topo::dirs_only(&tree.cols.parent, is_dir, &mut tree.alive);
        }

        if let Some(min_size) = min_size {
            let shown: Vec<u32> = parent_first
                .iter()
                .copied()
                .filter(|&i| tree.alive[ix(i)])
                .collect();
            let hidden = topo::hide_small(
                &shown,
                &tree.cols.parent,
                &tree.cols.size,
                min_size,
                &mut tree.alive,
            );

            tree.hidden = hidden
                .into_iter()
                .fold(FileCount::default(), |acc, i| acc + tree.node(i));
        }

        tree.order = topo::preorder(&tree.children, root, &tree.alive, &tree.cols.depth, cap);

        if min_size.is_some() {
            // Hidden entries must not leave the size columns wider than needed.
            *col_props = column::Properties::from(ctx);
            let stats = tree.scan(ctx, Scope::Listed(&tree.order), threads);
            tree.apply_widths(ctx, col_props, &stats);
        }

        tree.hidden.num_ignored = tree.cols.ignored;

        // Without filters every node is shown, whatever the level cuts off the display.
        tree.count = if filtered {
            tree.order
                .iter()
                .skip(1)
                .fold(tree.hidden, |acc, &i| acc + tree.node(i))
        } else {
            tree.count_all(threads)
        };
        tree.stump = if filtered {
            tree.order.len() <= 1
        } else {
            n <= 1
        };
        tree.last = topo::last_alive_child(&tree.children, &tree.order, &tree.alive, threads);
    }

    /// Sorts the child lists of the directories shown (down to the display level), and with
    /// `scan_now` also widens `col_props` to fit every node. The widths do not depend on the
    /// order, so large trees find them while the lists are sorted.
    fn sort_children(&mut self, ctx: &Context, col_props: &mut column::Properties, scan_now: bool) {
        let threads = thread_count(ctx);
        let cap = self.shown_depth;
        let comparator = node::cmp::comparator(ctx);
        let mut children = std::mem::replace(&mut self.children, Children::build(&[]));

        let stats = {
            let view = &*self;
            let keys = SortKeys::new(view, ctx, true);
            let overlap = scan_now && self.cols.len() >= OVERLAP_MIN && threads > 1;

            std::thread::scope(|s| {
                let job = overlap.then(|| s.spawn(|| view.scan(ctx, Scope::All, 1)));

                children.sort_each(
                    threads,
                    |p| view.cols.depth[ix(p)] < cap,
                    |i| keys.key(i),
                    |a, b| {
                        let (a, b) = (view.node(a), view.node(b));
                        comparator(a, b).then_with(|| a.file_name().cmp(b.file_name()))
                    },
                );

                job.map_or_else(
                    || scan_now.then(|| view.scan(ctx, Scope::All, threads)),
                    |job| Some(job.join().unwrap_or_else(|e| std::panic::resume_unwind(e))),
                )
            })
        };

        self.children = children;

        if let Some(stats) = stats {
            self.apply_widths(ctx, col_props, &stats);
        }
    }

    /// The summary count of an unfiltered tree: every node below the root, plus those skipped
    /// by the walk.
    fn count_all(&self, threads: usize) -> FileCount {
        let n = self.cols.len();
        let count = |range: std::ops::Range<usize>| {
            range
                .filter(|&i| i != ix(self.root))
                .fold(FileCount::default(), |acc, i| {
                    acc + self.node(u32::try_from(i).unwrap_or(NONE))
                })
        };

        let mut total = if n < PAR_COUNT_MIN || threads < 2 {
            count(0..n)
        } else {
            let width = n.div_ceil(threads);
            std::thread::scope(|s| {
                let count = &count;
                let mut jobs = vec![];
                for t in 1..threads {
                    jobs.push(s.spawn(move || count((t * width).min(n)..((t + 1) * width).min(n))));
                }

                jobs.into_iter().fold(count(0..width.min(n)), |acc, job| {
                    acc + job.join().unwrap_or_else(|e| std::panic::resume_unwind(e))
                })
            })
        };

        total.num_ignored = self.cols.ignored;
        total
    }

    /// Hard-link deduplication (see [`topo::dedupe`]): among entries sharing an inode (hard
    /// links, or a directory reached again through a followed symlink), one counts towards its
    /// directory's size: preferably one not reached through a symlink, then the least path.
    fn dedupe_hard_links(&mut self, threads: usize) -> Vec<u32> {
        let mut linked: Vec<(u32, Inode)> = std::mem::take(&mut self.cols.multi_linked);

        topo::dedupe(&mut linked, threads, |a, b| {
            self.through_symlink(a)
                .cmp(&self.through_symlink(b))
                .then_with(|| self.path_cmp(a, b))
        })
    }

    /// Whether node `i` or one of its ancestors is a symlink.
    fn through_symlink(&self, i: u32) -> bool {
        let mut cur = i;

        while cur != NONE {
            if self.is_link(cur) {
                return true;
            }
            cur = self.cols.parent[ix(cur)];
        }

        false
    }

    /// Whether node `i` is a symlink (has a link target).
    #[inline]
    pub(crate) fn is_link(&self, i: u32) -> bool {
        self.link[ix(i) / 64] >> (i % 64) & 1 == 1
    }

    /// Order of the paths of nodes `a` and `b`, as [`Path::cmp`](std::path::Path::cmp) orders
    /// [`Self::path_of`]: component by component, a directory before what it contains. Both
    /// paths start with the root's, so this compares the names below their closest common
    /// ancestor, without building either path.
    fn path_cmp(&self, a: u32, b: u32) -> Ordering {
        let (depth, parent) = (&self.cols.depth, &self.cols.parent);
        let (mut x, mut y) = (a, b);

        while depth[ix(x)] > depth[ix(y)] {
            x = parent[ix(x)];
        }
        while depth[ix(y)] > depth[ix(x)] {
            y = parent[ix(y)];
        }

        if x == y {
            // One is above the other, or they are the same node.
            return depth[ix(a)].cmp(&depth[ix(b)]);
        }

        while parent[ix(x)] != parent[ix(y)] {
            x = parent[ix(x)];
            y = parent[ix(y)];
        }

        self.cols.name(ix(x)).cmp(self.cols.name(ix(y)))
    }

    /// The summary count: every shown entry below the root, plus those hidden by `--min-size`
    /// or skipped by the walk. Not cut off by `--level`.
    pub const fn file_count(&self) -> FileCount {
        self.count
    }

    /// Returns `true` if there are no entries to show excluding the root.
    pub const fn is_stump(&self) -> bool {
        self.stump
    }

    /// The root node.
    pub const fn root(&self) -> u32 {
        self.root
    }

    /// Shown nodes in display pre-order, root first. Stops at the display level (`--level`)
    /// unless a filter is active; layouts show nothing deeper either way.
    pub fn order(&self) -> &[u32] {
        &self.order
    }

    /// Hints that the columns of the row of `i` are needed soon (see [`Self::prefetch_name`]).
    #[inline]
    pub fn prefetch_cols(&self, i: u32) {
        self.cols.prefetch_cols(ix(i));
    }

    /// Hints that the name of `i` is needed soon; call it after [`Self::prefetch_cols`].
    #[inline]
    pub fn prefetch_name(&self, i: u32) {
        self.cols.prefetch_name(ix(i));
    }

    /// View of node `i`.
    #[inline]
    pub const fn node(&self, i: u32) -> Node<'_> {
        Node::new(self, i)
    }

    /// Whether `i` is the last shown child of its parent.
    #[inline]
    pub fn is_last(&self, i: u32) -> bool {
        self.last[ix(i)]
    }

    /// Parent of `i`; [`topo::NONE`] for the root.
    #[inline]
    pub fn parent(&self, i: u32) -> u32 {
        self.cols.parent[ix(i)]
    }

    /// Whether `i` has a shown child: one in [`Self::order`].
    #[inline]
    pub fn has_children(&self, i: u32) -> bool {
        self.cols.depth[ix(i)] < self.shown_depth
            && self.children.of(i).iter().any(|&c| self.alive[ix(c)])
    }

    /// Nodes in the order the inverted flat layout (`iflat`) displays them; the flat layout
    /// displays the same list reversed.
    ///
    /// Name sorts keep the hierarchy: a pre-order walk of the per-directory sorted tree, so each
    /// directory's contents follow it, as when sorting by full path. Other sorts are global: the
    /// root comes first and every other node follows, ordered by the configured sort and
    /// `--dir-order` across the whole tree. That sort is stable over a pre-order walk, so ties
    /// list a directory before its contents.
    #[must_use]
    pub fn flat_order(&self, ctx: &Context) -> Vec<u32> {
        if ctx.sort.is_name() || self.order.len() <= 1 {
            return self.order.clone();
        }

        // The flat layouts show nothing deeper than the level, and a stable sort of a subset
        // lists it as the sort of the whole does, so sort just the visible nodes.
        let level = ctx.level();
        let rest: Vec<u32> = self.order[1..]
            .iter()
            .copied()
            .filter(|&i| ix(self.cols.depth[ix(i)]) <= level)
            .collect();

        // Sort positions in `rest`, breaking ties by position to keep the stable order.
        let comparator = node::cmp::comparator(ctx);
        let keys = SortKeys::new(self, ctx, false);
        let node = |k: u32| self.node(rest[ix(k)]);
        let mut positions: Vec<u32> = (0..rest.len())
            .map(|k| u32::try_from(k).unwrap_or(NONE))
            .collect();

        topo::sort_keyed(
            &mut positions,
            thread_count(ctx),
            &|k| keys.key(rest[ix(k)]),
            // Names are not in the keys: unless a key was clamped, equal keys are ties.
            &|a, b| {
                if keys.is_exact() {
                    a.cmp(&b)
                } else {
                    comparator(node(a), node(b)).then_with(|| a.cmp(&b))
                }
            },
        );

        let mut ids = Vec::with_capacity(rest.len() + 1);
        ids.push(self.order[0]);
        ids.extend(positions.iter().map(|&k| rest[ix(k)]));
        ids
    }
}

/// Widths of the number and the unit of a human-readable size, formatted without allocating.
fn human_widths(file_size: &FileSize) -> (usize, usize) {
    use std::io::Write;

    let mut buf = [0_u8; 64];
    let mut cursor = std::io::Cursor::new(&mut buf[..]);
    let _ = write!(cursor, "{file_size}");
    let len = usize::try_from(cursor.position()).unwrap_or(0);
    let text = &buf[..len];

    text.iter()
        .position(|&b| b == b' ')
        .map_or((len, 0), |space| (space, len - space - 1))
}

/// Configures the walk from `ctx`.
fn walker(ctx: &Context) -> Result<Walker> {
    let root = fs::canonicalize(ctx.dir())
        .map_err(|e| Error::DirNotFound(format!("{}: {e}", ctx.dir().display())))?;

    if !root.is_dir() {
        return Err(Error::DirNotFound(format!(
            "{}: Not a directory",
            ctx.dir().display()
        )));
    }

    // With `--include`, hidden entries are filtered by `include_predicate` instead, as the
    // ignore rules cannot make exceptions.
    let include_hidden = !ctx.hidden && !ctx.include.is_empty();

    let opts = rules::Options::new(!ctx.hidden && !include_hidden, !ctx.no_ignore);
    let cwd = std::env::current_dir().ok();
    let rules = rules::Rules::root(opts, ctx.no_git_override()?, cwd);

    let mut filters = vec![];

    if include_hidden {
        filters.push(ctx.include_predicate()?);
    }

    if ctx.pattern.is_some() {
        if ctx.glob || ctx.iglob {
            filters.push(ctx.glob_predicate()?);
        } else {
            filters.push(ctx.regex_predicate()?);
        }
    }

    let filter: Option<crate::walk::Filter> = if filters.is_empty() {
        None
    } else {
        Some(Box::new(move |entry| {
            filters.iter().all(|keep| keep(entry))
        }))
    };

    let threads = thread_count(ctx);

    Ok(Walker {
        root,
        rules,
        follow_links: ctx.follow,
        same_file_system: ctx.same_fs,
        // `--level` is display-only; only `--traversal-depth` limits the walk. Both count the
        // root as depth 0, so `--level N --traversal-depth N` shows the same entries.
        max_depth: ctx.traversal_depth,
        threads,
        // Without `--threads`, workers that wait for the file system get company.
        max_threads: if ctx.threads_explicit {
            threads
        } else {
            (threads * WALK_GROWTH).min(WALK_MAX_THREADS).max(threads)
        },
        filter,
        count_ignored: ctx.report_ignored,
        fields: crate::fs::stat::Fields::default(),
        tuning: crate::walk::Tuning::default(),
    })
}

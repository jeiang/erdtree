use crate::{
    context::Context,
    styles::Colors,
    walk::{
        self, Walker,
        filter::{self, EntryFilter, Hidden, Pattern},
        rules,
    },
};
use columns::{Collector, Columns, Probe};
use count::FileCount;
use error::Error;
use id::{NONE, id, ix};
#[cfg(unix)]
use node::owners::Owners;
use node::{
    Node,
    cmp::{Sorter, Ties},
};
use std::{
    cmp::Ordering, convert::TryFrom, path::Path, result::Result as StdResult,
    sync::atomic::AtomicU64,
};
use topo::Children;
use widths::{Scope, Stats, Widths};

/// Struct-of-arrays storage for walked entries.
pub mod columns;
pub mod id;

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

/// Counts of at least this many nodes use all threads.
const PAR_COUNT_MIN: usize = 1 << 18;

/// The walked file-system hierarchy, stored as columns indexed by node.
pub struct Tree {
    cols: Columns,
    root: u32,

    /// Alive nodes in display pre-order (children sorted), down to the display level.
    order: Vec<u32>,

    /// Whether a node is the last alive child of its parent.
    last: Vec<bool>,

    /// Bit `i` is set when node `i` is a symlink (has a link target).
    link: Vec<u64>,

    /// The display level: `order` stops at this depth.
    level: u32,

    /// Whether there is nothing to show below the root.
    stump: bool,

    /// Column widths that fit the shown nodes (all nodes unless `--min-size` hid some).
    widths: Widths,

    /// Owner and group names of the shown nodes, for the long view.
    #[cfg(unix)]
    owners: Owners,

    /// The summary count, see [`Self::file_count`].
    count: FileCount,
}

/// What hides nodes from the display, derived once from the [`Context`].
#[derive(Clone, Copy)]
struct Filters {
    /// `--prune` or a pattern: directories left without shown entries are hidden.
    prune: bool,
    dirs_only: bool,
    /// The `--min-size` threshold, when it can hide anything.
    min_size: Option<u64>,
}

impl Filters {
    fn new(ctx: &Context) -> Self {
        Self {
            prune: ctx.prune || ctx.pattern.is_some(),
            dirs_only: ctx.dirs_only,
            min_size: ctx.min_size.map(|m| m.value).filter(|&v| v > 0),
        }
    }
}

pub type Result<T> = StdResult<T, Error>;

impl Tree {
    /// Walks `root` with the options of `ctx` and builds the [Tree] with the column widths that
    /// rendering needs. Entries get `LS_COLORS` styles from `colors`, when output is colored.
    /// The tree may be a stump (see [`Self::is_stump`]). `progress` counts walked entries.
    pub fn try_init(
        ctx: &Context,
        root: &Path,
        colors: Option<&'static Colors>,
        progress: Option<&AtomicU64>,
    ) -> Result<Self> {
        let probe = Probe::new(ctx);
        let walker = walker(ctx, root, probe)?;

        let sinks = walker.run(|| Collector::new(probe, colors), progress);
        let cols = Columns::merge(sinks.into_iter().map(Collector::into_columns).collect());

        Self::assemble(cols, ctx)
    }

    /// Builds the topology, sizes, order, filters and column widths from the merged columns.
    ///
    /// The `depth` column must hold each node's distance from the root (parent depth plus
    /// one, root 0), as the walk records it: that gives a parent-first order without a
    /// traversal, and cuts the display order off at `--level`.
    fn assemble(mut cols: Columns, ctx: &Context) -> Result<Self> {
        let n = cols.len();
        let threads = ctx.threads;
        let root = cols
            .parent
            .iter()
            .position(|&p| p == NONE)
            .map(id)
            .filter(|&r| r != NONE)
            .ok_or(Error::MissingRoot)?;

        debug_assert!(
            cols.parent
                .iter()
                .zip(&cols.depth)
                .all(|(&p, &d)| p == NONE && d == 0 || p != NONE && cols.depth[ix(p)] + 1 == d),
            "depth is the distance from the root"
        );

        debug_assert!(
            cols.link_target.is_sorted_by_key(|(i, _)| *i),
            "each sink records links in index order and the merge appends sinks in offset order"
        );

        let mut link = vec![0_u64; n.div_ceil(64)];
        for &(i, _) in &cols.link_target {
            link[ix(i) / 64] |= 1 << (i % 64);
        }

        let is_dir: Vec<bool> = cols.file_type.iter().map(|kind| kind.is_dir()).collect();
        let mut children = Children::from_runs(n, &std::mem::take(&mut cols.runs));

        // The display order stops at `--level`; the filters only decide which nodes are shown.
        let filters = Filters::new(ctx);
        let level = u32::try_from(ctx.level()).unwrap_or(u32::MAX);

        let mut tree = Self {
            root,
            order: vec![],
            last: vec![],
            link,
            level,
            stump: false,
            widths: Widths::default(),
            #[cfg(unix)]
            owners: Owners::default(),
            count: FileCount::default(),
            cols,
        };

        // Sizes: each inode with several links counts once, in the directory that comes
        // first by path.
        let uncounted = tree.dedupe_hard_links(threads);
        topo::aggregate(
            &children,
            &tree.cols.parent,
            &tree.cols.depth,
            &is_dir,
            &uncounted,
            &mut tree.cols.size,
            threads,
        );
        drop(uncounted);

        // Entries hidden by `--min-size` must not widen the size columns, so then the widths
        // wait for the filters.
        let stats = tree.sort_children(ctx, &mut children, filters.min_size.is_none(), threads);
        let shown = tree.filter_and_order(&children, &is_dir, filters, threads);
        let stats = stats.unwrap_or_else(|| tree.scan(ctx, Scope::Shown(&shown), threads));

        #[cfg(unix)]
        {
            tree.owners = Owners::resolve(stats.owners());
            tree.widths = Widths::new(ctx, &stats, &tree.owners);
        }
        #[cfg(not(unix))]
        {
            tree.widths = Widths::new(ctx, &stats);
        }

        Ok(tree)
    }

    /// Applies `--prune`, patterns, `--dirs-only` and `--min-size`, then builds the display
    /// order and the summary count. Returns which nodes are shown, at any depth.
    fn filter_and_order(
        &mut self,
        children: &Children,
        is_dir: &[bool],
        filters: Filters,
        threads: usize,
    ) -> Vec<bool> {
        let tree = self;
        let root = tree.root;
        let n = tree.cols.len();

        // Nodes still shown after the filters.
        let mut alive = vec![true; n];

        // Any parent-first order serves `prune` and `hide_small`.
        let parent_first = (filters.prune || filters.min_size.is_some()).then(|| {
            topo::by_depth(&tree.cols.depth).unwrap_or_else(|| {
                topo::preorder(children, root, &alive, &tree.cols.depth, u32::MAX)
            })
        });
        let parent_first = parent_first.as_deref().unwrap_or_default();

        if filters.prune {
            topo::prune(parent_first, &tree.cols.parent, is_dir, &mut alive);
        }

        if filters.dirs_only {
            topo::dirs_only(&tree.cols.parent, is_dir, &mut alive);
        }

        // Counts added to the summary: entries hidden by `--min-size`, and entries skipped by
        // the walk (`--report-ignored`).
        let mut hidden = FileCount::default();

        if let Some(min_size) = filters.min_size {
            let shown: Vec<u32> = parent_first
                .iter()
                .copied()
                .filter(|&i| alive[ix(i)])
                .collect();
            hidden = topo::hide_small(
                &shown,
                &tree.cols.parent,
                &tree.cols.size,
                min_size,
                &mut alive,
            )
            .into_iter()
            .fold(hidden, |acc, i| acc + tree.node(i));
        }

        tree.order = topo::preorder(children, root, &alive, &tree.cols.depth, tree.level);

        // The filters leave the parent of every shown node shown, so the shown nodes are the
        // ones a pre-order without the level would reach.
        let shown = tree.count_shown(&alive, threads);
        tree.stump = shown.num_dirs + shown.num_files + shown.num_links == 0;
        hidden.num_ignored = tree.cols.ignored;
        tree.count = shown + hidden;
        tree.last = topo::last_alive_child(children, &tree.order, &alive, threads);
        alive
    }

    /// Sorts the child lists of the directories shown (down to the display level), and with
    /// `scan` also returns the width facts of every node. The widths do not depend on the
    /// order, so large trees find them while the lists are sorted.
    fn sort_children(
        &self,
        ctx: &Context,
        children: &mut Children,
        scan: bool,
        threads: usize,
    ) -> Option<Stats> {
        let cap = self.level;
        let sorter = Sorter::new(self, ctx.sort, ctx.dir_order, Ties::Name);
        let overlap = scan && self.cols.len() >= OVERLAP_MIN && threads > 1;

        std::thread::scope(|s| {
            let job = overlap.then(|| s.spawn(|| self.scan(ctx, Scope::All, 1)));

            children.sort_each(
                threads,
                |p| self.cols.depth[ix(p)] < cap,
                |i| sorter.key(i),
                |a, b| sorter.cmp(a, b),
            );

            job.map_or_else(
                || scan.then(|| self.scan(ctx, Scope::All, threads)),
                |job| Some(job.join().unwrap_or_else(|e| std::panic::resume_unwind(e))),
            )
        })
    }

    /// The shown nodes below the root (`shown[i]` for node `i`), counted with up to `threads`
    /// threads.
    fn count_shown(&self, shown: &[bool], threads: usize) -> FileCount {
        let n = shown.len();
        let count = |range: std::ops::Range<usize>| {
            range
                .filter(|&i| shown[i] && i != ix(self.root))
                .fold(FileCount::default(), |acc, i| acc + self.node(id(i)))
        };

        if n < PAR_COUNT_MIN || threads < 2 {
            count(0..n)
        } else {
            crate::par::run_parts(0..threads, |k| count(crate::par::split(n, threads, k)))
                .into_iter()
                .fold(FileCount::default(), |acc, part| acc + part)
        }
    }

    /// Hard-link deduplication (see [`topo::dedupe`]): among entries sharing an inode identity
    /// (hard links, or a directory reached again through a followed symlink), one counts towards
    /// its directory's size: preferably one not reached through a symlink, then the least path.
    fn dedupe_hard_links(&mut self, threads: usize) -> Vec<u32> {
        let mut linked = std::mem::take(&mut self.cols.multi_linked);

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

    pub const fn root(&self) -> u32 {
        self.root
    }

    /// Column widths that fit the shown nodes.
    pub const fn widths(&self) -> &Widths {
        &self.widths
    }

    /// The merged styles addressed by [`Node::style`].
    pub fn styles(&self) -> &[nu_ansi_term::Style] {
        &self.cols.style_table
    }

    /// Shown nodes in display pre-order, root first, down to the display level (`--level`).
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

    #[inline]
    pub const fn node(&self, i: u32) -> Node<'_> {
        Node::new(self, i)
    }

    /// Whether `i` is the last shown child of its parent.
    #[inline]
    pub fn is_last(&self, i: u32) -> bool {
        self.last[ix(i)]
    }

    /// Parent of `i`; [`NONE`] for the root.
    #[inline]
    pub fn parent(&self, i: u32) -> u32 {
        self.cols.parent[ix(i)]
    }

    /// Nodes in the order the inverted flat layout (`iflat`) displays them, down to the display
    /// level; the flat layout displays the same list reversed.
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

        let rest = &self.order[1..];

        // Sort positions in `rest`, breaking ties by position to keep the stable order.
        let sorter = Sorter::new(self, ctx.sort, ctx.dir_order, Ties::Position(rest));
        let mut positions: Vec<u32> = (0..rest.len()).map(id).collect();

        psort::sort_keyed(&mut positions, ctx.threads, &|k| sorter.key(k), &|a, b| {
            sorter.cmp(a, b)
        });

        let mut ids = Vec::with_capacity(rest.len() + 1);
        ids.push(self.order[0]);
        ids.extend(positions.iter().map(|&k| rest[ix(k)]));
        ids
    }
}

/// Configures the walk of `root` from `ctx`, reading what `probe` asks for.
fn walker(ctx: &Context, root: &Path, probe: Probe) -> Result<Walker> {
    let canonical = std::fs::canonicalize(root)
        .map_err(|e| Error::DirNotFound(format!("{}: {e}", root.display())))?;

    if !canonical.is_dir() {
        return Err(Error::DirNotFound(format!(
            "{}: Not a directory",
            root.display()
        )));
    }

    let exclusions = filter::exclusions(&canonical, ctx.no_git, &ctx.exclude)?;
    let hidden = Hidden::new(&canonical, ctx.hidden, &ctx.include)?;
    let pattern = ctx
        .pattern
        .as_deref()
        .map(|pattern| Pattern::new(&canonical, pattern, ctx.glob, ctx.iglob))
        .transpose()?;

    let opts = rules::Options::new(hidden.hidden_by_rules(), !ctx.no_ignore);
    let cwd = std::env::current_dir().ok();
    let rules = rules::Rules::root(opts, exclusions, cwd);
    let filter = EntryFilter::new(hidden, pattern.map(|pattern| (pattern, ctx.file_type())));

    let threads = ctx.threads;

    Ok(Walker {
        root: canonical,
        rules,
        follow_links: ctx.follow,
        same_file_system: ctx.same_fs,
        // `--level` is display-only; only `--traversal-depth` limits the walk. Both count the
        // root as depth 0, so `--level N --traversal-depth N` shows the same entries.
        max_depth: ctx.traversal_depth,
        threads,
        // Without `--threads`, workers that wait for the file system get company.
        max_threads: walk::max_threads(threads, !ctx.threads_explicit),
        filter,
        count_ignored: ctx.report_ignored,
        fields: probe.fields(),
        tuning: if probe.reads_files() {
            walk::Tuning::reading_files()
        } else {
            walk::Tuning::default()
        },
    })
}

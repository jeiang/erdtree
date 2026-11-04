use crate::{
    context::{Context, column},
    disk_usage::file_size::FileSize,
    fs::inode::Inode,
    utils,
    walk::{Walker, rules},
};
use columns::{Collector, Columns, Probe};
use count::FileCount;
use error::Error;
use node::{Node, cmp::NodeComparator};
#[cfg(unix)]
use std::collections::HashMap;
use std::{
    convert::TryFrom, fs, path::PathBuf, result::Result as StdResult, sync::atomic::AtomicU64,
};
use topo::{Children, NONE, ix};

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

/// The walked file-system hierarchy, stored as columns indexed by node.
pub struct Tree {
    cols: Columns,
    root: u32,
    root_path: PathBuf,
    children: Children,

    /// Nodes still shown after `--prune`, `--dirs-only` and `--min-size`.
    alive: Vec<bool>,

    /// Alive nodes in display pre-order (children sorted).
    order: Vec<u32>,

    /// Whether a node is the last alive child of its parent.
    last: Vec<bool>,

    /// Byte-metric display settings, to rebuild [`FileSize`] values.
    size_format: SizeFormat,

    /// Owner and group names by id, for the long view.
    #[cfg(unix)]
    users: HashMap<u32, Option<String>>,
    #[cfg(unix)]
    groups: HashMap<u32, Option<String>>,

    /// Counts added to the summary: entries hidden by `--min-size`, and entries skipped by the
    /// walk (`--report-ignored`).
    hidden: FileCount,
}

/// How sizes are displayed.
#[derive(Clone, Copy)]
struct SizeFormat {
    disk_usage: crate::disk_usage::file_size::DiskUsage,
    human: bool,
    unit: crate::disk_usage::units::PrefixKind,
}

pub type Result<T> = StdResult<T, Error>;

impl Tree {
    /// Walks the root of `ctx` and builds the [Tree]; also computes the column widths that
    /// rendering needs into the returned [Context]. The tree may be a stump (see
    /// [`Self::is_stump`]). `progress` counts walked entries.
    pub fn try_init(mut ctx: Context, progress: Option<&AtomicU64>) -> Result<(Self, Context)> {
        let mut walker = walker(&ctx)?;
        let root_path = walker.root.clone();
        let probe = Probe::new(&ctx);
        walker.fields = probe.fields();

        let sinks = walker.run(|_| Collector::new(probe), progress);
        let cols = Columns::merge(sinks.into_iter().map(|c| c.columns).collect());

        let mut col_props = column::Properties::from(&ctx);
        let tree = Self::assemble(cols, root_path, &ctx, &mut col_props)?;

        ctx.update_column_properties(&col_props);

        if ctx.truncate {
            ctx.set_window_width();
        }

        Ok((tree, ctx))
    }

    /// Builds the topology, sizes, order and filters from the merged columns.
    fn assemble(
        mut cols: Columns,
        root_path: PathBuf,
        ctx: &Context,
        col_props: &mut column::Properties,
    ) -> Result<Self> {
        let n = cols.len();
        let root = cols
            .parent
            .iter()
            .position(|&p| p == NONE)
            .and_then(|r| u32::try_from(r).ok())
            .ok_or(Error::MissingRoot)?;

        cols.link_target.sort_unstable_by_key(|(i, _)| *i);

        let is_dir: Vec<bool> = cols.file_type.iter().map(|kind| kind.is_dir()).collect();
        let mut children = Children::build(&cols.parent);

        let mut tree = Self {
            root,
            root_path,
            children: Children::build(&[]),
            alive: vec![true; n],
            order: vec![],
            last: vec![],
            size_format: SizeFormat {
                disk_usage: ctx.disk_usage,
                human: ctx.human,
                unit: ctx.unit,
            },
            #[cfg(unix)]
            users: HashMap::new(),
            #[cfg(unix)]
            groups: HashMap::new(),
            hidden: FileCount::default(),
            cols,
        };

        // Sizes: each inode with several links counts once, in the directory that comes
        // first by path.
        let counted = tree.dedupe_hard_links();
        let all = topo::preorder(&children, root, &tree.alive);
        topo::aggregate(
            &all,
            &tree.cols.parent,
            &counted,
            &is_dir,
            &mut tree.cols.size,
        );

        #[cfg(unix)]
        tree.resolve_owners();

        for i in 0..n {
            tree.update_column_properties(col_props, ctx, i);
        }

        let comparator = node::cmp::comparator(ctx);
        {
            let view = &tree;
            children.sort_each(ctx.threads, |a, b| {
                let (a, b) = (view.node(a), view.node(b));
                comparator(a, b).then_with(|| a.file_name().cmp(b.file_name()))
            });
        }

        tree.children = children;

        if ctx.prune || ctx.pattern.is_some() {
            let order = topo::preorder(&tree.children, root, &tree.alive);
            topo::prune(&order, &tree.cols.parent, &is_dir, &mut tree.alive);
        }

        if ctx.dirs_only {
            topo::dirs_only(&tree.cols.parent, &is_dir, &mut tree.alive);
        }

        tree.order = topo::preorder(&tree.children, root, &tree.alive);

        if let Some(min_size) = ctx.min_size
            && min_size.value > 0
        {
            let hidden = topo::hide_small(
                &tree.order,
                &tree.cols.parent,
                &tree.cols.size,
                min_size.value,
                &mut tree.alive,
            );

            tree.hidden = hidden
                .into_iter()
                .fold(FileCount::default(), |acc, i| acc + tree.node(i));

            tree.order = topo::preorder(&tree.children, root, &tree.alive);

            // Hidden entries must not leave the size columns wider than needed.
            *col_props = column::Properties::from(ctx);
            for k in 0..tree.order.len() {
                tree.update_column_properties(col_props, ctx, ix(tree.order[k]));
            }
        }

        tree.hidden.num_ignored = tree.cols.ignored;
        tree.last = topo::last_alive_child(&tree.children, &tree.order, &tree.alive);

        Ok(tree)
    }

    /// Hard-link deduplication (see [`topo::dedupe`]): among entries sharing an inode (hard
    /// links, or a directory reached again through a followed symlink), one counts towards its
    /// directory's size: preferably one not reached through a symlink, then the least path.
    fn dedupe_hard_links(&self) -> Vec<bool> {
        let mut linked: Vec<(u32, Inode)> = self.cols.multi_linked.clone();

        topo::dedupe(self.cols.len(), &mut linked, |a, b| {
            let rank = |i| (self.through_symlink(i), self.path_of(i));
            rank(a).cmp(&rank(b))
        })
    }

    /// Whether node `i` or one of its ancestors is a symlink.
    fn through_symlink(&self, i: u32) -> bool {
        let mut cur = i;

        while cur != NONE {
            if self.cols.link_target(cur).is_some() {
                return true;
            }
            cur = self.cols.parent[ix(cur)];
        }

        false
    }

    /// Resolves the owner and group names of every entry that has long-view attributes.
    #[cfg(unix)]
    fn resolve_owners(&mut self) {
        for attrs in self.cols.long.iter().filter(|a| a.read) {
            self.users
                .entry(attrs.uid)
                .or_insert_with(|| crate::fs::ug::try_get_user(attrs.uid).ok());
            self.groups
                .entry(attrs.gid)
                .or_insert_with(|| crate::fs::ug::try_get_group(attrs.gid).ok());
        }
    }

    /// The summary count: every shown entry below the root, plus those hidden by `--min-size`
    /// or skipped by the walk.
    pub fn file_count(&self) -> FileCount {
        self.order
            .iter()
            .skip(1)
            .fold(self.hidden, |acc, &i| acc + self.node(i))
    }

    /// Returns `true` if there are no entries to show excluding the root.
    pub const fn is_stump(&self) -> bool {
        self.order.len() <= 1
    }

    /// The root node.
    pub const fn root(&self) -> u32 {
        self.root
    }

    /// Shown nodes in display pre-order, root first.
    pub fn order(&self) -> &[u32] {
        &self.order
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

    /// Whether `i` has a shown child.
    #[inline]
    pub fn has_children(&self, i: u32) -> bool {
        self.children.of(i).iter().any(|&c| self.alive[ix(c)])
    }

    /// Path of node `i` (the canonical root joined with the names below it).
    pub fn path_of(&self, i: u32) -> PathBuf {
        let mut names = vec![];
        let mut cur = i;

        while cur != self.root && cur != NONE {
            names.push(self.cols.name(ix(cur)));
            cur = self.cols.parent[ix(cur)];
        }

        let mut path = self.root_path.clone();
        path.extend(names.iter().rev());
        path
    }

    /// Path of node `i` relative to the root; the root's own name for the root.
    pub fn relative_path_of(&self, i: u32) -> PathBuf {
        if i == self.root {
            return PathBuf::from(self.cols.name(ix(i)));
        }

        let mut names = vec![];
        let mut cur = i;

        while cur != self.root && cur != NONE {
            names.push(self.cols.name(ix(cur)));
            cur = self.cols.parent[ix(cur)];
        }

        names.iter().rev().collect()
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
        let mut ids = self.order.clone();

        if ctx.sort.is_name() || ids.is_empty() {
            return ids;
        }

        let comparator: Box<NodeComparator> = node::cmp::comparator(ctx);
        ids[1..].sort_by(|&a, &b| comparator(self.node(a), self.node(b)));
        ids
    }

    /// Widens [`column::Properties`] to fit node `i`.
    fn update_column_properties(
        &self,
        col_props: &mut column::Properties,
        ctx: &Context,
        i: usize,
    ) {
        let node = self.node(u32::try_from(i).unwrap_or(NONE));

        if let Some(value) = node.size_value() {
            if ctx.byte_metric() && ctx.human {
                let file_size = node.file_size().unwrap_or_else(|| FileSize::from(ctx));
                let (size, unit) = human_widths(&file_size);
                col_props.max_size_width = col_props.max_size_width.max(size);
                col_props.max_size_unit_width = col_props.max_size_unit_width.max(unit);
            } else {
                col_props.max_size_width = col_props.max_size_width.max(utils::num_integral(value));
            }
        }

        #[cfg(unix)]
        if ctx.long {
            if let Some(owner) = node.owner() {
                col_props.max_owner_width = col_props.max_owner_width.max(owner.len());
            }

            if let Some(group) = node.group() {
                col_props.max_group_width = col_props.max_group_width.max(group.len());
            }

            if let Some(ino) = node.ino() {
                col_props.max_ino_width = col_props.max_ino_width.max(utils::num_integral(ino));
            }

            if let Some(nlink) = node.nlink() {
                col_props.max_nlink_width =
                    col_props.max_nlink_width.max(utils::num_integral(nlink));
            }

            if let Some(blocks) = node.blocks() {
                col_props.max_block_width =
                    col_props.max_block_width.max(utils::num_integral(blocks));
            }
        }
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

    Ok(Walker {
        root,
        rules,
        follow_links: ctx.follow,
        same_file_system: ctx.same_fs,
        // `--level` is display-only; only `--traversal-depth` limits the walk. Both count the
        // root as depth 0, so `--level N --traversal-depth N` shows the same entries.
        max_depth: ctx.traversal_depth,
        threads: if ctx.threads == 0 {
            std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get)
        } else {
            ctx.threads
        },
        filter,
        count_ignored: ctx.report_ignored,
        fields: crate::fs::stat::Fields::default(),
    })
}

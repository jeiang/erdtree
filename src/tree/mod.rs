use crate::{
    context::{Context, column},
    disk_usage::file_size::FileSize,
    fs::inode::Inode,
    progress::{IndicatorHandle, Message},
    utils,
};
use count::FileCount;
use error::Error;
use ignore::{WalkBuilder, WalkParallel};
use indextree::{Arena, NodeId};
use node::{Node, cmp::NodeComparator};
use std::{
    collections::{HashMap, HashSet},
    convert::TryFrom,
    fs,
    path::PathBuf,
    result::Result as StdResult,
    sync::mpsc::{self, Sender},
    thread,
};
use visitor::{BranchVisitorBuilder, TraversalState};

/// Operations to handle and display aggregate file counts based on their type.
pub mod count;

/// Errors related to traversal, [Tree] construction, and the like.
pub mod error;

/// Contains components of the [`Tree`] data structure that derive from [`ignore::DirEntry`].
pub mod node;

/// Custom visitor that operates on each thread during filesystem traversal.
mod visitor;

/// Virtual data structure that represents local file-system hierarchy.
pub struct Tree {
    arena: Arena<Node>,
    root_id: NodeId,

    /// Counts added to the summary: entries removed from `arena` by `--min-size`, and entries
    /// skipped by the walk (`--report-ignored`).
    hidden: FileCount,
}

pub type Result<T> = StdResult<T, Error>;

impl Tree {
    /// Constructor for [Tree].
    pub const fn new(arena: Arena<Node>, root_id: NodeId, hidden: FileCount) -> Self {
        Self {
            arena,
            root_id,
            hidden,
        }
    }

    /// The summary count: every entry below the root, plus those hidden by `--min-size` or
    /// skipped by the walk.
    pub fn file_count(&self) -> FileCount {
        self.root_id
            .descendants(&self.arena)
            .skip(1)
            .fold(self.hidden, |acc, id| acc + self.arena[id].get())
    }

    /// Initiates file-system traversal and [Tree] as well as updates the [Context] object with
    /// various properties necessary to render output. The tree may be a stump (see
    /// [`Self::is_stump`]).
    pub fn try_init(
        mut ctx: Context,
        indicator: Option<&IndicatorHandle>,
    ) -> Result<(Self, Context)> {
        let mut column_properties = column::Properties::from(&ctx);

        let (arena, root_id, hidden) = Self::traverse(&ctx, &mut column_properties, indicator)?;

        ctx.update_column_properties(&column_properties);

        if ctx.truncate {
            ctx.set_window_width();
        }

        Ok((Self::new(arena, root_id, hidden), ctx))
    }

    /// Returns `true` if there are no entries to show excluding the `root_id`.
    pub fn is_stump(&self) -> bool {
        self.root_id
            .descendants(self.arena())
            .skip(1)
            .peekable()
            .next()
            .is_none()
    }

    /// Grab a reference to `root_id`.
    pub const fn root_id(&self) -> NodeId {
        self.root_id
    }

    /// Grabs a reference to `arena`.
    pub const fn arena(&self) -> &Arena<Node> {
        &self.arena
    }

    /// Parallel traversal of the `root_id` directory and its contents. Parallel traversal relies on
    /// `WalkParallel`. Any filesystem I/O or related system calls are expected to occur during
    /// parallel traversal; post-processing post-processing of all directory entries should
    /// be completely CPU-bound.
    fn traverse(
        ctx: &Context,
        column_properties: &mut column::Properties,
        indicator: Option<&IndicatorHandle>,
    ) -> Result<(Arena<Node>, NodeId, FileCount)> {
        let walker = WalkParallel::try_from(ctx)?;
        let (tx, rx) = mpsc::channel();

        let progress_indicator_mailbox = indicator.map(IndicatorHandle::mailbox);

        thread::scope(|s| {
            let res = s.spawn(move || {
                let mut tree = Arena::new();
                let mut branches: HashMap<PathBuf, Vec<NodeId>> = HashMap::new();
                let mut root_id = None;
                let mut walked_dirs = vec![];

                while let Ok(TraversalState::Ongoing(node)) = rx.recv() {
                    if let Some(ref mailbox) = progress_indicator_mailbox
                        && mailbox.send(Message::Index).is_err()
                    {
                        return Err(Error::Terminated);
                    }

                    if node.is_dir() {
                        let node_path = node.path();

                        if !branches.contains_key(node_path) {
                            branches.insert(node_path.to_owned(), vec![]);
                        }

                        // Directories at the traversal limit were not read by the walk.
                        if ctx.report_ignored
                            && ctx.traversal_depth.is_none_or(|max| node.depth() < max)
                        {
                            walked_dirs.push(node_path.to_owned());
                        }

                        if node.depth() == 0 {
                            root_id = Some(tree.new_node(node));
                            continue;
                        }
                    }

                    let parent = node.parent_path().ok_or(Error::ExpectedParent)?.to_owned();

                    let node_id = tree.new_node(node);

                    if branches
                        .get_mut(&parent)
                        .map(|mut_ref| mut_ref.push(node_id))
                        .is_none()
                    {
                        branches.insert(parent, vec![]);
                    }
                }

                let root_id = root_id.ok_or(Error::MissingRoot)?;

                // Every on-disk entry of a walked directory that the walk did not report was
                // skipped by its filtering rules.
                let num_ignored = walked_dirs
                    .iter()
                    .map(|dir| {
                        let on_disk = fs::read_dir(dir).map_or(0, Iterator::count);
                        on_disk.saturating_sub(branches.get(dir).map_or(0, Vec::len))
                    })
                    .sum();

                let node_comparator = node::cmp::comparator(ctx);
                let mut inodes = HashSet::new();

                Self::assemble_tree(
                    &mut tree,
                    root_id,
                    &mut branches,
                    &node_comparator,
                    &mut inodes,
                    column_properties,
                    ctx,
                )?;

                if ctx.prune || ctx.pattern.is_some() {
                    Self::prune_directories(root_id, &mut tree);
                }

                if ctx.dirs_only {
                    Self::filter_directories(root_id, &mut tree);
                }

                let mut hidden = FileCount::default();

                if let Some(min_size) = ctx.min_size
                    && min_size.value > 0
                {
                    hidden = Self::hide_small_entries(root_id, &mut tree, min_size.value);

                    // Hidden entries must not leave the size columns wider than needed.
                    *column_properties = column::Properties::from(ctx);

                    for node_id in root_id.descendants(&tree) {
                        Self::update_column_properties(column_properties, tree[node_id].get(), ctx);
                    }
                }

                hidden.num_ignored = num_ignored;

                Ok((tree, root_id, hidden))
            });

            let mut visitor_builder = BranchVisitorBuilder::new(ctx, Sender::clone(&tx));

            walker.visit(&mut visitor_builder);

            let _ = tx.send(TraversalState::Done);

            res.join().unwrap()
        })
    }

    /// Takes the results of the parallel traversal and uses it to construct the [Tree] data
    /// structure. Sorting occurs if specified. The amount of columns needed to fit all of the disk
    /// usages is also computed here.
    fn assemble_tree(
        tree: &mut Arena<Node>,
        current_node_id: NodeId,
        branches: &mut HashMap<PathBuf, Vec<NodeId>>,
        node_comparator: &NodeComparator,
        inode_set: &mut HashSet<Inode>,
        column_properties: &mut column::Properties,
        ctx: &Context,
    ) -> Result<()> {
        let current_node = tree[current_node_id].get_mut();

        // Every directory gets exactly one branch entry, so a missing entry means the walker
        // reported the same directory more than once (upstream #260).
        let mut children = branches
            .remove(current_node.path())
            .ok_or_else(|| Error::DuplicateDirectory(current_node.path().to_path_buf()))?;

        let mut dir_size = FileSize::from(ctx);

        for child_id in &children {
            let index = *child_id;

            if tree[index].get().is_dir() {
                Self::assemble_tree(
                    tree,
                    index,
                    branches,
                    node_comparator,
                    inode_set,
                    column_properties,
                    ctx,
                )?;
            }

            let node = tree[index].get();

            Self::update_column_properties(column_properties, node, ctx);

            // If a hard-link is already accounted for then don't increment parent dir size.
            if let Some(inode) = node.inode()
                && inode.nlink > 1
                && !inode_set.insert(inode)
            {
                continue;
            }

            if let Some(file_size) = node.file_size() {
                dir_size += file_size;
            }
        }

        if dir_size.value() > 0 {
            let dir = tree[current_node_id].get_mut();

            dir.set_file_size(dir_size);
        }

        let dir = tree[current_node_id].get();

        Self::update_column_properties(column_properties, dir, ctx);

        children.sort_by(|&id_a, &id_b| {
            let node_a = tree[id_a].get();
            let node_b = tree[id_b].get();
            node_comparator(node_a, node_b)
        });

        // Append children to current node.
        for child_id in children {
            current_node_id.append(child_id, tree);
        }

        Ok(())
    }

    /// Function to remove empty directories.
    fn prune_directories(root_id: NodeId, tree: &mut Arena<Node>) {
        let to_prune = root_id
            .descendants(tree)
            .skip(1)
            .filter(|node_id| tree[*node_id].get().is_dir() && node_id.children(tree).count() == 0)
            .collect::<Vec<_>>();

        if to_prune.is_empty() {
            return;
        }

        for node_id in &to_prune {
            node_id.remove_subtree(tree);
        }

        Self::prune_directories(root_id, tree);
    }

    /// Filter `arena` for only directories.
    fn filter_directories(root_id: NodeId, tree: &mut Arena<Node>) {
        let to_detach = root_id
            .descendants(tree)
            .skip(1)
            .filter(|&descendant_id| !tree[descendant_id].get().is_dir())
            .collect::<Vec<_>>();

        if to_detach.is_empty() {
            return;
        }

        for node_id in &to_detach {
            node_id.detach(tree);
        }
    }

    /// Detaches every entry whose displayed size is below `threshold`, except directories that
    /// still contain a shown entry. A directory's size is the sum of its children, so this only
    /// matters for hard links that were counted in another directory. Returns the file count of
    /// the detached entries; the root is never hidden.
    fn hide_small_entries(root_id: NodeId, tree: &mut Arena<Node>, threshold: u64) -> FileCount {
        let mut shown = HashSet::new();
        let mut hidden = FileCount::default();

        let ids = root_id.descendants(tree).skip(1).collect::<Vec<_>>();

        // Pre-order reversed: every child is visited before its parent.
        for &id in ids.iter().rev() {
            let node = tree[id].get();

            if node.file_size().map_or(0, FileSize::value) >= threshold {
                shown.insert(id);
            }

            if shown.contains(&id) {
                if let Some(parent) = tree[id].parent() {
                    shown.insert(parent);
                }
            } else {
                hidden += node;
            }
        }

        for id in ids {
            if !shown.contains(&id) {
                id.detach(tree);
            }
        }

        hidden
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
    pub fn flat_order(&self, ctx: &Context) -> Vec<NodeId> {
        let mut ids = self.root_id.descendants(&self.arena).collect::<Vec<_>>();

        if ctx.sort.is_name() {
            return ids;
        }

        let comparator = node::cmp::comparator(ctx);
        ids[1..].sort_by(|&a, &b| comparator(self.arena[a].get(), self.arena[b].get()));
        ids
    }

    /// Updates [`column::Properties`] with provided [`Node`].
    fn update_column_properties(col_props: &mut column::Properties, node: &Node, ctx: &Context) {
        if let Some(file_size) = node.file_size() {
            if ctx.byte_metric() && ctx.human {
                let out = format!("{file_size}");
                let [size, unit]: [&str; 2] =
                    out.split(' ').collect::<Vec<&str>>().try_into().unwrap();

                let file_size_cols = size.len();
                let file_size_unit_cols = unit.len();

                if file_size_cols > col_props.max_size_width {
                    col_props.max_size_width = file_size_cols;
                }

                if file_size_unit_cols > col_props.max_size_unit_width {
                    col_props.max_size_unit_width = file_size_unit_cols;
                }
            } else {
                let file_size_cols = utils::num_integral(file_size.value());

                if file_size_cols > col_props.max_size_width {
                    col_props.max_size_width = file_size_cols;
                }
            }
        }

        #[cfg(unix)]
        if ctx.long {
            if let Some(owner) = node.owner() {
                let owner_len = owner.len();

                if owner_len > col_props.max_owner_width {
                    col_props.max_owner_width = owner_len;
                }
            }

            if let Some(group) = node.group() {
                let group_len = group.len();

                if group_len > col_props.max_group_width {
                    col_props.max_group_width = group_len;
                }
            }

            if let Some(ino) = node.ino() {
                let ino_num_integral = utils::num_integral(ino);

                if ino_num_integral > col_props.max_ino_width {
                    col_props.max_ino_width = ino_num_integral;
                }
            }

            if let Some(nlink) = node.nlink() {
                let nlink_num_integral = utils::num_integral(nlink);

                if nlink_num_integral > col_props.max_nlink_width {
                    col_props.max_nlink_width = nlink_num_integral;
                }
            }

            if let Some(blocks) = node.blocks() {
                let blocks_num_integral = utils::num_integral(blocks);

                if blocks_num_integral > col_props.max_block_width {
                    col_props.max_block_width = blocks_num_integral;
                }
            }
        }
    }
}

impl TryFrom<&Context> for WalkParallel {
    type Error = Error;

    fn try_from(ctx: &Context) -> StdResult<Self, Self::Error> {
        let root_id = fs::canonicalize(ctx.dir())
            .map_err(|e| Error::DirNotFound(format!("{}: {e}", ctx.dir().display())))?;

        if !root_id.is_dir() {
            return Err(Error::DirNotFound(format!(
                "{}: Not a directory",
                ctx.dir().display()
            )));
        }

        let mut builder = WalkBuilder::new(root_id);

        // With `--include`, hidden entries are filtered by `include_predicate` instead, as the
        // walker's own filter cannot make exceptions.
        let include_hidden = !ctx.hidden && !ctx.include.is_empty();

        builder
            .follow_links(ctx.follow)
            .git_ignore(!ctx.no_ignore)
            .git_global(!ctx.no_ignore)
            .hidden(!ctx.hidden && !include_hidden)
            .overrides(ctx.no_git_override()?)
            .same_file_system(ctx.same_fs)
            .threads(ctx.threads);

        // `--level` is display-only; only `--traversal-depth` limits the walk. Both count the
        // root as depth 0, so `--level N --traversal-depth N` shows the same entries.
        if let Some(depth) = ctx.traversal_depth {
            builder.max_depth(Some(depth));
        }

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

        if !filters.is_empty() {
            builder.filter_entry(move |dir_entry| filters.iter().all(|keep| keep(dir_entry)));
        }

        Ok(builder.build_parallel())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use clap::Parser;

    /// A directory reported twice by the walker must produce an error naming it, not a panic
    /// (upstream #260).
    #[test]
    fn duplicate_directory_is_an_error() {
        let ctx = Context::try_parse_from(["erd", "--no-config", "tests/data"]).unwrap();
        let mut entries = WalkBuilder::new("tests/data").max_depth(Some(1)).build();
        let root_entry = entries.next().unwrap().unwrap();
        let dir_entry = entries
            .map(StdResult::unwrap)
            .find(|e| e.path().ends_with("lipsum"))
            .unwrap();

        let mut tree = Arena::new();
        let root = Node::try_from((root_entry, &ctx)).unwrap();
        let root_path = root.path().to_path_buf();
        let root_id = tree.new_node(root);
        let dir = Node::try_from((dir_entry.clone(), &ctx)).unwrap();
        let dir_path = dir.path().to_path_buf();
        let first = tree.new_node(dir);
        let second = tree.new_node(Node::try_from((dir_entry, &ctx)).unwrap());

        let mut branches = HashMap::from([(root_path, vec![first, second]), (dir_path, vec![])]);

        let err = Tree::assemble_tree(
            &mut tree,
            root_id,
            &mut branches,
            &node::cmp::comparator(&ctx),
            &mut HashSet::new(),
            &mut column::Properties::from(&ctx),
            &ctx,
        )
        .unwrap_err();

        assert!(matches!(&err, Error::DuplicateDirectory(p) if p.ends_with("lipsum")));
    }
}

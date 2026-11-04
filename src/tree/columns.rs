//! Struct-of-arrays storage for walked entries. Every worker fills its own [`Columns`]; the
//! workers' columns are then concatenated. Hot data (names, parents, types, sizes) sits in dense
//! arrays; data only some options need (timestamps, colors, long-view attributes) is collected
//! only when those options are set, and rare data (symlink targets, multiply-linked inodes) is
//! kept sparse.

use super::topo::NONE;
#[cfg(unix)]
use crate::context::time;
use crate::{
    context::{Context, sort},
    disk_usage::file_size::{DiskUsage, line_count, word_count},
    fs::{
        inode::Inode,
        stat::{Fields, Kind, Stat},
    },
    styles::{entry::Resolver, get_ls_colors},
    walk::{NodeRef, Record, Sink},
};
use lscolors::Style as LsStyle;
use nu_ansi_term::Style;
use std::{
    collections::HashMap,
    ffi::OsStr,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// What to collect per entry, derived once from [`Context`].
#[derive(Clone, Copy)]
pub struct Probe {
    pub disk_usage: DiskUsage,
    pub suppress_size: bool,
    pub follow: bool,
    pub colors: bool,
    pub modified: bool,
    pub accessed: bool,
    pub created: bool,
    /// Depth up to which long-view owner, group and xattrs are read, if any.
    #[cfg(unix)]
    pub long_level: Option<usize>,
    /// Collect `ino` and `nlink` of every entry (long view).
    pub inodes: bool,
    /// Collect the mode of every entry: the long view shows it and `--classify` marks
    /// executables.
    #[cfg(unix)]
    pub mode: bool,
    /// Collect the allocated blocks of every entry (long view).
    #[cfg(unix)]
    pub blocks: bool,
}

impl Probe {
    /// The optional metadata fields the walk must read.
    pub const fn fields(self) -> Fields {
        Fields {
            modified: self.modified,
            accessed: self.accessed,
            created: self.created,
        }
    }

    pub fn new(ctx: &Context) -> Self {
        // Timestamps shown by the long view.
        #[cfg(unix)]
        let (long_mod, long_access, long_create) = match ctx.long.then(|| ctx.time()) {
            Some(time::Stamp::Mod) => (true, false, false),
            Some(time::Stamp::Access) => (false, true, false),
            Some(time::Stamp::Create) => (false, false, true),
            None => (false, false, false),
        };

        #[cfg(not(unix))]
        let (long_mod, long_access, long_create) = (false, false, false);

        let sorted_by = |a, b| ctx.sort == a || ctx.sort == b;

        Self {
            disk_usage: ctx.disk_usage,
            suppress_size: ctx.suppress_size,
            follow: ctx.follow,
            colors: get_ls_colors().is_ok(),
            modified: long_mod || sorted_by(sort::Type::Mod, sort::Type::Rmod),
            accessed: long_access || sorted_by(sort::Type::Access, sort::Type::Raccess),
            created: long_create || sorted_by(sort::Type::Create, sort::Type::Rcreate),
            #[cfg(unix)]
            long_level: ctx.long.then(|| ctx.level()),
            #[cfg(unix)]
            inodes: ctx.long,
            #[cfg(not(unix))]
            inodes: false,
            #[cfg(unix)]
            mode: ctx.long || ctx.classify,
            #[cfg(unix)]
            blocks: ctx.long,
        }
    }
}

/// Long-view attributes, read only for entries up to the display level.
#[cfg(unix)]
#[derive(Clone, Copy, Default)]
pub struct LongAttrs {
    /// Whether owner, group and xattrs were read for this entry.
    pub read: bool,
    pub has_xattrs: bool,
    pub uid: u32,
    pub gid: u32,
}

/// A run of consecutive entries of a sink that share a parent.
struct ParentRun {
    /// Index of the first entry of the run.
    start: u32,
    /// The parent, `None` for the root.
    parent: Option<NodeRef>,
}

/// Columns of entries. In a worker's sink `parent_runs` holds references into any sink; after
/// [`Columns::merge`] `parent` holds global indices.
#[derive(Default)]
pub struct Columns {
    names: Vec<u8>,
    name_end: Vec<u32>,
    /// The parents of a sink's entries. Entries of one directory are stored together, so there
    /// are about as many runs as directories instead of one reference per entry.
    parent_runs: Vec<ParentRun>,
    pub parent: Vec<u32>,
    pub depth: Vec<u32>,
    /// Type of the entry, or of its target when it is a followed symlink.
    pub file_type: Vec<Kind>,
    pub size: Vec<Option<u64>>,
    /// Mode bits; only with [`Probe::mode`].
    #[cfg(unix)]
    pub mode: Vec<u32>,
    /// Allocated blocks; only with [`Probe::blocks`].
    #[cfg(unix)]
    pub blocks: Vec<u64>,
    /// Inodes with more than one link, for hard-link deduplication.
    pub multi_linked: Vec<(u32, Inode)>,
    /// Inode of every entry; only with [`Probe::inodes`].
    pub inode: Vec<Option<Inode>>,
    /// Symlink targets.
    pub link_target: Vec<(u32, PathBuf)>,
    /// Index into `style_table` of the `LS_COLORS` style; only with [`Probe::colors`].
    style: Vec<u16>,
    /// The distinct styles, with the default style first.
    style_table: Vec<Style>,
    /// Read buffer reused for every file of the line and word counts.
    scratch: Vec<u8>,
    /// Resolves styles; only with [`Probe::colors`].
    resolver: Option<Resolver>,
    /// Index in `style_table` per `lscolors` style, by address, for the styles seen so far.
    style_index: HashMap<usize, u16>,
    /// The style address and index of the previous entry; neighbors mostly share a style.
    last_style: (usize, u16),
    pub modified: Vec<Option<SystemTime>>,
    pub accessed: Vec<Option<SystemTime>>,
    pub created: Vec<Option<SystemTime>>,
    #[cfg(unix)]
    pub long: Vec<LongAttrs>,
    /// Entries skipped by the walk in walked directories (`--report-ignored`).
    pub ignored: usize,
    probe: Option<Probe>,
}

/// Sink of one worker.
pub struct Collector {
    pub columns: Columns,
}

impl Collector {
    pub fn new(probe: Probe) -> Self {
        Self {
            columns: Columns {
                probe: Some(probe),
                resolver: probe
                    .colors
                    .then(|| get_ls_colors().ok().map(Resolver::new))
                    .flatten(),
                style_table: vec![Style::default()],
                ..Columns::default()
            },
        }
    }
}

impl Sink for Collector {
    fn push(&mut self, record: &Record<'_>) -> Option<u32> {
        self.columns.push(record)
    }

    fn add_ignored(&mut self, count: usize) {
        self.columns.ignored += count;
    }
}

impl Columns {
    /// Number of entries.
    pub const fn len(&self) -> usize {
        self.name_end.len()
    }

    /// Name of entry `i`.
    #[inline]
    pub fn name(&self, i: usize) -> &OsStr {
        let start = if i == 0 {
            0
        } else {
            self.name_end[i - 1] as usize
        };
        let bytes = &self.names[start..self.name_end[i] as usize];

        // SAFETY: `names` only holds bytes from `OsStr::as_encoded_bytes`, split at the
        // boundaries they were appended with.
        unsafe { OsStr::from_encoded_bytes_unchecked(bytes) }
    }

    /// Symlink target of entry `i`, if it is a symlink.
    pub fn link_target(&self, i: u32) -> Option<&Path> {
        self.link_target
            .binary_search_by_key(&i, |(j, _)| *j)
            .ok()
            .map(|k| self.link_target[k].1.as_path())
    }

    /// The `LS_COLORS` style of entry `i`, if styles were collected.
    pub fn style_at(&self, i: usize) -> Option<Style> {
        self.style.get(i).map(|&s| self.style_table[usize::from(s)])
    }

    /// The index in `style_table` of `style`, which `lscolors` returned (`None` is the default
    /// style).
    fn intern_style(&mut self, style: Option<&'static LsStyle>) -> u16 {
        let Some(style) = style else {
            return 0;
        };

        let address = std::ptr::from_ref(style) as usize;
        if self.last_style.0 == address {
            return self.last_style.1;
        }

        let index = if let Some(&index) = self.style_index.get(&address) {
            index
        } else {
            let index = u16::try_from(self.style_table.len()).unwrap_or(0);
            if index != 0 {
                self.style_table.push(style.to_nu_ansi_term_style());
            }
            self.style_index.insert(address, index);
            index
        };

        self.last_style = (address, index);
        index
    }

    fn push(&mut self, record: &Record<'_>) -> Option<u32> {
        let probe = self.probe?;
        let index = u32::try_from(self.len()).ok().filter(|&i| i != NONE)?;
        let name_end = u32::try_from(self.names.len() + record.name.len()).ok()?;
        let stat = record.stat;
        let path = record.path;
        let file_type = stat.kind;

        let size = size(probe, record, file_type, &mut self.scratch);

        if record.is_symlink
            && let Ok(target) = record.read_link()
        {
            self.link_target.push((index, target));
        }

        let inode = Inode::try_new(path, stat, record.is_symlink).ok();

        if let Some(inode) = inode.filter(|inode| inode.nlink > 1) {
            self.multi_linked.push((index, inode));
        }

        if probe.inodes {
            self.inode.push(inode);
        }

        if let Some(resolver) = self.resolver {
            let style = resolver.style(record.name, stat, || !record.target_exists());
            let style = self.intern_style(style);
            self.style.push(style);
        }

        if probe.modified {
            self.modified.push(stat.modified);
        }
        if probe.accessed {
            self.accessed.push(stat.accessed);
        }
        if probe.created {
            self.created.push(stat.created);
        }

        #[cfg(unix)]
        {
            if probe.mode {
                self.mode.push(stat.mode);
            }
            if probe.blocks {
                self.blocks.push(stat.blocks);
            }

            if let Some(level) = probe.long_level {
                let attrs = if record.depth <= level {
                    LongAttrs {
                        read: true,
                        has_xattrs: record.has_xattrs(),
                        uid: stat.uid,
                        gid: stat.gid,
                    }
                } else {
                    LongAttrs::default()
                };
                self.long.push(attrs);
            }
        }

        self.names.extend_from_slice(record.name.as_encoded_bytes());
        self.name_end.push(name_end);
        if self
            .parent_runs
            .last()
            .is_none_or(|run| run.parent != record.parent)
        {
            self.parent_runs.push(ParentRun {
                start: index,
                parent: record.parent,
            });
        }
        self.depth
            .push(u32::try_from(record.depth).unwrap_or(u32::MAX));
        self.file_type.push(file_type);
        self.size.push(size);

        Some(index)
    }

    /// The parents of all entries as global indices, for the sinks in `order`.
    fn global_parents(sinks: &[Self], order: &[usize], offsets: &[u32], total: u32) -> Vec<u32> {
        let mut parent = Vec::with_capacity(usize::try_from(total).unwrap_or(0));

        for &i in order {
            let sink = &sinks[i];

            for (k, run) in sink.parent_runs.iter().enumerate() {
                let end = sink
                    .parent_runs
                    .get(k + 1)
                    .map_or(sink.len(), |next| next.start as usize);
                let global = run.parent.map_or(NONE, |p| {
                    super::topo::global_index(offsets, p.sink, p.index)
                });
                parent.extend(std::iter::repeat_n(global, end - run.start as usize));
            }
        }

        parent
    }

    /// Appends the columns of `sink`, whose first entry gets the global index `offset`. Parents
    /// are not touched.
    fn append(&mut self, sink: &mut Self, offset: u32) {
        let merged = self;

        let names_offset = u32::try_from(merged.names.len()).unwrap_or(u32::MAX);
        merged.names.append(&mut sink.names);
        merged
            .name_end
            .extend(sink.name_end.iter().map(|end| end + names_offset));
        merged.depth.append(&mut sink.depth);
        merged.file_type.append(&mut sink.file_type);
        merged.size.append(&mut sink.size);
        merged.inode.append(&mut sink.inode);
        merged.modified.append(&mut sink.modified);
        merged.accessed.append(&mut sink.accessed);
        merged.created.append(&mut sink.created);
        merged.multi_linked.extend(
            sink.multi_linked
                .drain(..)
                .map(|(i, inode)| (i + offset, inode)),
        );
        merged.link_target.extend(
            sink.link_target
                .drain(..)
                .map(|(i, target)| (i + offset, target)),
        );
        merged.ignored += sink.ignored;

        // Styles are small ids into per-sink tables; translate them into the merged table.
        let remap: Vec<u16> = sink
            .style_table
            .iter()
            .map(|style| {
                let found = merged.style_table.iter().position(|s| s == style);
                let index = found.unwrap_or_else(|| {
                    merged.style_table.push(*style);
                    merged.style_table.len() - 1
                });
                u16::try_from(index).unwrap_or(0)
            })
            .collect();
        merged
            .style
            .extend(sink.style.iter().map(|&s| remap[usize::from(s)]));

        #[cfg(unix)]
        {
            merged.mode.append(&mut sink.mode);
            merged.blocks.append(&mut sink.blocks);
            merged.long.append(&mut sink.long);
        }
    }

    /// Concatenates the sinks and resolves parent references to global indices. The largest sink
    /// goes first and keeps its columns in place; the others are appended to them, in index
    /// order, into space reserved for the exact total.
    pub fn merge(mut sinks: Vec<Self>) -> Self {
        let Some(base) = (0..sinks.len()).max_by_key(|&i| sinks[i].len()) else {
            return Self::default();
        };

        let order: Vec<usize> = std::iter::once(base)
            .chain((0..sinks.len()).filter(|&i| i != base))
            .collect();

        let mut offsets = vec![0_u32; sinks.len()];
        let mut total = 0_u32;
        for &i in &order {
            offsets[i] = total;
            total = total.saturating_add(u32::try_from(sinks[i].len()).unwrap_or(NONE));
        }

        let parent = Self::global_parents(&sinks, &order, &offsets, total);

        // Every column grows once, to its final size.
        macro_rules! reserve {
            ($($col:ident),*) => {{
                $(
                    let sum: usize = sinks.iter().map(|s| s.$col.len()).sum();
                    let have = sinks[base].$col.len();
                    sinks[base].$col.reserve_exact(sum - have);
                )*
            }};
        }
        reserve!(
            names,
            name_end,
            depth,
            file_type,
            size,
            inode,
            style,
            modified,
            accessed,
            created,
            multi_linked,
            link_target
        );
        #[cfg(unix)]
        reserve!(mode, blocks, long);

        let probe = sinks[base].probe;
        let mut merged = std::mem::take(&mut sinks[base]);
        merged.probe = probe;
        merged.parent = parent;
        merged.parent_runs = vec![];

        for &i in order.iter().skip(1) {
            merged.append(&mut sinks[i], offsets[i]);
        }

        merged
    }
}

/// The size of an entry in the configured metric; `None` for entries without one (directories,
/// `--suppress-size`, files that are not text for line and word counts).
fn size(probe: Probe, record: &Record<'_>, file_type: Kind, scratch: &mut Vec<u8>) -> Option<u64> {
    let stat: &Stat = record.stat;

    // A followed symlink has its target's type here; an unfollowed one is a symlink.
    let sized = file_type.is_file() || file_type.is_symlink() && !probe.follow;

    if probe.suppress_size || !sized {
        return None;
    }

    match probe.disk_usage {
        DiskUsage::Logical => Some(stat.size),
        // Unix: allocated blocks, which is what `filesize` reports for a Unix entry.
        #[cfg(unix)]
        DiskUsage::Physical => Some(stat.blocks * 512),
        #[cfg(not(unix))]
        DiskUsage::Physical => Some(filesize::file_real_size(record.path).unwrap_or(stat.size)),
        DiskUsage::Line => {
            line_count::Metric::from_file(record.open().ok()?, scratch).map(|m| m.value)
        },
        DiskUsage::Word => {
            word_count::Metric::from_file(record.open().ok()?, scratch).map(|m| m.value)
        },
        #[cfg(unix)]
        DiskUsage::Block => Some(stat.blocks),
    }
}

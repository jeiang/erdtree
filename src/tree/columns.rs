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
    fs::inode::Inode,
    styles::get_ls_colors,
    walk::{NodeRef, Record, Sink},
};
use lscolors::Style as LsStyle;
use nu_ansi_term::Style;
use std::{
    ffi::OsStr,
    fs::{FileType, Metadata},
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
}

impl Probe {
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

/// Columns of entries. In a worker's sink `parent_ref` holds references into any sink; after
/// [`Columns::merge`] `parent` holds global indices.
#[derive(Default)]
pub struct Columns {
    names: Vec<u8>,
    name_end: Vec<usize>,
    parent_ref: Vec<Option<NodeRef>>,
    pub parent: Vec<u32>,
    pub depth: Vec<u32>,
    /// Type of the entry, or of its target when it is a followed symlink.
    pub file_type: Vec<FileType>,
    pub size: Vec<Option<u64>>,
    #[cfg(unix)]
    pub mode: Vec<u32>,
    #[cfg(unix)]
    pub blocks: Vec<u64>,
    /// Inodes with more than one link, for hard-link deduplication.
    pub multi_linked: Vec<(u32, Inode)>,
    /// Inode of every entry; only with [`Probe::inodes`].
    pub inode: Vec<Option<Inode>>,
    /// Symlink targets.
    pub link_target: Vec<(u32, PathBuf)>,
    /// `LS_COLORS` style; only with [`Probe::colors`].
    pub style: Vec<Style>,
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
        let start = if i == 0 { 0 } else { self.name_end[i - 1] };
        let bytes = &self.names[start..self.name_end[i]];

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

    fn push(&mut self, record: &Record<'_>) -> Option<u32> {
        let probe = self.probe?;
        let index = u32::try_from(self.len()).ok().filter(|&i| i != NONE)?;
        let md = record.metadata;
        let path = record.path;
        let file_type = md.file_type();

        let size = size(probe, record, file_type);

        if record.is_symlink
            && let Ok(target) = std::fs::read_link(path)
        {
            self.link_target.push((index, target));
        }

        let inode = Inode::try_new(path, md, record.is_symlink).ok();

        if let Some(inode) = inode.filter(|inode| inode.nlink > 1) {
            self.multi_linked.push((index, inode));
        }

        if probe.inodes {
            self.inode.push(inode);
        }

        if probe.colors
            && let Ok(ls_colors) = get_ls_colors()
        {
            let style = ls_colors
                .style_for_path_with_metadata(path, Some(md))
                .map_or_else(Style::default, LsStyle::to_nu_ansi_term_style);
            self.style.push(style);
        }

        if probe.modified {
            self.modified.push(md.modified().ok());
        }
        if probe.accessed {
            self.accessed.push(md.accessed().ok());
        }
        if probe.created {
            self.created.push(md.created().ok());
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;

            self.mode.push(md.mode());
            self.blocks.push(md.blocks());

            if let Some(level) = probe.long_level {
                let attrs = if record.depth <= level {
                    LongAttrs {
                        read: true,
                        has_xattrs: crate::fs::xattr::has_xattrs(path),
                        uid: md.uid(),
                        gid: md.gid(),
                    }
                } else {
                    LongAttrs::default()
                };
                self.long.push(attrs);
            }
        }

        self.names.extend_from_slice(record.name.as_encoded_bytes());
        self.name_end.push(self.names.len());
        self.parent_ref.push(record.parent);
        self.depth
            .push(u32::try_from(record.depth).unwrap_or(u32::MAX));
        self.file_type.push(file_type);
        self.size.push(size);

        Some(index)
    }

    /// Concatenates the sinks, in order, and resolves parent references to global indices.
    pub fn merge(mut sinks: Vec<Self>) -> Self {
        let mut offsets = Vec::with_capacity(sinks.len());
        let mut total = 0_u32;
        for sink in &sinks {
            offsets.push(total);
            total = total.saturating_add(u32::try_from(sink.len()).unwrap_or(NONE));
        }

        let mut merged = Self {
            probe: sinks.first().and_then(|s| s.probe),
            ..Self::default()
        };

        let n = usize::try_from(total).unwrap_or(0);
        merged.parent.reserve(n);
        merged.name_end.reserve(n);

        for (sink, &offset) in sinks.iter_mut().zip(&offsets) {
            let names_offset = merged.names.len();
            merged.names.append(&mut sink.names);
            merged
                .name_end
                .extend(sink.name_end.iter().map(|end| end + names_offset));
            merged.parent.extend(sink.parent_ref.iter().map(|p| {
                p.map_or(NONE, |p| {
                    super::topo::global_index(&offsets, p.sink, p.index)
                })
            }));
            merged.depth.append(&mut sink.depth);
            merged.file_type.append(&mut sink.file_type);
            merged.size.append(&mut sink.size);
            merged.inode.append(&mut sink.inode);
            merged.style.append(&mut sink.style);
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

            #[cfg(unix)]
            {
                merged.mode.append(&mut sink.mode);
                merged.blocks.append(&mut sink.blocks);
                merged.long.append(&mut sink.long);
            }
        }

        merged
    }
}

/// The size of an entry in the configured metric; `None` for entries without one (directories,
/// `--suppress-size`, files that are not text for line and word counts).
fn size(probe: Probe, record: &Record<'_>, file_type: FileType) -> Option<u64> {
    let md: &Metadata = record.metadata;

    // A followed symlink has its target's type here; an unfollowed one is a symlink.
    let sized = file_type.is_file() || file_type.is_symlink() && !probe.follow;

    if probe.suppress_size || !sized {
        return None;
    }

    match probe.disk_usage {
        DiskUsage::Logical => Some(md.len()),
        DiskUsage::Physical => {
            use filesize::PathExt;
            Some(record.path.size_on_disk_fast(md).unwrap_or(md.len()))
        },
        DiskUsage::Line => line_count::Metric::init(record.path).map(|m| m.value),
        DiskUsage::Word => word_count::Metric::init(record.path).map(|m| m.value),
        #[cfg(unix)]
        DiskUsage::Block => {
            use std::os::unix::fs::MetadataExt;
            Some(md.blocks())
        },
    }
}

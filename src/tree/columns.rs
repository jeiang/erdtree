//! Struct-of-arrays storage for walked entries. Every worker fills its own [`Columns`]; the
//! workers' columns are then concatenated. Hot data (names, parents, types, sizes) sits in dense
//! arrays; data only some options need (timestamps, colors, long-view attributes) is collected
//! only when those options are set, and rare data (symlink targets, multiply-linked inodes) is
//! kept sparse.

use super::topo::{NONE, Run};
#[cfg(unix)]
use crate::context::time;
use crate::{
    context::{Context, sort},
    disk_usage::file_size::{DiskUsage, line_count, word_count},
    fs::{
        inode::Inode,
        stat::{Fields, Kind, Stat},
    },
    styles::{self, entry::Resolver},
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
pub struct ParentRun {
    /// Index of the first entry of the run.
    start: u32,
    /// The parent, `None` for the root.
    parent: Option<NodeRef>,
}

/// Columns of entries. After [`Columns::merge`] `parent` holds global indices.
#[derive(Default)]
pub struct Columns {
    names: Vec<u8>,
    name_end: Vec<usize>,
    pub parent: Vec<u32>,
    /// After [`Columns::merge`], the runs of equal parents in `parent`, except those of the root.
    pub runs: Vec<Run>,
    pub depth: Vec<u32>,
    /// Type of the entry, or of its target when it is a followed symlink.
    pub file_type: Vec<Kind>,
    pub size: Vec<Option<u64>>,
    /// Mode bits; only with [`Probe::mode`].
    #[cfg(unix)]
    pub mode: Vec<u32>,
    /// Identities ([`Inode::id`]) of inodes with more than one link, for hard-link deduplication.
    pub multi_linked: Vec<(u32, (u64, u64))>,
    /// Inode of every entry; only with [`Probe::inodes`].
    pub inode: Vec<Option<Inode>>,
    /// Symlink targets.
    pub link_target: Vec<(u32, PathBuf)>,
    /// Index into `style_table` of the `LS_COLORS` style; only with color.
    style: Vec<u16>,
    /// The distinct styles, with the default style first.
    style_table: Vec<Style>,
    pub modified: Vec<Option<SystemTime>>,
    pub accessed: Vec<Option<SystemTime>>,
    pub created: Vec<Option<SystemTime>>,
    #[cfg(unix)]
    pub long: Vec<LongAttrs>,
    /// Entries skipped by the walk in walked directories (`--report-ignored`).
    pub ignored: usize,
}

/// Sink of one worker: the columns it fills, and the state only the walk needs.
pub struct Collector {
    cols: Columns,
    probe: Probe,
    /// The parents of the entries, as references into any sink. Entries of one directory are
    /// stored together, so there are about as many runs as directories instead of one
    /// reference per entry.
    parent_runs: Vec<ParentRun>,
    /// Read buffer reused for every file of the line and word counts.
    scratch: Vec<u8>,
    /// Resolves styles; only with color.
    resolver: Option<Resolver>,
    /// Index in `style_table` per `lscolors` style, by address, for the styles seen so far.
    style_index: HashMap<usize, u16>,
    /// The style address and index of the previous entry; neighbors mostly share a style.
    last_style: (usize, u16),
}

impl Collector {
    pub fn new(probe: Probe) -> Self {
        Self {
            cols: Columns {
                style_table: vec![Style::default()],
                ..Columns::default()
            },
            probe,
            parent_runs: vec![],
            scratch: vec![],
            resolver: styles::colors().map(|colors| Resolver::new(&colors.ls)),
            style_index: HashMap::new(),
            last_style: (0, 0),
        }
    }

    /// The collected columns and the parents of their entries, for [`Columns::merge`].
    pub fn into_columns(self) -> (Columns, Vec<ParentRun>) {
        (self.cols, self.parent_runs)
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
            let table = &mut self.cols.style_table;
            let index = u16::try_from(table.len()).unwrap_or(0);
            if index != 0 {
                table.push(style.to_nu_ansi_term_style());
            }
            self.style_index.insert(address, index);
            index
        };

        self.last_style = (address, index);
        index
    }
}

impl Sink for Collector {
    fn push(&mut self, record: &Record<'_>) -> Option<u32> {
        let probe = self.probe;
        let index = u32::try_from(self.cols.len()).ok().filter(|&i| i != NONE)?;
        let name_end = self.cols.names.len() + record.name.len();
        let stat = record.stat;
        let path = record.path;
        let file_type = stat.kind;

        let size = size(probe, record, file_type, &mut self.scratch);

        if record.is_symlink
            && let Ok(target) = record.read_link()
        {
            self.cols.link_target.push((index, target));
        }

        let inode = Inode::try_new(path, stat, record.is_symlink);

        // A directory can be reached again through a followed symlink even with one link, as
        // on btrfs, where directories always have one.
        if let Some(inode) =
            inode.filter(|inode| inode.nlink > 1 || probe.follow && file_type.is_dir())
        {
            self.cols.multi_linked.push((index, inode.id()));
        }

        if probe.inodes {
            self.cols.inode.push(inode);
        }

        if let Some(resolver) = self.resolver {
            let style = resolver.style(record.name, stat, || !record.target_exists());
            let style = self.intern_style(style);
            self.cols.style.push(style);
        }

        let cols = &mut self.cols;

        if probe.modified {
            cols.modified.push(stat.modified);
        }
        if probe.accessed {
            cols.accessed.push(stat.accessed);
        }
        if probe.created {
            cols.created.push(stat.created);
        }

        #[cfg(unix)]
        {
            if probe.mode {
                cols.mode.push(stat.mode);
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
                cols.long.push(attrs);
            }
        }

        cols.names.extend_from_slice(record.name.as_encoded_bytes());
        cols.name_end.push(name_end);
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
        cols.depth
            .push(u32::try_from(record.depth).unwrap_or(u32::MAX));
        cols.file_type.push(file_type);
        cols.size.push(size);

        Some(index)
    }

    fn add_ignored(&mut self, count: usize) {
        self.cols.ignored += count;
    }
}

impl Columns {
    /// Number of entries.
    pub const fn len(&self) -> usize {
        self.name_end.len()
    }

    /// Hints that the columns that a row of entry `i` reads are needed soon.
    #[inline]
    pub fn prefetch_cols(&self, i: usize) {
        use crate::render::bytes::prefetch;

        prefetch(self.name_end.get(i));
        prefetch(self.parent.get(i));
        prefetch(self.size.get(i));
    }

    /// Hints that the name of entry `i` is needed soon; after [`Self::prefetch_cols`].
    #[inline]
    pub fn prefetch_name(&self, i: usize) {
        if let Some(&end) = self.name_end.get(i) {
            crate::render::bytes::prefetch(self.names.get(end - 1));
        }
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

    /// The `LS_COLORS` style of entry `i`, if styles were collected.
    pub fn style_at(&self, i: usize) -> Option<Style> {
        self.style.get(i).map(|&s| self.style_table[usize::from(s)])
    }

    /// The parents of all entries as global indices, and the runs of equal ones. `runs[k]` and
    /// `lens[k]` are the parent runs and the length of the `k`-th sink to be concatenated;
    /// `offsets` are indexed by sink id.
    fn global_parents(
        runs: &[Vec<ParentRun>],
        lens: &[usize],
        offsets: &[u32],
        total: u32,
    ) -> (Vec<u32>, Vec<Run>) {
        let mut parent = Vec::with_capacity(usize::try_from(total).unwrap_or(0));
        let mut spans = Vec::with_capacity(runs.iter().map(Vec::len).sum());

        for (runs, &len) in runs.iter().zip(lens) {
            for (k, run) in runs.iter().enumerate() {
                let end = runs.get(k + 1).map_or(len, |next| next.start as usize);
                let global = run.parent.map_or(NONE, |p| {
                    super::topo::global_index(offsets, p.sink, p.index)
                });
                let start = u32::try_from(parent.len()).unwrap_or(NONE);
                parent.extend(std::iter::repeat_n(global, end - run.start as usize));

                if global != NONE {
                    spans.push(Run {
                        start,
                        end: u32::try_from(parent.len()).unwrap_or(NONE),
                        parent: global,
                    });
                }
            }
        }

        (parent, spans)
    }

    /// Concatenates the sinks and resolves the parent references of their parent runs to global
    /// indices. The largest sink goes first and keeps its columns in place; the others are
    /// appended to them, in index order, into space reserved for the exact total. Large merges
    /// copy independent groups of columns on separate threads, which also spreads the page
    /// faults of the new memory.
    // One job per group of columns; the groups share the sinks' set-up, so they stay together.
    #[allow(clippy::too_many_lines)]
    pub fn merge(sinks: Vec<(Self, Vec<ParentRun>)>) -> Self {
        let (mut sinks, mut parent_runs): (Vec<Self>, Vec<Vec<ParentRun>>) =
            sinks.into_iter().unzip();
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

        // Plain copies need no thread for one sink or a small tree.
        let parallel = sinks.len() > 1 && total >= MERGE_PAR_MIN;

        let lens: Vec<usize> = order.iter().map(|&i| sinks[i].len()).collect();
        let runs: Vec<Vec<ParentRun>> = order
            .iter()
            .map(|&i| std::mem::take(&mut parent_runs[i]))
            .collect();
        let name_offsets: Vec<usize> = order
            .iter()
            .scan(0, |at, &i| {
                let offset = *at;
                *at += sinks[i].names.len();
                Some(offset)
            })
            .collect();

        let mut merged = std::mem::take(&mut sinks[base]);
        let mut rest: Vec<Self> = order
            .iter()
            .skip(1)
            .map(|&i| std::mem::take(&mut sinks[i]))
            .collect();

        // The other sinks' column `$col`, moved out of them.
        macro_rules! others {
            ($col:ident) => {
                rest.iter_mut()
                    .map(|sink| std::mem::take(&mut sink.$col))
                    .collect::<Vec<_>>()
            };
        }

        let mut jobs: Vec<Box<dyn FnOnce() + Send + '_>> = vec![];

        // The biggest columns get a job each.
        let (dst, src) = (&mut merged.names, others!(names));
        jobs.push(Box::new(move || append_all(dst, src)));
        let (dst, src) = (&mut merged.size, others!(size));
        jobs.push(Box::new(move || append_all(dst, src)));

        // The small dense columns.
        {
            let (name_end, depth, file_type) = (
                &mut merged.name_end,
                &mut merged.depth,
                &mut merged.file_type,
            );
            let (ends, depths, types) = (others!(name_end), others!(depth), others!(file_type));
            let (dst, spans) = (&mut merged.parent, &mut merged.runs);
            let (runs, lens, offsets) = (&runs, &lens, &offsets);
            let name_offsets = &name_offsets[1..];

            jobs.push(Box::new(move || {
                (*dst, *spans) = Self::global_parents(runs, lens, offsets, total);
                append_all(depth, depths);
                append_all(file_type, types);
                name_end.reserve_exact(ends.iter().map(Vec::len).sum());
                for (ends, &offset) in ends.iter().zip(name_offsets) {
                    name_end.extend(ends.iter().map(|end| end + offset));
                }
            }));
        }

        // Columns only some options collect (the long view, time sorts) are empty otherwise.
        if rest.iter().any(|s| {
            !s.inode.is_empty()
                || !s.modified.is_empty()
                || !s.accessed.is_empty()
                || !s.created.is_empty()
        }) {
            let (inode, modified, accessed, created) = (
                &mut merged.inode,
                &mut merged.modified,
                &mut merged.accessed,
                &mut merged.created,
            );
            let (i, m, a, c) = (
                others!(inode),
                others!(modified),
                others!(accessed),
                others!(created),
            );
            jobs.push(Box::new(move || {
                append_all(inode, i);
                append_all(modified, m);
                append_all(accessed, a);
                append_all(created, c);
            }));
        }
        #[cfg(unix)]
        if rest
            .iter()
            .any(|s| !s.mode.is_empty() || !s.long.is_empty())
        {
            let (mode, long) = (&mut merged.mode, &mut merged.long);
            let (m, l) = (others!(mode), others!(long));
            jobs.push(Box::new(move || {
                append_all(mode, m);
                append_all(long, l);
            }));
        }

        // Sparse columns, and styles, which are small ids into per-sink tables to translate
        // into the merged table.
        {
            let (style, table) = (&mut merged.style, &mut merged.style_table);
            let styles: Vec<(Vec<u16>, Vec<Style>)> = rest
                .iter_mut()
                .map(|sink| {
                    (
                        std::mem::take(&mut sink.style),
                        std::mem::take(&mut sink.style_table),
                    )
                })
                .collect();
            let (multi, links) = (&mut merged.multi_linked, &mut merged.link_target);
            let sparse: Vec<_> = rest
                .iter_mut()
                .map(|sink| {
                    (
                        std::mem::take(&mut sink.multi_linked),
                        std::mem::take(&mut sink.link_target),
                    )
                })
                .collect();
            let offsets = order
                .iter()
                .skip(1)
                .map(|&i| offsets[i])
                .collect::<Vec<_>>();

            jobs.push(Box::new(move || {
                for (((s, t), (m, l)), &offset) in styles.into_iter().zip(sparse).zip(&offsets) {
                    multi.extend(m.into_iter().map(|(i, inode)| (i + offset, inode)));
                    links.extend(l.into_iter().map(|(i, target)| (i + offset, target)));

                    let remap: Vec<u16> = t
                        .iter()
                        .map(|style| {
                            let found = table.iter().position(|x| x == style);
                            let index = found.unwrap_or_else(|| {
                                table.push(*style);
                                table.len() - 1
                            });
                            u16::try_from(index).unwrap_or(0)
                        })
                        .collect();
                    style.extend(s.iter().map(|&s| remap[usize::from(s)]));
                }
            }));
        }

        run(jobs, parallel);

        merged.ignored += rest.iter().map(|sink| sink.ignored).sum::<usize>();
        merged
    }
}

/// Entries at least this many make [`Columns::merge`] copy columns on several threads.
const MERGE_PAR_MIN: u32 = 1 << 16;

/// Appends `src` to `dst`, growing it once.
fn append_all<T>(dst: &mut Vec<T>, src: Vec<Vec<T>>) {
    dst.reserve_exact(src.iter().map(Vec::len).sum());
    for mut part in src {
        dst.append(&mut part);
    }
}

/// Runs `jobs`, the first on this thread and the others on new ones if `parallel`.
fn run(jobs: Vec<Box<dyn FnOnce() + Send + '_>>, parallel: bool) {
    if !parallel {
        for job in jobs {
            job();
        }
        return;
    }

    std::thread::scope(|s| {
        let mut jobs = jobs.into_iter();
        let first = jobs.next();

        for job in jobs {
            s.spawn(job);
        }

        if let Some(job) = first {
            job();
        }
    });
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

    // The size of an unfollowed symlink is the length of its target path, not of the file read.
    let hint = if record.is_symlink { 0 } else { stat.size };

    match probe.disk_usage {
        DiskUsage::Logical => Some(stat.size),
        // Unix: allocated blocks, which is what `filesize` reports for a Unix entry.
        #[cfg(unix)]
        DiskUsage::Physical => Some(stat.blocks * 512),
        #[cfg(not(unix))]
        DiskUsage::Physical => Some(filesize::file_real_size(record.path).unwrap_or(stat.size)),
        DiskUsage::Line => line_count::from_file(record.open().ok()?, hint, scratch),
        DiskUsage::Word => word_count::from_file(record.open().ok()?, hint, scratch),
        #[cfg(unix)]
        DiskUsage::Block => Some(stat.blocks),
    }
}

#[cfg(test)]
mod test {
    use super::{Columns, ParentRun};
    use crate::{fs::stat::Kind, walk::NodeRef};
    use std::time::{Duration, SystemTime};

    /// The time recorded for entry `k` of sink `id`.
    fn stamp(id: usize, k: usize) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(u64::try_from(id * 1_000_000 + k).unwrap_or(0))
    }

    /// A sink of `len` entries below entry 0 of sink 0, each with its own number in the columns
    /// that only some options fill, with its parent runs.
    fn sink(id: usize, len: usize) -> (Columns, Vec<ParentRun>) {
        let cols = Columns {
            names: (0..len)
                .map(|k| b'a' + u8::try_from(k % 26).unwrap_or(0))
                .collect(),
            name_end: (1..=len).collect(),
            depth: vec![1; len],
            file_type: vec![Kind::File; len],
            size: (0..len)
                .map(|k| Some(u64::try_from(k).unwrap_or(0)))
                .collect(),
            created: (0..len).map(|k| Some(stamp(id, k))).collect(),
            modified: (0..len).map(|k| Some(stamp(id, k))).collect(),
            style_table: vec![nu_ansi_term::Style::default()],
            ..Columns::default()
        };
        let runs = vec![ParentRun {
            start: 0,
            parent: Some(NodeRef { sink: 0, index: 0 }),
        }];
        (cols, runs)
    }

    /// Merging puts the sinks' columns one after the other, the largest first, whether it copies
    /// on one thread or on several.
    #[test]
    fn merge_concatenates_every_column() {
        for lens in [[3, 5, 2], [40_000, 30_000, 50_000]] {
            let merged = Columns::merge((0..3).map(|id| sink(id, lens[id])).collect());
            let total: usize = lens.iter().sum();

            assert_eq!(merged.len(), total);
            assert_eq!(merged.parent.len(), total);
            assert_eq!(merged.runs.len(), 3);

            let first = (0..3).max_by_key(|&id| lens[id]).unwrap_or(0);
            let mut at = 0;
            for id in std::iter::once(first).chain((0..3).filter(|&id| id != first)) {
                for k in 0..lens[id] {
                    assert_eq!(merged.created[at], Some(stamp(id, k)));
                    assert_eq!(merged.modified[at], Some(stamp(id, k)));
                    assert_eq!(merged.size[at], Some(u64::try_from(k).unwrap_or(0)));
                    assert_eq!(
                        merged.name(at).as_encoded_bytes(),
                        [b'a' + u8::try_from(k % 26).unwrap_or(0)]
                    );
                    at += 1;
                }
            }

            // Entry 0 of sink 0 is where that sink starts in the merged columns.
            let start = if first == 0 { 0 } else { lens[first] };
            assert!(
                merged
                    .parent
                    .iter()
                    .all(|&p| usize::try_from(p) == Ok(start))
            );
        }
    }
}

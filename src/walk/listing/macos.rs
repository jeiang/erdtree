//! Listing with `getattrlistbulk`, or with `readdir`, chosen per directory.

use super::{
    Listing,
    fd::{self, Access, Opened},
};
use crate::{
    fs::{
        at, bulk,
        stat::{Fields, Kind, Stat},
    },
    walk::{Walker, fds::Budget, rules::Found},
};
use std::{
    ffi::{CStr, OsStr},
    os::{fd::RawFd, unix::ffi::OsStrExt},
    path::Path,
    sync::Arc,
};

/// How many files a directory must have listed before they tell how to list its subdirectories;
/// with fewer, they are listed as the directory was.
const DENSITY_SAMPLE: usize = 8;

/// A directory with this many times the entries that split it is listed with `readdir` when
/// several workers may share it.
const SHARED_DIR_FACTOR: u64 = 16;

/// Opens and lists directories for one worker.
pub struct Lister {
    /// The `getattrlistbulk` buffer.
    buf: Vec<u8>,
    /// Which file systems are APFS, for bulk reads.
    volumes: bulk::Volumes,
    /// The attributes a bulk read asks for.
    req: bulk::Request,
    budget: Arc<Budget>,
    /// With several workers, the link count from which a directory is listed with `readdir`.
    shared_links: Option<u64>,
}

impl Lister {
    /// The lister of a walk by `walker`, which [`Lister::for_worker`] copies.
    pub fn new(walker: &Walker) -> Self {
        Self {
            buf: Vec::new(),
            volumes: bulk::Volumes::default(),
            req: bulk::Request::new(walker.fields),
            budget: fd::budget(walker),
            shared_links: (walker.max_workers() > 1)
                .then(|| (walker.tuning.split_min as u64).saturating_mul(SHARED_DIR_FACTOR)),
        }
    }

    /// A lister for another worker of the same walk, with buffers of its own.
    pub fn for_worker(&self) -> Self {
        Self {
            buf: Vec::new(),
            volumes: bulk::Volumes::default(),
            req: self.req,
            budget: Arc::clone(&self.budget),
            shared_links: self.shared_links,
        }
    }

    /// Opens the directory at `path` as `access` says.
    pub fn open(&self, path: &Path, access: &Access) -> Option<Opened> {
        fd::open(path, access, &self.budget)
    }

    /// Whether listing uses the metadata of the directory itself: its link count, which on
    /// APFS counts its entries, tells whether workers may share them.
    pub const fn wants_dir_stat(&self) -> bool {
        self.shared_links.is_some()
    }

    /// Reads every entry of `dir` into `listing` and notes ignore-relevant names in `found`.
    /// The entries are read with `getattrlistbulk`, with the metadata of each entry but a
    /// directory, when the job asked for it, unless they are only counted (`count_only`) or
    /// workers may share them (by the link count in `stat`); else with `readdir`, names and
    /// types only. Entries listed before an I/O error are kept.
    pub fn fill(
        &mut self,
        listing: &mut Listing,
        dir: Opened,
        stat: Option<&Stat>,
        count_only: bool,
        found: &mut Found,
    ) {
        // Contents of another file system are only counted, which needs no metadata. Neither is
        // a bulk read worth it for a directory large enough that idle workers share its entries:
        // it is serial, while their `fstatat` calls are not.
        let shared = self
            .shared_links
            .is_some_and(|min| stat.is_some_and(|stat| stat.nlink >= min));
        let bulk = dir.bulk && !count_only && !shared;

        let fd = dir.dir.as_raw_fd();
        listing.bulk = dir.bulk;
        listing.dir = Some(Arc::new(dir.dir));

        if !bulk {
            fill_readdir(listing, fd, found);
            return;
        }

        if self.buf.len() < bulk::BUF_LEN {
            self.buf.resize(bulk::BUF_LEN, 0);
        }

        let mut full = false;
        let mut seen = 0;

        while !full {
            let read = bulk::read(fd, &self.req, &mut self.volumes, &mut self.buf, |entry| {
                if full {
                    return;
                }
                // Listed, but `lstat` of it would fail: like an entry whose metadata cannot be
                // read, it counts as skipped.
                let Some(entry) = entry else {
                    listing.unusable += 1;
                    return;
                };

                if entry.name[0] == b'.' {
                    found.note(OsStr::from_bytes(entry.name));
                }

                full = !listing.push(entry.name, entry.kind);
                if !full {
                    listing.known.push(entry.stat);
                }
            });

            match read {
                Ok(n) if n > 0 => seen += n,
                // A directory that can be read but not searched lists with `readdir` only.
                Err(e) if seen == 0 && e.raw_os_error() == Some(libc::EACCES) => {
                    fill_readdir(listing, fd, found);
                    break;
                },
                _ => break,
            }
        }
    }
}

/// Lists the directory open as `fd` with `readdir`: names and types, without metadata.
fn fill_readdir(listing: &mut Listing, fd: RawFd, found: &mut Found) {
    // SAFETY: `fd` is open; the duplicate is owned by the stream from here on.
    let dup = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if dup < 0 {
        return;
    }
    // SAFETY: `dup` is an open directory descriptor.
    let stream = unsafe { libc::fdopendir(dup) };
    if stream.is_null() {
        // SAFETY: `dup` is open and owned here.
        unsafe { libc::close(dup) };
        return;
    }

    loop {
        // SAFETY: `stream` is an open directory stream.
        let ent = unsafe { libc::readdir(stream) };
        if ent.is_null() {
            break;
        }
        // SAFETY: `readdir` returned a valid entry, whose name is NUL-terminated.
        let (name, d_type) = unsafe { (CStr::from_ptr((*ent).d_name.as_ptr()), (*ent).d_type) };
        let name = name.to_bytes();
        if name == b"." || name == b".." {
            continue;
        }

        if name.first() == Some(&b'.') {
            found.note(OsStr::from_bytes(name));
        }
        if !listing.push(name, Kind::from_dtype(d_type)) {
            break;
        }
    }

    // SAFETY: `stream` is open and owned here.
    unsafe { libc::closedir(stream) };
}

impl Listing {
    /// Metadata of entry `i` without following a symlink: what the listing gave, else `fstatat`.
    pub fn lstat(&self, i: usize, fields: Fields) -> Option<Stat> {
        if let Some(&Some(stat)) = self.known.get(i) {
            return Some(stat);
        }

        at::stat_at(self.at(i)?, false, fields).ok()
    }
}

/// Counts the files of a directory and how many of them were recorded, to choose how its
/// subdirectories are listed.
pub struct Sample {
    files: usize,
    recorded: usize,
}

impl Sample {
    /// An empty sample.
    pub const fn new() -> Self {
        Self {
            files: 0,
            recorded: 0,
        }
    }

    /// Counts an entry of the reported type `kind`, and whether the walk recorded it (which
    /// needs its metadata).
    pub fn add(&mut self, kind: Option<Kind>, recorded: bool) {
        let file = kind.is_none_or(|kind| !kind.is_dir());
        self.files += usize::from(file);
        self.recorded += usize::from(file && recorded);
    }

    /// Sets how the subdirectories queued with `jobs` are listed. A bulk read costs more per
    /// entry than `readdir` but saves an `fstatat` per recorded file: it pays when about a third
    /// of the files are recorded. The files of the parent tell; too few of them leave the
    /// subdirectories to be listed as the parent was.
    pub fn apply<'a>(self, jobs: impl Iterator<Item = &'a mut Access>) {
        if self.files < DENSITY_SAMPLE {
            return;
        }

        let bulk = self.recorded * 3 >= self.files;
        for access in jobs {
            access.bulk = bulk;
        }
    }
}

//! The entries of one directory, read in one go. Names live in a single arena (each followed by a
//! NUL, so a name is also a C string), so listing allocates nothing per entry. A worker reuses
//! its [`Listing`] from directory to directory, and lists with its own [`Lister`], which holds
//! the buffers and state of the platform's way to read a directory:
//! - Linux (`linux`): `getdents64`, entries then stat'ed with `statx` relative to the open
//!   directory.
//! - macOS (`macos`): `getattrlistbulk`, which returns the metadata of the entries with their
//!   names, or `readdir`, chosen per directory.
//! - Elsewhere (`other`): `std::fs::read_dir`.
//!
//! Every platform has the same interface: [`Lister::open`] opens the directory of a job as its
//! [`Access`] says, [`Lister::fill`] lists it, and [`Listing::access`] tells how to open a
//! subdirectory. [`Sample`] lets the entries a directory recorded choose how its subdirectories
//! are listed.

use crate::fs::stat::Kind;
use std::ffi::OsStr;

#[cfg(fd_walk)]
use super::fds::DirFd;
#[cfg(fd_walk)]
use std::sync::Arc;

/// The walk relative to directory descriptors, shared by Linux and macOS.
#[cfg(fd_walk)]
mod fd;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(fd_walk))]
mod other;

#[cfg(fd_walk)]
pub use fd::{Access, lstat_dir};
#[cfg(target_os = "linux")]
pub use linux::Lister;
#[cfg(target_os = "macos")]
pub use macos::{Lister, Sample};
#[cfg(not(fd_walk))]
pub use other::{Access, Lister};

/// One listed entry.
#[derive(Clone, Copy)]
struct Ent {
    /// Inode number, for visiting a large directory in inode order.
    #[cfg(target_os = "linux")]
    ino: u64,
    /// Offset of the name in the arena.
    off: u32,
    /// Length of the name, without its NUL.
    len: u16,
    /// The type the directory reported; `None` when it did not.
    kind: Option<Kind>,
}

/// The entries of one directory.
#[derive(Default)]
pub struct Listing {
    names: Vec<u8>,
    ents: Vec<Ent>,
    /// Entries the directory listed that cannot be used (the read failed for them).
    pub unusable: usize,
    /// The open directory, which `statx` of the entries is relative to.
    #[cfg(fd_walk)]
    dir: Option<Arc<DirFd>>,
    /// Metadata of each entry without following a symlink, as far as it came with the listing
    /// (never for a directory).
    #[cfg(target_os = "macos")]
    known: Vec<Option<crate::fs::stat::Stat>>,
    /// Whether the job of the directory asked for a bulk read, which its subdirectories inherit
    /// unless its entries tell otherwise (see [`Sample`]).
    #[cfg(target_os = "macos")]
    bulk: bool,
    #[cfg(not(fd_walk))]
    std: Vec<std::fs::DirEntry>,
}

impl Listing {
    /// Number of usable entries.
    pub const fn len(&self) -> usize {
        self.ents.len()
    }

    /// Empties the listing, keeping its buffers.
    pub fn clear(&mut self) {
        self.names.clear();
        self.ents.clear();
        self.unusable = 0;
        #[cfg(fd_walk)]
        {
            self.dir = None;
        }
        #[cfg(target_os = "macos")]
        self.known.clear();
        #[cfg(not(fd_walk))]
        self.std.clear();
    }

    /// Name of entry `i`.
    pub fn name(&self, i: usize) -> &[u8] {
        let e = &self.ents[i];
        let start = e.off as usize;
        &self.names[start..start + usize::from(e.len)]
    }

    /// Name of entry `i`.
    pub fn os_name(&self, i: usize) -> &OsStr {
        // SAFETY: the arena only holds bytes from `OsStr::as_encoded_bytes`, split at the
        // boundaries they were appended with.
        unsafe { OsStr::from_encoded_bytes_unchecked(self.name(i)) }
    }

    /// The type of entry `i` the directory reported, if it did.
    pub fn kind(&self, i: usize) -> Option<Kind> {
        self.ents[i].kind
    }

    fn push(
        &mut self,
        name: &[u8],
        #[cfg(target_os = "linux")] ino: u64,
        kind: Option<Kind>,
    ) -> bool {
        let (Ok(off), Ok(len)) = (u32::try_from(self.names.len()), u16::try_from(name.len()))
        else {
            return false;
        };
        // The next offset must fit too, or the listing stops here.
        if u32::try_from(self.names.len() + name.len() + 1).is_err() {
            return false;
        }

        self.names.extend_from_slice(name);
        self.names.push(0);
        self.ents.push(Ent {
            #[cfg(target_os = "linux")]
            ino,
            off,
            len,
            kind,
        });
        true
    }
}

/// Counts the entries of a directory to choose how its subdirectories are listed; only macOS
/// chooses (see `macos::Sample`).
#[cfg(not(target_os = "macos"))]
pub struct Sample;

// The same interface as the macOS sample keeps the worker free of platform cases.
#[cfg(not(target_os = "macos"))]
#[allow(
    clippy::unused_self,
    clippy::needless_pass_by_value,
    clippy::needless_pass_by_ref_mut
)]
impl Sample {
    /// An empty sample.
    pub const fn new() -> Self {
        Self
    }

    /// Counts an entry of the reported type `kind`, and whether the walk recorded it.
    pub const fn add(&mut self, _kind: Option<Kind>, _recorded: bool) {}

    /// Sets how the subdirectories queued with `jobs` are listed.
    pub fn apply<'a>(self, _jobs: impl Iterator<Item = &'a mut Access>) {}
}

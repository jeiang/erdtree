//! Listing with `getdents64`.

use super::{
    Listing,
    fd::{self, Access, Opened},
};
use crate::{
    fs::{
        at,
        stat::{Fields, Stat},
    },
    walk::{Walker, fds::Budget, rules::Found},
};
use std::{ffi::OsStr, os::unix::ffi::OsStrExt, path::Path, sync::Arc};

/// Directories with at least this many entries are visited in inode order (when that helps).
const INODE_ORDER_MIN: usize = 10_000;

/// Size of the `getdents64` buffer. Larger buffers only help directories of thousands of
/// entries.
const DIRENT_BUF: usize = 32 * 1024;

/// Opens and lists directories for one worker.
pub struct Lister {
    buf: Vec<u8>,
    budget: Arc<Budget>,
}

impl Lister {
    /// The lister of a walk by `walker`, which [`Lister::for_worker`] copies.
    pub fn new(walker: &Walker) -> Self {
        Self {
            buf: Vec::new(),
            budget: fd::budget(walker),
        }
    }

    /// A lister for another worker of the same walk, with buffers of its own.
    pub fn for_worker(&self) -> Self {
        Self {
            buf: Vec::new(),
            budget: Arc::clone(&self.budget),
        }
    }

    pub fn open(&self, path: &Path, access: &Access) -> Option<Opened> {
        fd::open(path, access, &self.budget)
    }

    /// Whether listing uses the metadata of the directory itself; only on macOS.
    // The macOS lister decides at run time.
    #[allow(clippy::unused_self)]
    pub const fn wants_dir_stat(&self) -> bool {
        false
    }

    /// Reads every entry of `dir` into `listing` with `getdents64`, and notes ignore-relevant
    /// names in `found`. Entries listed before an I/O error are kept. The metadata of the
    /// directory and whether its entries are only counted matter on macOS only.
    pub fn fill(
        &mut self,
        listing: &mut Listing,
        dir: Opened,
        _stat: Option<&Stat>,
        _count_only: bool,
        found: &mut Found,
    ) {
        let buf = &mut self.buf;
        if buf.len() < DIRENT_BUF {
            buf.resize(DIRENT_BUF, 0);
        }

        let fd = dir.dir.as_raw_fd();

        'read: while let Ok(n) = at::getdents(fd, buf) {
            if n == 0 {
                break;
            }

            let mut full = false;
            at::for_each_dirent(buf, n, |entry| {
                // `.` and `..`, and entries removed from the directory (inode 0), are not listed
                // by `readdir`.
                if full || entry.ino == 0 || matches!(entry.name, b"." | b".." | b"") {
                    return;
                }

                if entry.name[0] == b'.' {
                    found.note(OsStr::from_bytes(entry.name));
                }

                full = !listing.push(entry.name, entry.ino, entry.kind);
            });

            if full {
                break 'read;
            }
        }

        if listing.ents.len() >= INODE_ORDER_MIN && !at::inode_order_is_useless(fd) {
            // Stat the entries in the order of their inodes: close inodes share a block of the
            // inode table, so a cold cache reads each block once instead of at random.
            listing.ents.sort_unstable_by_key(|e| e.ino);
        }

        listing.dir = Some(Arc::new(dir.dir));
    }
}

impl Listing {
    /// Metadata of entry `i` without following a symlink.
    pub fn lstat(&self, i: usize, fields: Fields) -> Option<Stat> {
        at::stat_at(self.at(i)?, false, fields).ok()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::{collections::BTreeSet, fs};

    /// Lists the directory at `path`.
    fn list(path: &Path) -> (Listing, Found) {
        let mut lister = Lister {
            buf: Vec::new(),
            budget: Budget::new(1024, 1),
        };
        let mut listing = Listing::default();
        let mut found = Found::default();
        let dir = lister.open(path, &Access::root()).unwrap();
        lister.fill(&mut listing, dir, None, false, &mut found);
        (listing, found)
    }

    #[test]
    fn lists_what_read_dir_lists_in_inode_order_when_large() {
        let dir = tempfile::tempdir().unwrap();

        for i in 0..INODE_ORDER_MIN + 50 {
            fs::write(dir.path().join(format!("f{i}")), "").unwrap();
        }
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join(".gitignore"), "").unwrap();

        let (listing, found) = list(dir.path());

        assert!(found.has_git_ignore && !found.has_git_dir);

        let got: BTreeSet<Vec<u8>> = (0..listing.len())
            .map(|i| listing.name(i).to_vec())
            .collect();
        let want: BTreeSet<Vec<u8>> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_encoded_bytes())
            .collect();
        assert_eq!(got, want);
        assert_eq!(listing.len(), INODE_ORDER_MIN + 52);

        let fd = listing.dir.as_ref().unwrap().as_raw_fd();
        let inodes: Vec<u64> = (0..listing.len()).map(|i| listing.ents[i].ino).collect();

        if at::inode_order_is_useless(fd) {
            // tmpfs, FUSE, network file systems: the order of the directory is kept.
            return;
        }
        assert!(
            inodes.is_sorted(),
            "large directories are visited in inode order"
        );

        // Entry and name still belong together after sorting.
        for i in 0..listing.len() {
            let md = fs::symlink_metadata(dir.path().join(listing.os_name(i))).unwrap();
            assert_eq!(
                std::os::unix::fs::MetadataExt::ino(&md),
                listing.ents[i].ino
            );
        }
    }
}

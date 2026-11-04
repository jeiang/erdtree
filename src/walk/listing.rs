//! The entries of one directory, read in one go. Names live in a single arena (each followed by a
//! NUL, so a name is also a C string), so listing allocates nothing per entry. A worker reuses
//! its [`Listing`] from directory to directory.

use super::rules::Found;
use crate::fs::stat::{Fields, Kind, Stat};
use std::ffi::OsStr;

#[cfg(target_os = "linux")]
use crate::fs::at::{self, At};
#[cfg(target_os = "linux")]
use std::{
    ffi::CStr,
    os::{
        fd::{AsRawFd, OwnedFd},
        unix::ffi::OsStrExt,
    },
    path::Path,
    sync::Arc,
};
#[cfg(not(target_os = "linux"))]
use std::{fs, path::Path};

/// Directories with at least this many entries are visited in inode order (when that helps).
#[cfg(target_os = "linux")]
const INODE_ORDER_MIN: usize = 10_000;

/// Size of the `getdents64` buffer. Larger buffers only help directories of thousands of
/// entries.
#[cfg(target_os = "linux")]
pub const DIRENT_BUF: usize = 32 * 1024;

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
    #[cfg(target_os = "linux")]
    pub dir: Option<Arc<OwnedFd>>,
    #[cfg(not(target_os = "linux"))]
    std: Vec<fs::DirEntry>,
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
        #[cfg(target_os = "linux")]
        {
            self.dir = None;
        }
        #[cfg(not(target_os = "linux"))]
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

#[cfg(target_os = "linux")]
impl Listing {
    /// Reads every entry of `dir` with `getdents64` through `buf`, and notes ignore-relevant names
    /// in `found`. Entries listed before an I/O error are kept.
    pub fn fill(&mut self, dir: Arc<OwnedFd>, buf: &mut Vec<u8>, found: &mut Found) {
        if buf.len() < DIRENT_BUF {
            buf.resize(DIRENT_BUF, 0);
        }

        let fd = dir.as_raw_fd();

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

                full = !self.push(entry.name, entry.ino, entry.kind);
            });

            if full {
                break 'read;
            }
        }

        if self.ents.len() >= INODE_ORDER_MIN && !at::inode_order_is_useless(fd) {
            // Stat the entries in the order of their inodes: close inodes share a block of the
            // inode table, so a cold cache reads each block once instead of at random.
            self.ents.sort_unstable_by_key(|e| e.ino);
        }

        self.dir = Some(dir);
    }

    /// Entry `i` as the target of a relative system call.
    pub fn at(&self, i: usize) -> Option<At<'_>> {
        let e = &self.ents[i];
        let start = e.off as usize;
        let bytes = &self.names[start..=start + usize::from(e.len)];

        // SAFETY: `push` wrote a NUL after the name, and a name has no NUL inside.
        let name = unsafe { CStr::from_bytes_with_nul_unchecked(bytes) };

        Some(At {
            dir: self.dir.as_ref()?.as_raw_fd(),
            name,
        })
    }

    /// Metadata of entry `i` without following a symlink.
    pub fn lstat(&self, i: usize, _path: &Path, fields: Fields) -> Option<Stat> {
        at::stat_at(self.at(i)?, false, fields).ok()
    }

    /// Metadata of entry `i`, following a symlink.
    pub fn stat(&self, i: usize, _path: &Path, fields: Fields) -> Option<Stat> {
        at::stat_at(self.at(i)?, true, fields).ok()
    }

    /// Whether entry `i` is hidden.
    pub fn is_hidden(&self, i: usize) -> bool {
        self.name(i).first() == Some(&b'.')
    }
}

#[cfg(not(target_os = "linux"))]
impl Listing {
    /// Reads every entry of the directory at `path`. Returns `false` when it cannot be read.
    pub fn fill(&mut self, path: &Path, found: &mut Found) -> bool {
        let Ok(read_dir) = fs::read_dir(path) else {
            return false;
        };

        for entry in read_dir {
            let Ok(entry) = entry else {
                self.unusable += 1;
                continue;
            };

            let name = entry.file_name();
            found.note(&name);

            // Free on Unix (`d_type`) and Windows (find data).
            let Ok(file_type) = entry.file_type() else {
                self.unusable += 1;
                continue;
            };

            if self.push(name.as_encoded_bytes(), Some(file_type.into())) {
                self.std.push(entry);
            }
        }

        true
    }

    /// Metadata of entry `i` without following a symlink.
    pub fn lstat(&self, i: usize, _path: &Path, fields: Fields) -> Option<Stat> {
        Some(Stat::from_metadata(&self.std[i].metadata().ok()?, fields))
    }

    /// Metadata of entry `i`, following a symlink.
    // Same signature as on Linux, where the entry is looked up relative to the open directory.
    #[allow(clippy::unused_self)]
    pub fn stat(&self, _i: usize, path: &Path, fields: Fields) -> Option<Stat> {
        Some(Stat::from_metadata(&fs::metadata(path).ok()?, fields))
    }

    /// Whether entry `i` is hidden: its name starts with `.` or, on Windows, it has the hidden
    /// attribute.
    pub fn is_hidden(&self, i: usize) -> bool {
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;

            const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;

            // Free on Windows: the attributes come with the directory listing.
            if self.std[i]
                .metadata()
                .is_ok_and(|md| md.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0)
            {
                return true;
            }
        }

        self.name(i).first() == Some(&b'.')
    }
}

#[cfg(all(test, target_os = "linux"))]
mod test {
    use super::*;
    use crate::fs::at;
    use std::{collections::BTreeSet, fs};

    #[test]
    fn lists_what_read_dir_lists_in_inode_order_when_large() {
        let dir = tempfile::tempdir().unwrap();

        for i in 0..INODE_ORDER_MIN + 50 {
            fs::write(dir.path().join(format!("f{i}")), "").unwrap();
        }
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join(".gitignore"), "").unwrap();

        let fd = at::open_dir_path(dir.path()).unwrap();
        let mut listing = Listing::default();
        let mut found = Found::default();
        listing.fill(Arc::new(fd), &mut Vec::new(), &mut found);

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

    #[test]
    fn small_directories_are_listed_whole() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..20 {
            fs::write(dir.path().join(format!("f{i}")), "").unwrap();
        }

        let mut listing = Listing::default();
        let fd = at::open_dir_path(dir.path()).unwrap();
        listing.fill(Arc::new(fd), &mut Vec::new(), &mut Found::default());

        assert_eq!(listing.len(), 20);
        assert!((0..20).all(|i| listing.kind(i) == Some(Kind::File) || listing.kind(i).is_none()));
    }
}

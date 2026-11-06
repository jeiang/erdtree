use super::stat::Stat;
use std::path::Path;

/// A file's underlying inode: its identity, [`Inode::id`], and its link count.
#[derive(Clone, Copy, Debug)]
pub struct Inode {
    pub ino: u64,
    pub dev: u64,
    /// Number of hard links. Not part of the identity: another process may add or remove a link
    /// between two stats of the same file.
    pub nlink: u64,
}

impl Inode {
    pub const fn new(ino: u64, dev: u64, nlink: u64) -> Self {
        Self { ino, dev, nlink }
    }

    /// What makes the inode unique: device and inode number.
    pub const fn id(self) -> (u64, u64) {
        (self.dev, self.ino)
    }
}

impl Inode {
    /// The inode of the entry at `path` given its `stat`, which describes the link target if
    /// `followed_link` is `true` and the entry itself otherwise; `None` when it cannot be known.
    #[cfg(unix)]
    // Same signature as the fallible Windows implementation.
    #[allow(clippy::unnecessary_wraps)]
    pub const fn try_new(_path: &Path, stat: &Stat, _followed_link: bool) -> Option<Self> {
        Some(Self::new(stat.ino, stat.dev, stat.nlink))
    }

    /// Computes the inode of the file at `path`. Windows does not expose the file index through
    /// the metadata of a directory entry on stable Rust, so the file is opened and queried with
    /// `GetFileInformationByHandle`. Only regular files can be hard-linked so everything else
    /// (directories, unfollowed symlinks) is reported as having insufficient information. A
    /// followed symlink resolves to its target, matching the Unix behavior.
    #[cfg(windows)]
    pub fn try_new(path: &Path, stat: &Stat, followed_link: bool) -> Option<Self> {
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        };

        if !stat.kind.is_file() {
            return None;
        }

        // Open the reparse point itself unless it is a followed link, so that file-type reparse
        // points (e.g. cloud placeholders) are not recalled.
        let flags = if followed_link {
            FILE_FLAG_BACKUP_SEMANTICS
        } else {
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT
        };

        let info = file_information(path, flags)?;
        let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);

        Some(Self::new(
            index,
            u64::from(info.dwVolumeSerialNumber),
            u64::from(info.nNumberOfLinks),
        ))
    }
}

/// `GetFileInformationByHandle` for the file or directory at `path`, opened with `flags`.
#[cfg(windows)]
fn file_information(
    path: &Path,
    flags: u32,
) -> Option<windows_sys::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION> {
    use std::{
        fs::OpenOptions,
        mem::MaybeUninit,
        os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    };
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        GetFileInformationByHandle,
    };

    // No access rights are requested; we only query metadata.
    let file = OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(flags)
        .open(path)
        .ok()?;

    let mut raw = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();

    // SAFETY: the handle is valid for the lifetime of `file` and `raw` is a valid pointer to
    // a `BY_HANDLE_FILE_INFORMATION`, which is fully initialized when the call succeeds.
    unsafe {
        if GetFileInformationByHandle(file.as_raw_handle().cast(), raw.as_mut_ptr()) == 0 {
            return None;
        }
        Some(raw.assume_init())
    }
}

/// Device number of the file system of the directory at `path` given its stat, for
/// `--one-file-system`. On Windows, the serial number of the volume.
// Only the Windows lookup can fail or is not `const`.
#[cfg_attr(unix, allow(clippy::unnecessary_wraps, clippy::missing_const_for_fn))]
pub fn device_of(stat: &Stat, path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        let _ = path;
        Some(stat.dev)
    }

    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS;

        let _ = stat;
        file_information(path, FILE_FLAG_BACKUP_SEMANTICS)
            .map(|info| u64::from(info.dwVolumeSerialNumber))
    }
}

#[cfg(test)]
mod test {
    use super::Inode;
    use crate::tree::topo;

    /// A link added or removed between the stats of two names of one file changes the count
    /// each name sees, not the file: it must still count once.
    #[test]
    fn link_count_is_not_identity() {
        let a = Inode::new(7, 1, 2);
        let b = Inode::new(7, 1, 3);
        assert_eq!(a.id(), b.id());

        let mut linked = vec![(0, a.id()), (1, b.id()), (2, Inode::new(8, 1, 2).id())];
        assert_eq!(topo::dedupe(&mut linked, 1, |x, y| x.cmp(&y)), [1]);
    }
}

use std::{fs::Metadata, path::Path};

/// Represents a file's underlying inode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Inode {
    pub ino: u64,
    pub dev: u64,
    pub nlink: u64,
}

impl Inode {
    /// Initializer for an inode given all the properties that make it unique.
    pub const fn new(ino: u64, dev: u64, nlink: u64) -> Self {
        Self { ino, dev, nlink }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Insufficient information to compute inode")]
pub struct Error;

impl Inode {
    /// Computes the inode of the entry at `path` given its `md`, which must come from
    /// `symlink_metadata` semantics (i.e. not following links).
    #[cfg(unix)]
    // Same signature as the fallible Windows implementation.
    #[allow(clippy::unnecessary_wraps)]
    pub fn try_new(_path: &Path, md: &Metadata) -> Result<Self, Error> {
        use std::os::unix::fs::MetadataExt;

        Ok(Self::new(md.ino(), md.dev(), md.nlink()))
    }

    /// Computes the inode of the file at `path`. Windows does not expose the file index through
    /// the metadata of a directory entry on stable Rust, so the file is opened and queried with
    /// `GetFileInformationByHandle`. Only regular files can be hard-linked so everything else
    /// (directories, symlinks) is reported as having insufficient information.
    #[cfg(windows)]
    pub fn try_new(path: &Path, md: &Metadata) -> Result<Self, Error> {
        use std::{
            fs::OpenOptions,
            mem::MaybeUninit,
            os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
        };
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle,
        };

        if !md.is_file() {
            return Err(Error);
        }

        // No access rights are requested; we only query metadata.
        let file = OpenOptions::new()
            .access_mode(0)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .map_err(|_e| Error)?;

        let mut raw = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();

        // SAFETY: the handle is valid for the lifetime of `file` and `raw` is a valid pointer to
        // a `BY_HANDLE_FILE_INFORMATION`, which is fully initialized when the call succeeds.
        let info = unsafe {
            if GetFileInformationByHandle(file.as_raw_handle().cast(), raw.as_mut_ptr()) == 0 {
                return Err(Error);
            }
            raw.assume_init()
        };

        let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);

        Ok(Self::new(
            index,
            u64::from(info.dwVolumeSerialNumber),
            u64::from(info.nNumberOfLinks),
        ))
    }

    /// Platforms without a notion of inodes.
    #[cfg(not(any(unix, windows)))]
    pub fn try_new(_path: &Path, _md: &Metadata) -> Result<Self, Error> {
        Err(Error)
    }
}

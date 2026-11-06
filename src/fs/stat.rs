//! Platform-neutral metadata of one directory entry. The walk fills a [`Stat`] straight from the
//! kernel (`statx` or `fstatat` on Linux, `getattrlistbulk` or `fstatat` on macOS), or from
//! [`std::fs::Metadata`] for the root and on other platforms, so later stages never need
//! `Metadata` and the walk can ask the kernel for only the fields it uses.

use std::{
    fs::{FileType, Metadata},
    time::SystemTime,
};

/// What an entry is. Mirrors [`std::fs::FileType`], with the same method names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    File,
    Dir,
    Symlink,
    #[cfg(unix)]
    Fifo,
    #[cfg(unix)]
    Socket,
    #[cfg(unix)]
    BlockDevice,
    #[cfg(unix)]
    CharDevice,
    /// Anything else, including entries whose type is unknown.
    Other,
}

impl Kind {
    pub const fn is_file(self) -> bool {
        matches!(self, Self::File)
    }

    pub const fn is_dir(self) -> bool {
        matches!(self, Self::Dir)
    }

    pub const fn is_symlink(self) -> bool {
        matches!(self, Self::Symlink)
    }

    #[cfg(unix)]
    pub const fn is_fifo(self) -> bool {
        matches!(self, Self::Fifo)
    }

    #[cfg(unix)]
    pub const fn is_socket(self) -> bool {
        matches!(self, Self::Socket)
    }

    /// The kind encoded in the `S_IFMT` bits of a Unix mode.
    #[cfg(unix)]
    pub const fn from_mode(mode: u32) -> Self {
        match mode & 0o170_000 {
            0o100_000 => Self::File,
            0o040_000 => Self::Dir,
            0o120_000 => Self::Symlink,
            0o010_000 => Self::Fifo,
            0o140_000 => Self::Socket,
            0o060_000 => Self::BlockDevice,
            0o020_000 => Self::CharDevice,
            _ => Self::Other,
        }
    }

    /// The kind given by a `d_type` value of a directory entry, or `None` for `DT_UNKNOWN` and
    /// values this code does not know.
    #[cfg(fd_walk)]
    pub const fn from_dtype(d_type: u8) -> Option<Self> {
        Some(match d_type {
            1 => Self::Fifo,
            2 => Self::CharDevice,
            4 => Self::Dir,
            6 => Self::BlockDevice,
            8 => Self::File,
            10 => Self::Symlink,
            12 => Self::Socket,
            _ => return None,
        })
    }
}

impl From<FileType> for Kind {
    fn from(file_type: FileType) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileTypeExt;

            if file_type.is_file() {
                Self::File
            } else if file_type.is_dir() {
                Self::Dir
            } else if file_type.is_symlink() {
                Self::Symlink
            } else if file_type.is_fifo() {
                Self::Fifo
            } else if file_type.is_socket() {
                Self::Socket
            } else if file_type.is_block_device() {
                Self::BlockDevice
            } else if file_type.is_char_device() {
                Self::CharDevice
            } else {
                Self::Other
            }
        }

        #[cfg(not(unix))]
        {
            if file_type.is_symlink() {
                Self::Symlink
            } else if file_type.is_dir() {
                Self::Dir
            } else if file_type.is_file() {
                Self::File
            } else {
                Self::Other
            }
        }
    }
}

/// Which optional fields of a [`Stat`] to read. Timestamps cost conversion work (and, on network
/// file systems, bytes), so they are only read when something shows or sorts by them.
#[derive(Clone, Copy, Debug, Default)]
pub struct Fields {
    pub modified: bool,
    pub accessed: bool,
    pub created: bool,
}

/// Metadata of one entry. Without Unix, `mode` is zero and `nlink` is 1. A timestamp is `None`
/// when [`Fields`] did not ask for it or the platform or file system does not report it.
#[derive(Clone, Copy, Debug)]
pub struct Stat {
    pub kind: Kind,
    /// The `st_mode` bits, including the file type bits.
    pub mode: u32,
    /// Length in bytes.
    pub size: u64,
    /// Number of [`Self::BLOCK_BYTES`]-byte blocks allocated.
    #[cfg(unix)]
    pub blocks: u64,
    #[cfg(unix)]
    pub ino: u64,
    #[cfg(unix)]
    pub dev: u64,
    pub nlink: u64,
    #[cfg(unix)]
    pub uid: u32,
    #[cfg(unix)]
    pub gid: u32,
    pub modified: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    pub created: Option<SystemTime>,
}

impl Stat {
    /// Size of the blocks that [`Self::blocks`] counts, whatever the file system's block size.
    #[cfg(unix)]
    pub const BLOCK_BYTES: u64 = 512;

    /// Reads `md`; only the timestamps in `fields` are converted.
    pub fn from_metadata(md: &Metadata, fields: Fields) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;

        Self {
            kind: md.file_type().into(),
            #[cfg(unix)]
            mode: md.mode(),
            #[cfg(not(unix))]
            mode: 0,
            size: md.len(),
            #[cfg(unix)]
            blocks: md.blocks(),
            #[cfg(unix)]
            ino: md.ino(),
            #[cfg(unix)]
            dev: md.dev(),
            #[cfg(unix)]
            nlink: md.nlink(),
            #[cfg(not(unix))]
            nlink: 1,
            #[cfg(unix)]
            uid: md.uid(),
            #[cfg(unix)]
            gid: md.gid(),
            modified: fields.modified.then(|| md.modified().ok()).flatten(),
            accessed: fields.accessed.then(|| md.accessed().ok()).flatten(),
            created: fields.created.then(|| md.created().ok()).flatten(),
        }
    }
}

/// A timestamp as the kernel reports it, `secs` since the epoch (negative before it) and
/// `nanos`, when it is `wanted`; `None` otherwise and when the nanoseconds are out of range.
#[cfg(fd_walk)]
pub fn time(wanted: bool, secs: i64, nanos: impl TryInto<u32>) -> Option<SystemTime> {
    use std::time::Duration;

    if !wanted {
        return None;
    }

    let nanos = Duration::from_nanos(u64::from(nanos.try_into().ok()?));

    match u64::try_from(secs) {
        Ok(secs) => SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(secs) + nanos),
        // Same as `std`: a negative time is the epoch minus the seconds, plus the nanoseconds.
        Err(_) => SystemTime::UNIX_EPOCH
            .checked_sub(Duration::from_secs(secs.unsigned_abs()))?
            .checked_add(nanos),
    }
}

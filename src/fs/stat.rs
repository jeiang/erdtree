//! Platform-neutral metadata of one directory entry. The walk fills a [`Stat`] straight from the
//! kernel (`statx` on Linux) or from [`std::fs::Metadata`] elsewhere, so later stages never need
//! `Metadata` and the walk can ask the kernel for only the fields it uses.

use std::{
    fs::{FileType, Metadata},
    time::SystemTime,
};

/// What an entry is. Mirrors [`std::fs::FileType`], with the same method names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
    /// Whether this is a regular file.
    pub const fn is_file(self) -> bool {
        matches!(self, Self::File)
    }

    /// Whether this is a directory.
    pub const fn is_dir(self) -> bool {
        matches!(self, Self::Dir)
    }

    /// Whether this is a symbolic link.
    pub const fn is_symlink(self) -> bool {
        matches!(self, Self::Symlink)
    }

    /// Whether this is a named pipe.
    #[cfg(unix)]
    pub const fn is_fifo(self) -> bool {
        matches!(self, Self::Fifo)
    }

    /// Whether this is a socket.
    #[cfg(unix)]
    pub const fn is_socket(self) -> bool {
        matches!(self, Self::Socket)
    }

    /// Whether this is a block device.
    #[cfg(unix)]
    #[allow(
        dead_code,
        reason = "mirrors `FileType`; only the Linux walk and tests build it"
    )]
    pub const fn is_block_device(self) -> bool {
        matches!(self, Self::BlockDevice)
    }

    /// Whether this is a character device.
    #[cfg(unix)]
    #[allow(
        dead_code,
        reason = "mirrors `FileType`; only the Linux walk and tests build it"
    )]
    pub const fn is_char_device(self) -> bool {
        matches!(self, Self::CharDevice)
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

/// Metadata of one entry. Fields the platform lacks are zero, except `nlink`, which is 1.
#[derive(Clone, Copy, Debug)]
pub struct Stat {
    pub kind: Kind,
    /// The `st_mode` bits, including the file type bits.
    pub mode: u32,
    /// Length in bytes.
    pub size: u64,
    /// Number of 512-byte blocks allocated.
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

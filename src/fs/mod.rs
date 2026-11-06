/// Operations pertaining to underlying inodes of files.
pub mod inode;

/// System calls relative to a directory file descriptor.
#[cfg(fd_walk)]
pub mod at;

/// Directory listings with metadata in bulk (`getattrlistbulk`).
#[cfg(target_os = "macos")]
pub mod bulk;

/// Platform-neutral entry metadata read by the walk.
pub mod stat;

/// Unix file permissions.
#[cfg(unix)]
pub mod permissions;

/// Determining whether or not a file has extended attributes.
#[cfg(unix)]
pub mod xattr;

/// Concerned with determining group and owner of file.
#[cfg(unix)]
pub mod ug;

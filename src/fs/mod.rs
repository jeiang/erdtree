/// Operations pertaining to underlying inodes of files.
pub mod inode;

/// Linux system calls relative to a directory file descriptor.
#[cfg(target_os = "linux")]
pub mod at;

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

/// Operations pertaining to underlying inodes of files.
pub mod inode;

/// Unix file permissions.
#[cfg(unix)]
pub mod permissions;

/// Determining whether or not a file has extended attributes.
#[cfg(unix)]
pub mod xattr;

/// Concerned with determining group and owner of file.
#[cfg(unix)]
pub mod ug;

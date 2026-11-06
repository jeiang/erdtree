/// Operations pertaining to underlying inodes of files.
pub mod inode;

/// System calls relative to a directory file descriptor.
#[cfg(fd_walk)]
pub mod at;

/// Directory listings with metadata in bulk (`getattrlistbulk`).
#[cfg(target_os = "macos")]
pub mod bulk;

/// System calls a kernel may lack, tried once.
#[cfg(target_os = "linux")]
mod probe;

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

/// The longest path, counting the NUL, that a path-based system call takes.
#[cfg(target_os = "linux")]
pub const PATH_MAX: usize = 4096;
#[cfg(target_os = "macos")]
pub const PATH_MAX: usize = 1024;

/// Whether a path of `len` bytes fits in [`PATH_MAX`], so a path-based system call can take it.
/// erdtree once read directories, followed symlinks and opened files on macOS by path only; what
/// did not fit then is not reached relative to a descriptor either, so the output stays the
/// same.
#[cfg(target_os = "macos")]
pub const fn fits_path_max(len: usize) -> bool {
    len < PATH_MAX
}

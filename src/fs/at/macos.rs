//! macOS: `fstatat` and `fstat`. Directories are listed by [`crate::fs::bulk`] or `readdir`.

use super::{check, retry};
use crate::fs::stat::{Fields, Kind, Stat};
use std::{
    ffi::{CStr, c_int, c_long},
    io,
    os::fd::RawFd,
};

/// No flag beyond the common ones is needed to open a file.
pub const OPEN_FLAGS: c_int = 0;

/// Whether a path of `len` bytes fits in `PATH_MAX` (1024 on macOS, counting the NUL), so a
/// path-based system call can take it. erdtree once read directories, followed symlinks and
/// opened files on macOS by path only; what did not fit then is not reached relative to a
/// descriptor either, so the output stays the same.
pub const fn fits_path_max(len: usize) -> bool {
    len < 1024
}

/// Reads metadata of `path` relative to `dir`, with the `AT_*` `flags`.
pub fn stat_raw(dir: RawFd, path: &CStr, flags: c_int, fields: Fields) -> io::Result<Stat> {
    let mut buf = std::mem::MaybeUninit::<libc::stat>::zeroed();

    retry(|| {
        // SAFETY: `path` is NUL-terminated and `buf` is large enough for a `stat`.
        let ret = unsafe { libc::fstatat(dir, path.as_ptr(), buf.as_mut_ptr(), flags) };
        check(c_long::from(ret))
    })?;

    // SAFETY: zero-initialized, and every field is an integer.
    Ok(from_stat(&unsafe { buf.assume_init() }, fields))
}

/// Reads metadata of the open file `fd` itself.
pub fn stat_fd(fd: RawFd, fields: Fields) -> io::Result<Stat> {
    let mut buf = std::mem::MaybeUninit::<libc::stat>::zeroed();

    retry(|| {
        // SAFETY: `buf` is large enough for a `stat`.
        let ret = unsafe { libc::fstat(fd, buf.as_mut_ptr()) };
        check(c_long::from(ret))
    })?;

    // SAFETY: zero-initialized, and every field is an integer.
    Ok(from_stat(&unsafe { buf.assume_init() }, fields))
}

/// A [`Stat`] with the values `std::fs::Metadata` gives for `s`; only the timestamps in `fields`
/// are converted.
// `std` reads `st_dev` (an `i32`) with `as u64`, which sign-extends; this does the same.
#[allow(clippy::cast_sign_loss)]
fn from_stat(s: &libc::stat, fields: Fields) -> Stat {
    let time = |wanted: bool, secs: libc::time_t, nanos: c_long| {
        wanted
            .then(|| crate::fs::stat::system_time(secs, u32::try_from(nanos).ok()?))
            .flatten()
    };

    Stat {
        kind: Kind::from_mode(u32::from(s.st_mode)),
        mode: u32::from(s.st_mode),
        size: s.st_size as u64,
        blocks: s.st_blocks as u64,
        ino: s.st_ino,
        dev: s.st_dev as u64,
        nlink: u64::from(s.st_nlink),
        uid: s.st_uid,
        gid: s.st_gid,
        modified: time(fields.modified, s.st_mtime, s.st_mtime_nsec),
        accessed: time(fields.accessed, s.st_atime, s.st_atime_nsec),
        created: time(fields.created, s.st_birthtime, s.st_birthtime_nsec),
    }
}

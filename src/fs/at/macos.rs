//! macOS: `fstatat`, `fstat` and `readdir`. Directories are also listed by [`crate::fs::bulk`].

use super::{check, retry};
use crate::fs::stat::{Fields, Kind, Stat, time};
use std::{
    ffi::{CStr, c_int, c_long},
    io,
    os::fd::RawFd,
};

/// No flag beyond the common ones is needed to open a file.
pub const OPEN_FLAGS: c_int = 0;

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

/// Calls `f` with the name and `d_type` of every entry of the directory open as `fd` but `.` and
/// `..`, until it returns `false`. Reads through a duplicate of `fd`, which stays open; a
/// directory that cannot be read has no entries.
pub fn for_each_readdir(fd: RawFd, mut f: impl FnMut(&[u8], u8) -> bool) {
    // SAFETY: `fd` is open; the duplicate is owned by the stream from here on.
    let dup = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if dup < 0 {
        return;
    }
    // SAFETY: `dup` is an open directory descriptor.
    let stream = unsafe { libc::fdopendir(dup) };
    if stream.is_null() {
        // SAFETY: `dup` is open and owned here.
        unsafe { libc::close(dup) };
        return;
    }

    loop {
        // SAFETY: `stream` is an open directory stream.
        let ent = unsafe { libc::readdir(stream) };
        if ent.is_null() {
            break;
        }
        // SAFETY: `readdir` returned a valid entry, whose name is NUL-terminated.
        let (name, d_type) = unsafe { (CStr::from_ptr((*ent).d_name.as_ptr()), (*ent).d_type) };
        let name = name.to_bytes();
        if name == b"." || name == b".." {
            continue;
        }

        if !f(name, d_type) {
            break;
        }
    }

    // SAFETY: `stream` is open and owned here.
    unsafe { libc::closedir(stream) };
}

/// A [`Stat`] with the values `std::fs::Metadata` gives for `s`; only the timestamps in `fields`
/// are converted.
// `std` reads `st_dev` (an `i32`) with `as u64`, which sign-extends; this does the same.
#[allow(clippy::cast_sign_loss)]
fn from_stat(s: &libc::stat, fields: Fields) -> Stat {
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

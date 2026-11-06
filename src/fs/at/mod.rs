//! System calls the walk issues relative to a directory file descriptor: opening directories,
//! reading them (`getdents64` on Linux, `getattrlistbulk` on macOS, see [`super::bulk`]) and
//! reading metadata (`statx` or `fstatat` on Linux, `fstatat` on macOS). Relative calls look up
//! one path component instead of the whole path, and they work for paths longer than
//! `PATH_MAX`. What differs between the two lives in [`linux`] and [`macos`].

use super::stat::{Fields, Stat};
use std::{
    ffi::{CStr, CString, OsString, c_char, c_int, c_long},
    fs::File,
    io,
    os::{
        fd::{FromRawFd, OwnedFd, RawFd},
        unix::ffi::{OsStrExt, OsStringExt},
    },
    path::{Path, PathBuf},
};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "linux")]
use linux as sys;
#[cfg(target_os = "macos")]
use macos as sys;

#[cfg(target_os = "linux")]
pub use linux::{for_each_dirent, getdents, inode_order_is_useless};
pub use sys::stat_fd;

/// An entry named `name` in the directory open as `dir`.
#[derive(Clone, Copy, Debug)]
pub struct At<'a> {
    pub dir: RawFd,
    pub name: &'a CStr,
}

/// Runs `f` with `bytes` as a NUL-terminated string. Names up to the size of the stack buffer
/// (every name a Linux file system allows) need no allocation. Bytes with an interior NUL, which
/// no path has, give `EINVAL`.
pub fn with_cstr<T>(bytes: &[u8], f: impl FnOnce(&CStr) -> io::Result<T>) -> io::Result<T> {
    with_stack::<384, T>(bytes, f)
}

/// Runs `f` with `path` as a C string. Paths that fit in `PATH_MAX`, the longest any path-based
/// system call takes, need no allocation.
pub fn with_path<T>(path: &Path, f: impl FnOnce(&CStr) -> io::Result<T>) -> io::Result<T> {
    with_stack::<{ super::PATH_MAX }, T>(path.as_os_str().as_bytes(), f)
}

/// [`with_cstr`] with a stack buffer of `N` bytes.
fn with_stack<const N: usize, T>(
    bytes: &[u8],
    f: impl FnOnce(&CStr) -> io::Result<T>,
) -> io::Result<T> {
    fn invalid<E>(_: E) -> io::Error {
        io::Error::from_raw_os_error(libc::EINVAL)
    }

    if bytes.len() < N {
        let mut stack = [0_u8; N];
        stack[..bytes.len()].copy_from_slice(bytes);

        return CStr::from_bytes_with_nul(&stack[..=bytes.len()])
            .map_err(invalid)
            .and_then(f);
    }

    f(&CString::new(bytes).map_err(invalid)?)
}

/// Retries `f` while it is interrupted by a signal.
pub(super) fn retry<T>(mut f: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    loop {
        match f() {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {},
            result => return result,
        }
    }
}

pub(super) fn check(ret: c_long) -> io::Result<c_long> {
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(ret)
    }
}

/// Flags of every `open`: read only, close on `exec`, plus what the platform needs.
const OPEN_FLAGS: c_int = libc::O_RDONLY | libc::O_CLOEXEC | sys::OPEN_FLAGS;
const DIR_FLAGS: c_int = OPEN_FLAGS | libc::O_DIRECTORY;

/// Opens the directory `name` of directory `dir`. A symlink is only followed with `follow`.
pub fn open_dir_at(dir: RawFd, name: &CStr, follow: bool) -> io::Result<OwnedFd> {
    let flags = if follow {
        DIR_FLAGS
    } else {
        DIR_FLAGS | libc::O_NOFOLLOW
    };

    retry(|| {
        // SAFETY: `name` is NUL-terminated and outlives the call.
        let fd = unsafe { libc::openat(dir, name.as_ptr(), flags) };
        owned(fd)
    })
}

/// Opens the directory at `path`. A symlink as the last component is only followed with `follow`.
pub fn open_dir_path(path: &Path, follow: bool) -> io::Result<OwnedFd> {
    let flags = if follow {
        DIR_FLAGS
    } else {
        DIR_FLAGS | libc::O_NOFOLLOW
    };

    with_path(path, |path| {
        retry(|| {
            // SAFETY: `path` is NUL-terminated and outlives the call.
            let fd = unsafe { libc::open(path.as_ptr(), flags) };
            owned(fd)
        })
    })
}

fn owned(fd: c_int) -> io::Result<OwnedFd> {
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: `fd` is a new, open descriptor that nothing else owns.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }
}

pub fn read_link(at: At<'_>) -> io::Result<PathBuf> {
    let mut buf: Vec<u8> = vec![0; 256];

    loop {
        // SAFETY: the kernel writes at most `buf.len()` bytes; `name` is NUL-terminated.
        let n = unsafe {
            libc::readlinkat(
                at.dir,
                at.name.as_ptr(),
                buf.as_mut_ptr().cast::<c_char>(),
                buf.len(),
            )
        };

        let n = usize::try_from(n).map_err(|_| io::Error::last_os_error())?;

        if n < buf.len() {
            buf.truncate(n);
            return Ok(PathBuf::from(OsString::from_vec(buf)));
        }

        // The target may have been cut off: try again with room to spare.
        buf.resize(buf.len() * 2, 0);
    }
}

/// Opens the file `at` for reading, following a symlink as `File::open` does.
pub fn open_file(at: At<'_>) -> io::Result<File> {
    retry(|| {
        // SAFETY: `at.name` is NUL-terminated and outlives the call.
        let fd = unsafe { libc::openat(at.dir, at.name.as_ptr(), OPEN_FLAGS) };
        owned(fd).map(File::from)
    })
}

/// Reads metadata of `name` in directory `dir` (`at.dir`), following a symlink only with
/// `follow`.
pub fn stat_at(at: At<'_>, follow: bool, fields: Fields) -> io::Result<Stat> {
    let flags = if follow { 0 } else { libc::AT_SYMLINK_NOFOLLOW };
    sys::stat_raw(at.dir, at.name, flags, fields)
}

/// Metadata of the file at `path`, without following a symlink.
pub fn lstat_path(path: &Path, fields: Fields) -> io::Result<Stat> {
    with_path(path, |path| {
        sys::stat_raw(libc::AT_FDCWD, path, libc::AT_SYMLINK_NOFOLLOW, fields)
    })
}

/// Whether following the symlink `at` reaches something that exists.
pub fn target_exists(at: At<'_>) -> bool {
    stat_at(at, true, Fields::default()).is_ok()
}

/// Raises the soft limit of open files towards the hard limit, and returns the limit in force.
/// The walk keeps a directory open while its subdirectories wait to be read, so deep trees need
/// more than the common 1024.
pub fn raise_open_file_limit() -> usize {
    const WANTED: libc::rlim_t = 65_536;

    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };

    // SAFETY: `limit` is a valid `rlimit` for the calls to fill and read.
    unsafe {
        if libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut limit) != 0 {
            // Unknown: assume the common default.
            return 1024;
        }

        if limit.rlim_cur < WANTED {
            let old = limit.rlim_cur;
            limit.rlim_cur = limit.rlim_max.min(WANTED);

            // Failing leaves the limit as it was. macOS refuses more than
            // `kern.maxfilesperproc`, which may be below the wanted number but not below
            // `OPEN_MAX`.
            if libc::setrlimit(libc::RLIMIT_NOFILE, &raw const limit) != 0 {
                limit.rlim_cur = old;

                #[cfg(target_os = "macos")]
                {
                    const OPEN_MAX: libc::rlim_t = 10_240;

                    let mac = libc::rlimit {
                        rlim_cur: limit.rlim_max.min(OPEN_MAX),
                        rlim_max: limit.rlim_max,
                    };
                    if mac.rlim_cur > old
                        && libc::setrlimit(libc::RLIMIT_NOFILE, &raw const mac) == 0
                    {
                        limit.rlim_cur = mac.rlim_cur;
                    }
                }
            }
        }
    }

    usize::try_from(limit.rlim_cur).unwrap_or(usize::MAX)
}

#[cfg(test)]
mod test {
    use super::*;
    use std::{
        fs,
        os::{fd::AsRawFd, unix::fs::symlink},
    };

    pub const ALL: Fields = Fields {
        modified: true,
        accessed: true,
        created: true,
    };

    /// Everything but the creation time, which `fstatat` cannot give.
    pub fn same(a: &Stat, b: &Stat) -> bool {
        (
            a.kind, a.mode, a.size, a.blocks, a.ino, a.dev, a.nlink, a.uid, a.gid,
        ) == (
            b.kind, b.mode, b.size, b.blocks, b.ino, b.dev, b.nlink, b.uid, b.gid,
        ) && a.modified == b.modified
            && a.accessed == b.accessed
    }

    pub fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = |name: &str| dir.path().join(name);
        fs::write(p("file"), "hello").unwrap();
        fs::create_dir(p("dir")).unwrap();
        symlink("file", p("link")).unwrap();
        symlink("missing", p("dangling")).unwrap();
        dir
    }

    #[test]
    fn stat_at_matches_std() {
        let dir = fixture();
        let fd = open_dir_path(dir.path(), true).unwrap();

        for name in ["file", "dir", "link", "dangling"] {
            let c = CString::new(name).unwrap();
            let at = At {
                dir: fd.as_raw_fd(),
                name: &c,
            };
            let path = dir.path().join(name);

            let md = fs::symlink_metadata(&path).unwrap();
            let got = stat_at(at, false, ALL).unwrap();
            let want = Stat::from_metadata(&md, ALL);
            assert!(same(&got, &want), "{name}: {got:?} vs {want:?}");

            match fs::metadata(&path) {
                Ok(md) => {
                    let got = stat_at(at, true, ALL).unwrap();
                    assert!(
                        same(&got, &Stat::from_metadata(&md, ALL)),
                        "{name} followed"
                    );
                },
                Err(_) => assert!(stat_at(at, true, ALL).is_err(), "{name} followed"),
            }
        }

        let own = stat_fd(fd.as_raw_fd(), ALL).unwrap();
        let md = fs::metadata(dir.path()).unwrap();
        assert!(same(&own, &Stat::from_metadata(&md, ALL)));
    }

    #[test]
    fn read_link_handles_long_targets() {
        let dir = tempfile::tempdir().unwrap();
        let fd = open_dir_path(dir.path(), true).unwrap();

        // macOS refuses symlink targets of `PATH_MAX` bytes or more.
        let longest = if cfg!(target_os = "macos") {
            1023
        } else {
            3000
        };
        for len in [1, 255, 256, 257, 1000, longest] {
            let target = "t".repeat(len);
            let name = format!("l{len}");
            symlink(&target, dir.path().join(&name)).unwrap();

            let c = CString::new(name).unwrap();
            let at = At {
                dir: fd.as_raw_fd(),
                name: &c,
            };
            assert_eq!(read_link(at).unwrap(), Path::new(&target), "{len}");
        }

        let c = CString::new("nothing").unwrap();
        let at = At {
            dir: fd.as_raw_fd(),
            name: &c,
        };
        assert!(read_link(at).is_err());
    }

    #[test]
    fn open_dir_refuses_files_and_unfollowed_symlinks() {
        let dir = fixture();
        let fd = open_dir_path(dir.path(), true).unwrap();
        let open = |name: &str, follow| {
            let c = CString::new(name).unwrap();
            open_dir_at(fd.as_raw_fd(), &c, follow)
        };

        assert!(open("dir", false).is_ok());
        assert!(open("file", false).is_err());
        assert!(open("link", true).is_err(), "the link points to a file");

        symlink("dir", dir.path().join("dirlink")).unwrap();
        assert!(open("dirlink", false).is_err());
        assert!(open("dirlink", true).is_ok());
    }
}

#[cfg(test)]
mod test_at_calls {
    use super::*;
    use std::{
        fs,
        io::Read,
        os::{fd::AsRawFd, unix::fs::symlink},
    };

    #[test]
    fn targets_match_the_path_based_calls() {
        let dir = tempfile::tempdir().unwrap();
        let p = |name: &str| dir.path().join(name);

        fs::write(p("plain"), "x").unwrap();
        symlink("plain", p("link-to-plain")).unwrap();
        symlink("missing", p("dangling")).unwrap();

        let fd = open_dir_path(dir.path(), true).unwrap();

        for name in ["plain", "link-to-plain", "dangling"] {
            let c = CString::new(name).unwrap();
            let at = At {
                dir: fd.as_raw_fd(),
                name: &c,
            };
            assert_eq!(target_exists(at), p(name).exists(), "{name}");
        }
    }

    /// Bytes with a NUL inside give `EINVAL` whether they fit the stack buffer or not; before,
    /// short ones were cut at the NUL.
    #[test]
    fn c_strings_refuse_interior_nul() {
        let ok = |bytes: &[u8]| with_cstr(bytes, |c| Ok(c.to_bytes().len()));
        let einval = |r: io::Result<usize>| r.unwrap_err().raw_os_error() == Some(libc::EINVAL);

        for len in [0, 1, 383, 384, 5000] {
            assert_eq!(ok(&vec![b'a'; len]).unwrap(), len);
        }
        assert!(einval(ok(b"a\0b")));
        assert!(einval(ok(&[b"a\0".as_slice(), &[b'b'; 500]].concat())));
        assert!(einval(with_path(Path::new("a\0b"), |c| Ok(c
            .to_bytes()
            .len()))));
    }

    /// On 32-bit glibc, opening a file of 2 GiB or more needs `O_LARGEFILE`. A sparse file takes
    /// no space; skip where the file system cannot hold one.
    #[test]
    fn open_file_opens_files_over_2_gib() {
        let dir = tempfile::tempdir().unwrap();
        let big = fs::File::create(dir.path().join("big")).unwrap();
        if big.set_len(3 << 30).is_err() {
            return;
        }
        let fd = open_dir_path(dir.path(), true).unwrap();
        let c = CString::new("big").unwrap();
        let file = open_file(At {
            dir: fd.as_raw_fd(),
            name: &c,
        })
        .unwrap();
        assert_eq!(file.metadata().unwrap().len(), 3 << 30);
    }

    #[test]
    fn open_file_reads_like_file_open() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("f"), "contents").unwrap();
        symlink("f", dir.path().join("l")).unwrap();
        let fd = open_dir_path(dir.path(), true).unwrap();

        for name in ["f", "l"] {
            let c = CString::new(name).unwrap();
            let mut text = String::new();
            open_file(At {
                dir: fd.as_raw_fd(),
                name: &c,
            })
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
            assert_eq!(text, "contents");
        }

        let c = CString::new("nothing").unwrap();
        assert!(
            open_file(At {
                dir: fd.as_raw_fd(),
                name: &c
            })
            .is_err()
        );
    }
}

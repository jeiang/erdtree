//! Linux: `getdents64`, `statx` (with `fstatat64` as the fallback) and `fstatfs`.

use super::{At, check, retry};
use crate::fs::stat::{Fields, Kind, Stat};
use std::{
    ffi::{CStr, c_char, c_int, c_long, c_void},
    io,
    os::fd::RawFd,
    sync::atomic::{AtomicBool, AtomicU8, Ordering},
    time::SystemTime,
};

/// `O_LARGEFILE` is zero on 64-bit targets. On 32-bit glibc, `openat` and `open` do not set it,
/// and without it the kernel refuses (`EOVERFLOW`) to open a file of 2 GiB or more.
pub const OPEN_FLAGS: c_int = libc::O_LARGEFILE;

/// Reads directory entries of `fd` into `buf` with `getdents64`. Returns the number of bytes
/// read; zero is the end of the directory.
pub fn getdents(fd: RawFd, buf: &mut [u8]) -> io::Result<usize> {
    retry(|| {
        // SAFETY: the kernel writes at most `buf.len()` bytes into `buf`.
        let n = unsafe {
            libc::syscall(
                libc::SYS_getdents64,
                libc::c_long::from(fd),
                buf.as_mut_ptr().cast::<c_void>(),
                buf.len(),
            )
        };
        check(n).map(|n| usize::try_from(n).unwrap_or(0))
    })
}

/// One entry of a `getdents64` buffer.
pub struct Dirent<'a> {
    pub ino: u64,
    /// The `d_type` field; `None` for `DT_UNKNOWN`.
    pub kind: Option<Kind>,
    /// The name, without the terminating NUL.
    pub name: &'a [u8],
}

/// Calls `f` with each entry of the first `len` bytes of `buf`, which `getdents` filled. A
/// malformed record ends the walk over the buffer.
pub fn for_each_dirent(buf: &[u8], len: usize, mut f: impl FnMut(Dirent<'_>)) {
    // struct linux_dirent64 { u64 d_ino; i64 d_off; u16 d_reclen; u8 d_type; char d_name[]; }
    const NAME: usize = 19;

    let buf = &buf[..len.min(buf.len())];
    let mut at = 0;

    while let Some(head) = buf.get(at..at + NAME) {
        let reclen = usize::from(u16::from_ne_bytes([head[16], head[17]]));
        let Some(record) = buf.get(at..at + reclen).filter(|_| reclen > NAME) else {
            return;
        };

        let field = &record[NAME..];
        let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());

        f(Dirent {
            ino: u64::from_ne_bytes([
                head[0], head[1], head[2], head[3], head[4], head[5], head[6], head[7],
            ]),
            kind: Kind::from_dtype(head[18]),
            name: &field[..end],
        });

        at += reclen;
    }
}

/// The layout of `struct statx_timestamp`.
#[repr(C)]
#[derive(Clone, Copy)]
struct Timestamp {
    sec: i64,
    nsec: u32,
    _reserved: i32,
}

/// The layout of `struct statx` (256 bytes on every architecture).
#[repr(C)]
struct Statx {
    mask: u32,
    _blksize: u32,
    _attributes: u64,
    nlink: u32,
    uid: u32,
    gid: u32,
    mode: u16,
    _pad: u16,
    ino: u64,
    size: u64,
    blocks: u64,
    _attributes_mask: u64,
    atime: Timestamp,
    btime: Timestamp,
    _ctime: Timestamp,
    mtime: Timestamp,
    _rdev_major: u32,
    _rdev_minor: u32,
    dev_major: u32,
    dev_minor: u32,
    _rest: [u64; 14],
}

const _: () = assert!(size_of::<Statx>() == 256);

const STATX_TYPE: u32 = 0x1;
const STATX_MODE: u32 = 0x2;
const STATX_NLINK: u32 = 0x4;
const STATX_UID: u32 = 0x8;
const STATX_GID: u32 = 0x10;
const STATX_ATIME: u32 = 0x20;
const STATX_MTIME: u32 = 0x40;
const STATX_INO: u32 = 0x100;
const STATX_SIZE: u32 = 0x200;
const STATX_BLOCKS: u32 = 0x400;
const STATX_BTIME: u32 = 0x800;

/// Set once `statx` turned out to be unavailable (an old kernel, or a sandbox that blocks it);
/// metadata then comes from `fstatat`.
static NO_STATX: AtomicBool = AtomicBool::new(false);

/// Reads metadata of `path` relative to `dir`, with the `AT_*` `flags`.
pub fn stat_raw(dir: RawFd, path: &CStr, flags: c_int, fields: Fields) -> io::Result<Stat> {
    if !NO_STATX.load(Ordering::Relaxed) {
        match statx(dir, path, flags, fields) {
            Err(e) if matches!(e.raw_os_error(), Some(libc::ENOSYS | libc::EPERM)) => {
                NO_STATX.store(true, Ordering::Relaxed);
            },
            result => return result,
        }
    }

    fstatat(dir, path, flags, fields)
}

/// Reads metadata of the open file `fd` itself.
pub fn stat_fd(fd: RawFd, fields: Fields) -> io::Result<Stat> {
    stat_raw(fd, c"", libc::AT_EMPTY_PATH, fields)
}

/// `statx` asking for the fields the walk uses.
// The mask has few bits set, so it fits `c_long`, which is 32 bits wide on 32-bit targets and
// 64 bits wide elsewhere: neither `as` is lossy, and `From` exists on only one of the two.
#[allow(clippy::cast_possible_wrap, clippy::cast_lossless)]
fn statx(dir: RawFd, path: &CStr, flags: c_int, fields: Fields) -> io::Result<Stat> {
    let mut mask = STATX_TYPE
        | STATX_MODE
        | STATX_NLINK
        | STATX_UID
        | STATX_GID
        | STATX_INO
        | STATX_SIZE
        | STATX_BLOCKS;

    if fields.modified {
        mask |= STATX_MTIME;
    }
    if fields.accessed {
        mask |= STATX_ATIME;
    }
    if fields.created {
        mask |= STATX_BTIME;
    }

    let mut buf = std::mem::MaybeUninit::<Statx>::zeroed();

    retry(|| {
        // SAFETY: `path` is NUL-terminated; the kernel fills at most 256 bytes of `buf`.
        let ret = unsafe {
            libc::syscall(
                libc::SYS_statx,
                c_long::from(dir),
                path.as_ptr().cast::<c_char>(),
                c_long::from(flags),
                mask as c_long,
                buf.as_mut_ptr(),
            )
        };
        check(ret)
    })?;

    // SAFETY: zero-initialized, and every field is an integer.
    let s = unsafe { buf.assume_init() };
    let time = |bit: u32, ts: Timestamp| -> Option<SystemTime> {
        (s.mask & bit != 0)
            .then(|| crate::fs::stat::system_time(ts.sec, ts.nsec))
            .flatten()
    };

    Ok(Stat {
        kind: Kind::from_mode(u32::from(s.mode)),
        mode: u32::from(s.mode),
        size: s.size,
        blocks: s.blocks,
        ino: s.ino,
        dev: libc::makedev(s.dev_major, s.dev_minor),
        nlink: u64::from(s.nlink),
        uid: s.uid,
        gid: s.gid,
        modified: time(STATX_MTIME, s.mtime),
        accessed: time(STATX_ATIME, s.atime),
        created: time(STATX_BTIME, s.btime),
    })
}

/// The fallback of [`statx`]. The integer types of `stat64` differ between architectures.
#[allow(clippy::useless_conversion, clippy::unnecessary_cast)]
fn fstatat(dir: RawFd, path: &CStr, flags: c_int, fields: Fields) -> io::Result<Stat> {
    let mut buf = std::mem::MaybeUninit::<libc::stat64>::zeroed();

    retry(|| {
        // SAFETY: `path` is NUL-terminated and `buf` is large enough for a `stat64`.
        let ret = unsafe { libc::fstatat64(dir, path.as_ptr(), buf.as_mut_ptr(), flags) };
        check(c_long::from(ret))
    })?;

    // SAFETY: zero-initialized, and every field is an integer.
    let s = unsafe { buf.assume_init() };

    Ok(Stat {
        kind: Kind::from_mode(s.st_mode),
        mode: s.st_mode,
        size: u64::try_from(s.st_size).unwrap_or(0),
        blocks: u64::try_from(s.st_blocks).unwrap_or(0),
        ino: s.st_ino,
        dev: s.st_dev,
        nlink: u64::from(s.st_nlink),
        uid: s.st_uid,
        gid: s.st_gid,
        modified: fields
            .modified
            .then(|| {
                crate::fs::stat::system_time(
                    i64::from(s.st_mtime),
                    u32::try_from(s.st_mtime_nsec).ok()?,
                )
            })
            .flatten(),
        accessed: fields
            .accessed
            .then(|| {
                crate::fs::stat::system_time(
                    i64::from(s.st_atime),
                    u32::try_from(s.st_atime_nsec).ok()?,
                )
            })
            .flatten(),
        created: None,
    })
}

/// Whether the file system of the directory open as `fd` returns directory entries in an order
/// unrelated to inode numbers, or is remote: sorting entries by inode number would not help
/// there. The magic numbers are those `gnulib` uses for `fts`.
// `f_type` is signed or unsigned and 32 or 64 bits wide, depending on the architecture and libc.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::unnecessary_cast
)]
pub fn inode_order_is_useless(fd: RawFd) -> bool {
    const NFS: i64 = 0x6969;
    const TMPFS: i64 = 0x0102_1994;
    const FUSE: i64 = 0x6573_5546;
    const SMB: i64 = 0x517B;
    const SMB2: i64 = 0xFE53_4D42;
    const CIFS: i64 = 0xFF53_4D42;
    const NINEP: i64 = 0x0102_1997;
    const CEPH: i64 = 0x00C3_6400;

    let mut buf = std::mem::MaybeUninit::<libc::statfs>::zeroed();

    // SAFETY: `buf` is large enough for a `statfs`.
    if unsafe { libc::fstatfs(fd, buf.as_mut_ptr()) } != 0 {
        return true;
    }

    // SAFETY: the call succeeded.
    let magic = i64::from(unsafe { buf.assume_init() }.f_type as u32);

    [NFS, TMPFS, FUSE, SMB, SMB2, CIFS, NINEP, CEPH].contains(&magic)
}

/// Whether `has_xattrs` can use `listxattrat`: not known yet, yes or no.
static LISTXATTRAT: AtomicU8 = AtomicU8::new(UNKNOWN);
const UNKNOWN: u8 = 0;
const SUPPORTED: u8 = 1;
const UNSUPPORTED: u8 = 2;

/// The number of `listxattrat` (Linux 6.17), which is the same on the architectures that have
/// the common system call numbers after 400.
#[cfg(any(
    target_arch = "x86_64",
    target_arch = "x86",
    target_arch = "aarch64",
    target_arch = "arm",
    target_arch = "riscv64",
    target_arch = "powerpc64",
    target_arch = "s390x",
    target_arch = "loongarch64"
))]
const SYS_LISTXATTRAT: Option<c_long> = Some(465);
#[cfg(not(any(
    target_arch = "x86_64",
    target_arch = "x86",
    target_arch = "aarch64",
    target_arch = "arm",
    target_arch = "riscv64",
    target_arch = "powerpc64",
    target_arch = "s390x",
    target_arch = "loongarch64"
)))]
const SYS_LISTXATTRAT: Option<c_long> = None;

/// Whether the entry `at` has extended attributes, following a symlink like `listxattr`; `None`
/// when the kernel has no `listxattrat` (or a sandbox blocks it), so the caller must use the
/// path-based call. A failing call means no attributes, as with `listxattr`.
pub fn has_xattrs(at: At<'_>) -> Option<bool> {
    let state = LISTXATTRAT.load(Ordering::Relaxed);
    let number = SYS_LISTXATTRAT.filter(|_| state != UNSUPPORTED)?;

    // SAFETY: `at.name` is NUL-terminated; a null list with size 0 only asks for the size.
    let ret = unsafe {
        libc::syscall(
            number,
            c_long::from(at.dir),
            at.name.as_ptr(),
            0 as c_long,
            std::ptr::null_mut::<c_char>(),
            0 as libc::size_t,
        )
    };

    if ret >= 0 {
        LISTXATTRAT.store(SUPPORTED, Ordering::Relaxed);
        return Some(ret > 0);
    }

    let errno = io::Error::last_os_error().raw_os_error();

    if state == UNKNOWN && matches!(errno, Some(libc::ENOSYS | libc::EPERM | libc::EINVAL)) {
        LISTXATTRAT.store(UNSUPPORTED, Ordering::Relaxed);
        return None;
    }

    Some(false)
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::fs::at::{
        open_dir_path,
        test::{ALL, fixture, same},
    };
    use std::{
        collections::BTreeSet,
        ffi::CString,
        fs::{self, File},
        os::{fd::AsRawFd, unix::fs::symlink},
    };

    #[test]
    fn fstatat_fallback_matches_statx() {
        let dir = fixture();
        let fd = open_dir_path(dir.path(), true).unwrap();

        for name in ["file", "dir", "link", "dangling"] {
            let c = CString::new(name).unwrap();

            for flags in [0, libc::AT_SYMLINK_NOFOLLOW] {
                match (
                    statx(fd.as_raw_fd(), &c, flags, ALL),
                    fstatat(fd.as_raw_fd(), &c, flags, ALL),
                ) {
                    (Ok(a), Ok(b)) => assert!(same(&a, &b), "{name} {flags}"),
                    (Err(a), Err(b)) => assert_eq!(a.raw_os_error(), b.raw_os_error()),
                    (a, b) => panic!("{name}: {a:?} vs {b:?}"),
                }
            }
        }
    }

    #[test]
    fn getdents_lists_what_read_dir_lists() {
        let dir = tempfile::tempdir().unwrap();

        // More entries than fit in one small buffer, so the buffer is refilled.
        for i in 0..300 {
            File::create(dir.path().join(format!("entry-with-a-longer-name-{i}"))).unwrap();
        }
        fs::create_dir(dir.path().join("sub")).unwrap();
        symlink("sub", dir.path().join("link")).unwrap();

        let fd = open_dir_path(dir.path(), true).unwrap();
        let mut buf = vec![0_u8; 1024];
        let mut got = BTreeSet::new();

        loop {
            let n = getdents(fd.as_raw_fd(), &mut buf).unwrap();
            if n == 0 {
                break;
            }
            for_each_dirent(&buf, n, |entry| {
                got.insert((entry.name.to_vec(), entry.kind));
            });
        }

        got.retain(|(name, _)| name != b"." && name != b"..");

        let want: BTreeSet<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                let kind = Kind::from(e.file_type().unwrap());
                (e.file_name().into_encoded_bytes(), Some(kind))
            })
            .collect();

        assert_eq!(want.len(), 302);
        assert_eq!(got, want);
    }

    #[test]
    fn dirent_parser_stops_at_malformed_records() {
        // A record of 24 bytes holding the name "ab", then one with a length that overruns the
        // buffer.
        let mut buf = vec![0_u8; 24];
        buf[0] = 7;
        buf[16..18].copy_from_slice(&24_u16.to_ne_bytes());
        buf[18] = 8;
        buf[19..21].copy_from_slice(b"ab");
        buf.extend_from_slice(&[0; 16]);
        buf.extend_from_slice(&200_u16.to_ne_bytes());
        buf.extend_from_slice(&[8, b'x', 0, 0, 0]);

        let mut names = vec![];
        for_each_dirent(&buf, buf.len(), |e| {
            names.push((e.ino, e.kind, e.name.to_vec()));
        });
        assert_eq!(names, [(7, Some(Kind::File), b"ab".to_vec())]);

        // Lengths past the buffer are clamped, and an empty buffer has no entries.
        for_each_dirent(&buf, usize::MAX, |_| {});
        for_each_dirent(&[], 0, |_| panic!("no entries"));
    }
}

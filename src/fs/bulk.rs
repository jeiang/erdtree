//! Reading a directory with `getattrlistbulk(2)`: one call returns the names, types and metadata
//! of many entries, so listing a directory needs no `lstat` per entry, and no lookup of each name.
//!
//! The kernel packs, per entry, the attributes it could return (the `ATTR_CMN_RETURNED_ATTRS` set
//! says which), in the order of their bits. `ATTR_CMN_OBJTYPE` is always asked for: without it,
//! APFS answers through the generic path, which looks up a vnode per entry and is as slow as
//! `lstat`.
//!
//! With it, APFS reads the attributes from its own records, without looking up a vnode, so an
//! entry another file system is mounted on, or a cryptex is grafted onto
//! (`/System/Volumes/Preboot/Cryptexes/OS`), comes with the attributes of the directory below,
//! and nothing tells the two apart: same inode, same device. The metadata of a directory is
//! therefore never taken from a bulk read; the walk reads it from the opened directory instead.
//! Other entries come with a [`Stat`] when their attributes are complete and on APFS, whose
//! attributes are those `lstat` reports.

use super::stat::{Fields, Kind, Stat, time};
use std::{
    ffi::{CStr, c_void},
    io,
    os::fd::RawFd,
};

/// Size of the buffer one call fills. A directory of a few hundred entries is read in one call.
pub const BUF_LEN: usize = 64 * 1024;

/// `ATTR_CMN_ERROR`, which `libc` lacks: an error reading the attributes of the entry.
const ATTR_CMN_ERROR: u32 = 0x2000_0000;

/// Common attributes a [`Stat`] needs, besides the timestamps.
const COMMON: u32 = libc::ATTR_CMN_DEVID
    | libc::ATTR_CMN_OBJTYPE
    | libc::ATTR_CMN_OWNERID
    | libc::ATTR_CMN_GRPID
    | libc::ATTR_CMN_ACCESSMASK
    | libc::ATTR_CMN_FLAGS
    | libc::ATTR_CMN_FILEID;

/// File attributes a [`Stat`] of anything but a directory needs.
const FILE: u32 =
    libc::ATTR_FILE_LINKCOUNT | libc::ATTR_FILE_ALLOCSIZE | libc::ATTR_FILE_DATALENGTH;

/// A data vault: `lstat` of it fails (`EPERM`) without an entitlement, a bulk read reports it.
const UF_DATAVAULT: u32 = 0x0000_0080;

#[derive(Clone, Copy)]
pub struct Request {
    list: libc::attrlist,
    /// The common attributes a [`Stat`] needs: [`COMMON`] and the wanted timestamps.
    common: u32,
    fields: Fields,
}

impl Request {
    /// Asks for what a [`Stat`] with the timestamps in `fields` needs.
    pub const fn new(fields: Fields) -> Self {
        let mut common = COMMON;

        if fields.modified {
            common |= libc::ATTR_CMN_MODTIME;
        }
        if fields.accessed {
            common |= libc::ATTR_CMN_ACCTIME;
        }
        if fields.created {
            common |= libc::ATTR_CMN_CRTIME;
        }

        Self {
            list: libc::attrlist {
                bitmapcount: libc::ATTR_BIT_MAP_COUNT,
                reserved: 0,
                commonattr: libc::ATTR_CMN_RETURNED_ATTRS
                    | libc::ATTR_CMN_NAME
                    | ATTR_CMN_ERROR
                    | common,
                volattr: 0,
                dirattr: 0,
                fileattr: FILE,
                forkattr: 0,
            },
            common,
            fields,
        }
    }
}

/// One entry of a bulk read.
pub struct Entry<'a> {
    /// The name, without a NUL.
    pub name: &'a [u8],
    /// The type; `None` when the kernel did not report it.
    pub kind: Option<Kind>,
    /// The metadata `lstat` gives; never for a directory, and only when the attributes give all
    /// of it.
    pub stat: Option<Stat>,
}

/// Reads the next entries of the directory open as `fd` into `buf`, and calls `f` with each:
/// `None` for an entry the kernel listed without a name, as it does for one whose attributes
/// it may not read (a data vault). Returns how many entries the kernel returned; zero is the
/// end of the directory.
pub fn read(
    fd: RawFd,
    req: &Request,
    volumes: &mut Volumes,
    buf: &mut [u8],
    mut f: impl FnMut(Option<Entry<'_>>),
) -> io::Result<usize> {
    let mut list = req.list;

    let count = super::at::retry(|| {
        // SAFETY: `list` is a valid `attrlist` and the kernel writes at most `buf.len()` bytes
        // into `buf`.
        let ret = unsafe {
            libc::getattrlistbulk(
                fd,
                (&raw mut list).cast::<c_void>(),
                buf.as_mut_ptr().cast::<c_void>(),
                buf.len(),
                0,
            )
        };
        usize::try_from(ret).map_err(|_| io::Error::last_os_error())
    })?;

    let mut at = 0;
    for _ in 0..count {
        let Some(len) = read_u32(buf, at).and_then(|len| usize::try_from(len).ok()) else {
            break;
        };
        let Some(record) = buf.get(at..at + len).filter(|_| len >= 4) else {
            break;
        };

        f(parse(record, req, &mut |dev| volumes.is_apfs(dev, fd)));

        at += len;
    }

    Ok(count)
}

/// Which file systems are APFS, by device number, as far as a worker has seen them.
#[derive(Default)]
pub struct Volumes {
    /// The device asked about last and its answer: entries of one directory share a device.
    last: Option<(u64, bool)>,
    seen: Vec<(u64, bool)>,
}

impl Volumes {
    /// Whether the device `dev` is an APFS volume. `fd` is a directory on that device: the
    /// entries a directory lists are on its own file system, except mount points, which are
    /// directories, whose attributes are not used.
    fn is_apfs(&mut self, dev: u64, fd: RawFd) -> bool {
        if let Some((last, apfs)) = self.last
            && last == dev
        {
            return apfs;
        }

        let apfs = if let Some(&(_, apfs)) = self.seen.iter().find(|(seen, _)| *seen == dev) {
            apfs
        } else {
            let apfs = is_apfs(fd);
            self.seen.push((dev, apfs));
            apfs
        };

        self.last = Some((dev, apfs));
        apfs
    }
}

/// Whether the file open as `fd` is on an APFS volume.
fn is_apfs(fd: RawFd) -> bool {
    let mut buf = std::mem::MaybeUninit::<libc::statfs>::uninit();

    // SAFETY: `buf` is large enough for a `statfs`.
    if unsafe { libc::fstatfs(fd, buf.as_mut_ptr()) } != 0 {
        return false;
    }
    // SAFETY: the call succeeded, so it filled `buf`.
    let name = unsafe { buf.assume_init() }
        .f_fstypename
        .map(|c| c.to_ne_bytes()[0]);
    name.starts_with(b"apfs\0")
}

/// Reads a native-endian `u32` at `at`, which need not be aligned.
fn read_u32(buf: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_ne_bytes(buf.get(at..at + 4)?.try_into().ok()?))
}

/// Reads a native-endian `u64` at `at`, which need not be aligned.
fn read_u64(buf: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_ne_bytes(buf.get(at..at + 8)?.try_into().ok()?))
}

/// Walks the attributes packed in one entry, in order.
struct Cursor<'a> {
    record: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn u32(&mut self) -> Option<u32> {
        let v = read_u32(self.record, self.at)?;
        self.at += 4;
        Some(v)
    }

    fn u64(&mut self) -> Option<u64> {
        let v = read_u64(self.record, self.at)?;
        self.at += 8;
        Some(v)
    }

    /// A `struct timespec`: 64-bit seconds and nanoseconds.
    // The seconds are a signed `time_t`, read as their bits.
    #[allow(clippy::cast_possible_wrap)]
    fn time(&mut self) -> Option<(i64, u64)> {
        Some((self.u64()? as i64, self.u64()?))
    }

    /// Reads the attribute `bit` when `returned` has it; `None` inside when it was not returned.
    // Three cases: malformed record (outer `None`), not returned (inner `None`), or the value.
    #[allow(clippy::option_option)]
    fn take<T>(
        &mut self,
        returned: u32,
        bit: u32,
        read: impl FnOnce(&mut Self) -> Option<T>,
    ) -> Option<Option<T>> {
        if returned & bit == 0 {
            return Some(None);
        }
        read(self).map(Some)
    }
}

/// The kind of a `vtype` (`ATTR_CMN_OBJTYPE`); `None` for one without a file type.
const fn kind_of(vtype: u32) -> Option<Kind> {
    Some(match vtype {
        1 => Kind::File,
        2 => Kind::Dir,
        3 => Kind::BlockDevice,
        4 => Kind::CharDevice,
        5 => Kind::Symlink,
        6 => Kind::Socket,
        7 => Kind::Fifo,
        _ => return None,
    })
}

/// Decodes one entry; `None` when it is malformed or has no name. `apfs` tells whether a device
/// is an APFS volume.
fn parse<'a>(
    record: &'a [u8],
    req: &Request,
    apfs: &mut impl FnMut(u64) -> bool,
) -> Option<Entry<'a>> {
    let mut c = Cursor { record, at: 4 };

    // `ATTR_CMN_RETURNED_ATTRS` comes first: the common, volume, directory, file and fork sets.
    let common = c.u32()?;
    let _vol = c.u32()?;
    let _dir = c.u32()?;
    let file = c.u32()?;
    let _fork = c.u32()?;

    // `ATTR_CMN_ERROR` directly follows the attribute sets (getattrlistbulk(2)); only a record
    // whose attributes could not be read carries it, and such a record holds just the name.
    let error = c.take(common, ATTR_CMN_ERROR, Cursor::u32)?;

    let name = c.take(common, libc::ATTR_CMN_NAME, |c| {
        let base = c.at;
        let offset = isize::try_from(i32::from_ne_bytes(c.u32()?.to_ne_bytes())).ok()?;
        let len = usize::try_from(c.u32()?).ok()?;
        let start = base.checked_add_signed(offset)?;
        let bytes = c.record.get(start..start + len)?;
        Some(CStr::from_bytes_until_nul(bytes).map_or(bytes, CStr::to_bytes))
    })??;

    if name.is_empty() {
        return None;
    }

    // Leave the entry to `fstatat`, which reports it as `lstat` does.
    if error.is_some_and(|e| e != 0) {
        return Some(Entry {
            name,
            kind: None,
            stat: None,
        });
    }

    let dev = c.take(common, libc::ATTR_CMN_DEVID, Cursor::u32)?;
    let kind = c
        .take(common, libc::ATTR_CMN_OBJTYPE, Cursor::u32)?
        .and_then(kind_of);

    // A directory needs nothing more.
    if kind.is_none_or(Kind::is_dir) {
        return Some(Entry {
            name,
            kind,
            stat: None,
        });
    }

    let created = c.take(common, libc::ATTR_CMN_CRTIME, Cursor::time)?;
    let modified = c.take(common, libc::ATTR_CMN_MODTIME, Cursor::time)?;
    let accessed = c.take(common, libc::ATTR_CMN_ACCTIME, Cursor::time)?;
    let uid = c.take(common, libc::ATTR_CMN_OWNERID, Cursor::u32)?;
    let gid = c.take(common, libc::ATTR_CMN_GRPID, Cursor::u32)?;
    let mode = c.take(common, libc::ATTR_CMN_ACCESSMASK, Cursor::u32)?;
    let flags = c.take(common, libc::ATTR_CMN_FLAGS, Cursor::u32)?;
    let ino = c.take(common, libc::ATTR_CMN_FILEID, Cursor::u64)?;
    let nlink = c.take(file, libc::ATTR_FILE_LINKCOUNT, Cursor::u32)?;
    let blocks = c.take(file, libc::ATTR_FILE_ALLOCSIZE, Cursor::u64)?;
    let size = c.take(file, libc::ATTR_FILE_DATALENGTH, Cursor::u64)?;

    let stat = (|| {
        if common & req.common != req.common || flags? & UF_DATAVAULT != 0 {
            return None;
        }

        let mode = mode?;
        let kind = Kind::from_mode(mode);

        // `st_dev` is an `i32` that `std` sign-extends.
        let dev =
            u64::from_ne_bytes(i64::from(i32::from_ne_bytes(dev?.to_ne_bytes())).to_ne_bytes());
        if !apfs(dev) {
            return None;
        }

        let fields = req.fields;

        Some(Stat {
            kind,
            mode,
            size: size?,
            blocks: blocks?.div_ceil(Stat::BLOCK_BYTES),
            ino: ino?,
            dev,
            // `st_nlink` is 16 bits wide; `stat` saturates.
            nlink: u64::from(nlink?.min(u32::from(u16::MAX))),
            uid: uid?,
            gid: gid?,
            modified: modified.and_then(|(secs, nanos)| time(fields.modified, secs, nanos)),
            accessed: accessed.and_then(|(secs, nanos)| time(fields.accessed, secs, nanos)),
            created: created.and_then(|(secs, nanos)| time(fields.created, secs, nanos)),
        })
    })()
    // The mode must agree with the type the listing reported.
    .filter(|stat| Some(stat.kind) == kind);

    Some(Entry { name, kind, stat })
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::fs::at::{self, test::ALL};
    use std::{
        collections::BTreeSet,
        fs,
        os::{
            fd::AsRawFd,
            unix::{ffi::OsStrExt, fs::symlink},
        },
        path::Path,
    };

    /// One entry read in bulk: name, type and metadata.
    type Read = (Vec<u8>, Option<Kind>, Option<Stat>);

    /// Reads `dir` in bulk with a buffer of `len` bytes; returns the entries, how many came
    /// with metadata, and how many came without a name.
    fn bulk(dir: &Path, len: usize) -> (Vec<Read>, usize, usize) {
        let fd = at::open_dir_path(dir, true).unwrap();
        let req = Request::new(ALL);
        let mut volumes = Volumes::default();
        let mut buf = vec![0; len];
        let mut out = vec![];
        let mut nameless = 0;

        while read(fd.as_raw_fd(), &req, &mut volumes, &mut buf, |e| match e {
            Some(e) => out.push((e.name.to_vec(), e.kind, e.stat)),
            None => nameless += 1,
        })
        .unwrap()
            > 0
        {}

        let full = out.iter().filter(|e| e.2.is_some()).count();
        (out, full, nameless)
    }

    /// Checks the bulk read of `dir` against `read_dir` and `lstat`; returns how many entries
    /// came with metadata.
    fn check(dir: &Path, len: usize) -> usize {
        let (entries, full, nameless) = bulk(dir, len);
        let names: BTreeSet<_> = entries.iter().map(|e| e.0.clone()).collect();
        let want: BTreeSet<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().as_bytes().to_vec())
            .collect();
        assert!(names.is_subset(&want), "{}", dir.display());
        assert_eq!(names.len() + nameless, want.len(), "{}", dir.display());
        // What came without a name cannot be stat'ed.
        for name in want.difference(&names) {
            let path = dir.join(std::ffi::OsStr::from_bytes(name));
            assert!(fs::symlink_metadata(&path).is_err(), "{}", path.display());
        }

        for (name, kind, stat) in entries {
            let path = dir.join(std::ffi::OsStr::from_bytes(&name));
            let Ok(md) = fs::symlink_metadata(&path) else {
                continue;
            };
            let std = Stat::from_metadata(&md, ALL);
            if let Some(kind) = kind {
                assert_eq!(kind, std.kind, "{}", path.display());
            }
            if let Some(got) = stat {
                assert!(!got.kind.is_dir(), "{}", path.display());
                assert_eq!(format!("{got:?}"), format!("{std:?}"), "{}", path.display());
            }
        }

        full
    }

    #[test]
    fn bulk_metadata_matches_lstat() {
        let dir = tempfile::tempdir().unwrap();
        let p = |name: &str| dir.path().join(name);

        fs::write(p("file"), "hello").unwrap();
        fs::write(p("big"), vec![7; 100_000]).unwrap();
        fs::hard_link(p("file"), p("hard")).unwrap();
        fs::create_dir(p("dir")).unwrap();
        fs::create_dir(p("dir/sub")).unwrap();
        symlink("file", p("link")).unwrap();
        symlink("missing", p("dangling")).unwrap();
        for i in 0..500 {
            fs::write(p(&format!("many-{i:04}-with-a-longer-name")), "x").unwrap();
        }
        let c = std::ffi::CString::new(p("fifo").as_os_str().as_bytes()).unwrap();
        // SAFETY: a valid path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
        let _socket = std::os::unix::net::UnixListener::bind(p("socket")).unwrap();
        fs::write(p("modes"), "").unwrap();
        fs::set_permissions(
            p("modes"),
            std::os::unix::fs::PermissionsExt::from_mode(0o7755),
        )
        .unwrap();

        // A small buffer takes several calls.
        assert!(check(dir.path(), 4096) > 500);
        assert!(check(dir.path(), BUF_LEN) > 500);

        for system in [
            "/",
            "/usr/bin",
            "/dev",
            "/System/Volumes",
            "/System/Volumes/Preboot/Cryptexes",
            "/System/Volumes/Preboot/Cryptexes/OS",
            "/Library/Caches",
            "/private/var",
        ] {
            if Path::new(system).is_dir() && fs::read_dir(system).is_ok() {
                check(Path::new(system), BUF_LEN);
            }
        }
    }
}

//! Opening directories relative to their parent's descriptor, and the entries of a listing as
//! targets of relative system calls.

use super::Listing;
use crate::{
    fs::{
        at::{self, At},
        stat::{Fields, Stat},
    },
    walk::{
        Walker,
        fds::{Budget, DirFd, LONG_PATH},
        rules,
    },
};
use std::{
    ffi::CStr,
    os::{
        fd::{OwnedFd, RawFd},
        unix::ffi::OsStrExt,
    },
    path::Path,
    sync::Arc,
    thread,
    time::Duration,
};

/// How often opening a directory is retried when the process or system is out of descriptors,
/// and the longest wait between tries.
const OPEN_RETRIES: u32 = 100;

/// How a job opens and lists its directory.
pub struct Access {
    /// The parent directory, which the directory is opened relative to. `None` when the budget
    /// of held descriptors is spent: the directory is then opened by path.
    parent: Option<Arc<DirFd>>,
    /// Whether the directory is a followed symlink, so opening it follows the link.
    follow: bool,
    /// Whether to list the directory with `getattrlistbulk`, which gives the metadata of its
    /// entries, rather than `readdir`, which gives their names and types only.
    #[cfg(target_os = "macos")]
    pub bulk: bool,
}

impl Access {
    /// How the root is opened: by path, following a symlink.
    pub const fn root() -> Self {
        Self {
            parent: None,
            follow: true,
            #[cfg(target_os = "macos")]
            bulk: true,
        }
    }

    /// The descriptor of the parent directory, if the job holds it.
    pub fn parent_fd(&self) -> Option<RawFd> {
        self.parent.as_deref().map(DirFd::as_raw_fd)
    }
}

/// An open directory, not listed yet.
pub struct Opened {
    pub dir: DirFd,
    /// Whether its job asked for a bulk read.
    #[cfg(target_os = "macos")]
    pub bulk: bool,
}

impl Opened {
    pub fn stat(&self, fields: Fields) -> Option<Stat> {
        at::stat_fd(self.dir.as_raw_fd(), fields).ok()
    }
}

/// The budget of descriptors for a walk by `walker`. Parent directories stay open while their
/// subdirectories wait, so this allows many files, and holds no more of them than the limit
/// allows.
pub fn budget(walker: &Walker) -> Arc<Budget> {
    Budget::new(at::raise_open_file_limit(), walker.max_workers())
}

/// Opens the directory at `path` as `access` says: relative to its parent when the job holds
/// it, else by path. Running out of descriptors (`EMFILE`, `ENFILE`) is retried for a while, as
/// other workers close theirs; only then the directory counts as unreadable. The budget (see
/// [`crate::walk::fds`]) keeps the walk from causing this by itself, except for paths too long
/// to open by path.
pub fn open(path: &Path, access: &Access, budget: &Arc<Budget>) -> Option<Opened> {
    // The path-based walk could not read a directory whose path does not fit.
    #[cfg(target_os = "macos")]
    if !crate::fs::fits_path_max(path.as_os_str().len()) {
        return None;
    }

    for attempt in 1..=OPEN_RETRIES {
        match open_once(path, access) {
            Err(e) if matches!(e.raw_os_error(), Some(libc::EMFILE | libc::ENFILE)) => {
                thread::sleep(Duration::from_micros(100 * u64::from(attempt)));
            },
            result => {
                return result.ok().map(|fd| Opened {
                    dir: DirFd::new(fd, Arc::clone(budget)),
                    #[cfg(target_os = "macos")]
                    bulk: access.bulk,
                });
            },
        }
    }

    None
}

fn open_once(path: &Path, access: &Access) -> std::io::Result<OwnedFd> {
    if let Some(parent) = &access.parent {
        let name = path
            .file_name()
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))?
            .as_bytes();
        let opened = at::with_cstr(name, |name| {
            at::open_dir_at(parent.as_raw_fd(), name, access.follow)
        });

        match opened {
            Ok(fd) => return Ok(fd),
            // Another lookup cannot fix these.
            Err(e)
                if matches!(
                    e.raw_os_error(),
                    Some(
                        libc::ENOENT
                            | libc::EACCES
                            | libc::ENOTDIR
                            | libc::ELOOP
                            | libc::EPERM
                            | libc::EMFILE
                            | libc::ENFILE
                    )
                ) =>
            {
                return Err(e);
            },
            Err(_) => {},
        }
    }

    at::open_dir_path(path, access.follow)
}

/// Metadata of the directory at `path` looked up by name in its parent, or by path when the
/// job holds no descriptor of the parent.
pub fn lstat_dir(path: &Path, access: &Access, fields: Fields) -> Option<Stat> {
    let Some(dir) = access.parent_fd() else {
        return at::lstat_path(path, fields).ok();
    };

    let name = path.file_name()?.as_bytes();
    at::with_cstr(name, |name| at::stat_at(At { dir, name }, false, fields)).ok()
}

impl Listing {
    /// Entry `i` as the target of a relative system call.
    pub fn at(&self, i: usize) -> Option<At<'_>> {
        let e = &self.ents[i];
        let start = e.off as usize;
        let bytes = &self.names[start..=start + usize::from(e.len)];

        // SAFETY: `push` wrote a NUL after the name, and a name has no NUL inside.
        let name = unsafe { CStr::from_bytes_with_nul_unchecked(bytes) };

        Some(At {
            dir: self.dir.as_ref()?.as_raw_fd(),
            name,
        })
    }

    /// Metadata of entry `i`, following a symlink.
    pub fn stat(&self, i: usize, path: &Path, fields: Fields) -> Option<Stat> {
        #[cfg(target_os = "macos")]
        if !crate::fs::fits_path_max(path.as_os_str().len()) {
            return None;
        }
        #[cfg(target_os = "linux")]
        let _ = path;

        at::stat_at(self.at(i)?, true, fields).ok()
    }

    pub fn is_hidden(&self, i: usize) -> bool {
        self.name(i).first() == Some(&b'.')
    }

    /// The open directory, to read its ignore files relative to.
    pub fn fd(&self) -> rules::OpenDir {
        self.dir.as_deref().map(DirFd::as_raw_fd)
    }

    /// How to open the subdirectory at `path`; `follow` tells whether it is a followed symlink.
    pub fn access(&self, path: &Path, follow: bool) -> Access {
        // A path this long cannot be opened by path, so the job needs the descriptor.
        let long = path.as_os_str().len() >= LONG_PATH;

        Access {
            parent: self
                .dir
                .as_ref()
                .and_then(|parent| parent.retain(long).then(|| Arc::clone(parent))),
            follow,
            #[cfg(target_os = "macos")]
            bulk: self.bulk,
        }
    }
}

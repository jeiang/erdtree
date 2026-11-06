//! Whether an entry has extended attributes, following a symlink as `listxattr` does. A failing
//! call means no attributes.

use std::{ffi::CStr, path::Path, ptr};

/// Whether the entry at `path` has extended attributes. On Linux, `at` names it relative to its
/// open directory, which `listxattrat` (Linux 6.13) takes; without that call, or without `at`,
/// the path is used.
pub fn has_xattrs(path: &Path, #[cfg(target_os = "linux")] at: Option<super::at::At<'_>>) -> bool {
    #[cfg(target_os = "linux")]
    if let Some(has) = at.and_then(linux::list_at) {
        return has;
    }

    #[cfg(fd_walk)]
    return super::at::with_path(path, |path| Ok(list(path))).unwrap_or(false);

    #[cfg(not(fd_walk))]
    {
        use std::os::unix::ffi::OsStrExt;

        std::ffi::CString::new(path.as_os_str().as_bytes()).is_ok_and(|path| list(&path))
    }
}

/// `listxattr` of `path`, asking only for the size of the list.
fn list(path: &CStr) -> bool {
    use libc::c_char;

    // SAFETY: `path` is NUL-terminated, and a null buffer of size 0 only asks for the size of
    // the attribute list.
    unsafe {
        #[cfg(not(target_os = "macos"))]
        return 0 < libc::listxattr(path.as_ptr(), ptr::null_mut::<c_char>(), 0);

        #[cfg(target_os = "macos")]
        return 0 < libc::listxattr(path.as_ptr(), ptr::null_mut::<c_char>(), 0, 0);
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use crate::fs::{at::At, probe::Probed};
    use std::ffi::{c_char, c_long};

    /// `listxattrat`; without it, the path-based `listxattr`.
    static LISTXATTRAT: Probed = Probed::new();

    /// The number of `listxattrat`, which is the same on the architectures that have the common
    /// system call numbers after 400.
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

    /// Whether the entry `at` has extended attributes; `None` when the kernel has no
    /// `listxattrat`.
    pub fn list_at(at: At<'_>) -> Option<bool> {
        let number = SYS_LISTXATTRAT?;

        let result = LISTXATTRAT.run(|| {
            // SAFETY: `at.name` is NUL-terminated; a null list with size 0 only asks for the
            // size.
            crate::fs::at::check(unsafe {
                libc::syscall(
                    number,
                    c_long::from(at.dir),
                    at.name.as_ptr(),
                    0 as c_long,
                    std::ptr::null_mut::<c_char>(),
                    0 as libc::size_t,
                )
            })
        })?;

        Some(result.is_ok_and(|len| len > 0))
    }

    #[cfg(test)]
    mod test {
        use super::{super::has_xattrs, *};
        use crate::fs::at::open_dir_path;
        use std::{
            ffi::CString,
            fs,
            os::{fd::AsRawFd, unix::ffi::OsStrExt, unix::fs::symlink},
        };

        #[test]
        fn relative_calls_match_the_path_based_calls() {
            let dir = tempfile::tempdir().unwrap();
            let p = |name: &str| dir.path().join(name);

            fs::write(p("plain"), "x").unwrap();
            fs::write(p("tagged"), "x").unwrap();
            symlink("tagged", p("link-to-tagged")).unwrap();
            symlink("plain", p("link-to-plain")).unwrap();
            symlink("missing", p("dangling")).unwrap();

            // A user attribute; file systems without support (tmpfs in older kernels) refuse it.
            let tagged = CString::new(p("tagged").as_os_str().as_bytes()).unwrap();
            // SAFETY: valid NUL-terminated strings and a buffer of the given size.
            let set = unsafe {
                libc::setxattr(
                    tagged.as_ptr(),
                    c"user.erd".as_ptr(),
                    b"1".as_ptr().cast(),
                    1,
                    0,
                )
            };
            if set == 0 {
                assert!(has_xattrs(&p("tagged"), None));
            } else {
                let err = std::io::Error::last_os_error();
                assert_eq!(
                    err.raw_os_error(),
                    Some(libc::EOPNOTSUPP),
                    "setxattr: {err}"
                );
                eprintln!("no user xattrs here ({err}): checking entries without them only");
            }

            let fd = open_dir_path(dir.path(), true).unwrap();

            for name in [
                "plain",
                "tagged",
                "link-to-tagged",
                "link-to-plain",
                "dangling",
            ] {
                let c = CString::new(name).unwrap();
                let at = At {
                    dir: fd.as_raw_fd(),
                    name: &c,
                };

                if let Some(has) = list_at(at) {
                    assert_eq!(has, has_xattrs(&p(name), None), "{name}");
                }
                assert_eq!(
                    has_xattrs(&p(name), Some(at)),
                    has_xattrs(&p(name), None),
                    "{name}"
                );
            }
        }
    }
}

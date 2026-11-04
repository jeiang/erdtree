use std::{os::unix::ffi::OsStrExt, path::Path, ptr};

/// Checks to see if a directory entry referred to by `path` has extended attributes. If the file
/// at the provided `path` is symlink the file it points to is interrogated.
pub fn has_xattrs(path: &Path) -> bool {
    use libc::{c_char, listxattr};

    // `listxattr` needs a NUL-terminated path.
    let Ok(c_path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    let path_ptr = c_path.as_ptr().cast::<c_char>();

    // SAFETY: `path_ptr` is a valid NUL-terminated string for the duration of the call, and a
    // null buffer of size 0 only queries the size of the attribute list.
    unsafe {
        #[cfg(not(target_os = "macos"))]
        return 0 < listxattr(path_ptr, ptr::null_mut::<c_char>(), 0);

        #[cfg(target_os = "macos")]
        return 0 < listxattr(path_ptr, ptr::null_mut::<c_char>(), 0, 0);
    }
}

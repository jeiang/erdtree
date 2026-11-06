use std::{ffi::CStr, ptr};

/// Upper bound on the scratch buffer handed to the reentrant lookups.
const MAX_BUF_LEN: usize = 1 << 20;

/// Calls a reentrant `get*_r` function with a scratch buffer, doubling the buffer on `ERANGE`.
/// `lookup` receives the buffer pointer and length and returns the C return code together with
/// whether a record was found. Returns the filled buffer when found; `None` when there is no such
/// entry or the lookup failed.
fn with_buffer(
    initial_len: libc::c_int,
    mut lookup: impl FnMut(*mut libc::c_char, usize) -> (libc::c_int, bool),
) -> Option<Vec<libc::c_char>> {
    let mut len = usize::try_from(initial_len)
        .ok()
        .filter(|n| *n > 0)
        .unwrap_or(1024);

    loop {
        let mut buf: Vec<libc::c_char> = vec![0; len];
        let (code, found) = lookup(buf.as_mut_ptr(), buf.len());

        match code {
            0 if found => return Some(buf),
            libc::ERANGE if len < MAX_BUF_LEN => len *= 2,
            // Code 0 without a result means no such entry, as do `ENOENT`, `ESRCH`, `EBADF`
            // and `EPERM` on some libcs; other codes are errors.
            _ => return None,
        }
    }
}

/// The name of the group `gid`, if it has one.
pub fn group_name(gid: libc::gid_t) -> Option<String> {
    // SAFETY: `sysconf` has no preconditions.
    let size = unsafe { libc::sysconf(libc::_SC_GETGR_R_SIZE_MAX) };
    let size = libc::c_int::try_from(size).unwrap_or(0);

    let mut grp: std::mem::MaybeUninit<libc::group> = std::mem::MaybeUninit::uninit();

    let buf = with_buffer(size, |buf, len| {
        let mut result: *mut libc::group = ptr::null_mut();
        // SAFETY: `grp` and `buf` are valid for the call; `buf` is `len` bytes long. The result
        // is only read by the caller when `result` is non-null, in which case `grp` is initialized.
        let code = unsafe { libc::getgrgid_r(gid, grp.as_mut_ptr(), buf, len, &raw mut result) };
        (code, !result.is_null())
    })?;

    // SAFETY: `with_buffer` returned a buffer only when `getgrgid_r` found a record, which
    // initialized `grp` with `gr_name` pointing into `buf` (still alive here).
    let name = unsafe { CStr::from_ptr(grp.assume_init().gr_name) };
    let name = name.to_string_lossy().into_owned();
    drop(buf);

    Some(name)
}

/// The name of the user `uid`, if it has one.
pub fn user_name(uid: libc::uid_t) -> Option<String> {
    // SAFETY: `sysconf` has no preconditions.
    let size = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
    let size = libc::c_int::try_from(size).unwrap_or(0);

    let mut pwd: std::mem::MaybeUninit<libc::passwd> = std::mem::MaybeUninit::uninit();

    let buf = with_buffer(size, |buf, len| {
        let mut result: *mut libc::passwd = ptr::null_mut();
        // SAFETY: `pwd` and `buf` are valid for the call; `buf` is `len` bytes long. The result
        // is only read by the caller when `result` is non-null, in which case `pwd` is initialized.
        let code = unsafe { libc::getpwuid_r(uid, pwd.as_mut_ptr(), buf, len, &raw mut result) };
        (code, !result.is_null())
    })?;

    // SAFETY: `with_buffer` returned a buffer only when `getpwuid_r` found a record, which
    // initialized `pwd` with `pw_name` pointing into `buf` (still alive here).
    let name = unsafe { CStr::from_ptr(pwd.assume_init().pw_name) };
    let name = name.to_string_lossy().into_owned();
    drop(buf);

    Some(name)
}

#[cfg(test)]
mod test {
    use super::*;
    use std::thread;

    #[test]
    fn lookups_are_consistent_across_threads() {
        let ids = [0, unsafe { libc::geteuid() }];
        let expected: Vec<_> = ids
            .iter()
            .map(|id| (user_name(*id), group_name(*id)))
            .collect();
        // Every system names user 0, and group 0 (`root`, or `wheel` on macOS).
        assert_eq!(expected[0].0.as_deref(), Some("root"));
        assert!(expected[0].1.is_some());

        let handles: Vec<_> = (0..16)
            .map(|_| {
                let expected = expected.clone();
                thread::spawn(move || {
                    for _ in 0..200 {
                        for (id, want) in ids.iter().zip(&expected) {
                            assert_eq!(&(user_name(*id), group_name(*id)), want);
                        }
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }
    }

    #[test]
    fn missing_ids_are_not_found() {
        assert_eq!(user_name(987_654_321), None);
        assert_eq!(group_name(987_654_321), None);
    }
}

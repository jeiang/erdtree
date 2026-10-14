use errno::Errno;
use std::{ffi::CStr, fs::Metadata, os::unix::fs::MetadataExt, ptr};

type Owner = String;
type Group = String;

impl UserGroupInfo for Metadata {}

/// Trait that allows for files to query their owner and group.
pub trait UserGroupInfo: MetadataExt {
    /// Attempts to query both the owner and group of the implementor.
    fn try_get_owner_and_group(&self) -> Result<(Owner, Group), Error> {
        let user = try_get_user(self.uid())?;
        let group = try_get_group(self.gid())?;

        Ok((user, group))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("libc error")]
    LibC(Errno),

    #[error("Invalid user")]
    InvalidUser,

    #[error("Invalid group")]
    InvalidGroup,
}

/// Upper bound on the scratch buffer handed to the reentrant lookups.
const MAX_BUF_LEN: usize = 1 << 20;

/// Calls a reentrant `get*_r` function with a scratch buffer, doubling the buffer on `ERANGE`.
/// `lookup` receives the buffer pointer and length and returns the C return code together with
/// whether a record was found. Returns `Ok(Some(buf))` with the filled buffer when found.
fn with_buffer(
    initial_len: libc::c_int,
    mut lookup: impl FnMut(*mut libc::c_char, usize) -> (libc::c_int, bool),
) -> Result<Option<Vec<libc::c_char>>, Errno> {
    let mut len = usize::try_from(initial_len)
        .ok()
        .filter(|n| *n > 0)
        .unwrap_or(1024);

    loop {
        let mut buf: Vec<libc::c_char> = vec![0; len];
        let (code, found) = lookup(buf.as_mut_ptr(), buf.len());

        match code {
            0 if found => return Ok(Some(buf)),
            0 => return Ok(None),
            libc::ERANGE if len < MAX_BUF_LEN => len *= 2,
            // Some libcs report a missing entry through the return code.
            libc::ENOENT | libc::ESRCH | libc::EBADF | libc::EPERM => return Ok(None),
            code => return Err(Errno(code)),
        }
    }
}

/// Attempts to return the name of the group associated with `gid`.
fn try_get_group(gid: libc::gid_t) -> Result<String, Error> {
    // SAFETY: `sysconf` has no preconditions.
    let size = unsafe { libc::sysconf(libc::_SC_GETGR_R_SIZE_MAX) };
    let size = libc::c_int::try_from(size).unwrap_or(0);

    let mut grp: std::mem::MaybeUninit<libc::group> = std::mem::MaybeUninit::uninit();

    let buf = with_buffer(size, |buf, len| {
        let mut result: *mut libc::group = ptr::null_mut();
        // SAFETY: `grp` and `buf` are valid for the call; `buf` is `len` bytes long. The result
        // is only read by the caller when `result` is non-null, in which case `grp` is initialized.
        let code = unsafe { libc::getgrgid_r(gid, grp.as_mut_ptr(), buf, len, &mut result) };
        (code, !result.is_null())
    })
    .map_err(Error::LibC)?
    .ok_or(Error::InvalidGroup)?;

    // SAFETY: `with_buffer` returned a buffer only when `getgrgid_r` found a record, which
    // initialized `grp` with `gr_name` pointing into `buf` (still alive here).
    let name = unsafe { CStr::from_ptr(grp.assume_init().gr_name) };
    let name = name.to_string_lossy().into_owned();
    drop(buf);

    Ok(name)
}

/// Attempts to return the name of the user associated with `uid`.
fn try_get_user(uid: libc::uid_t) -> Result<String, Error> {
    // SAFETY: `sysconf` has no preconditions.
    let size = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
    let size = libc::c_int::try_from(size).unwrap_or(0);

    let mut pwd: std::mem::MaybeUninit<libc::passwd> = std::mem::MaybeUninit::uninit();

    let buf = with_buffer(size, |buf, len| {
        let mut result: *mut libc::passwd = ptr::null_mut();
        // SAFETY: `pwd` and `buf` are valid for the call; `buf` is `len` bytes long. The result
        // is only read by the caller when `result` is non-null, in which case `pwd` is initialized.
        let code = unsafe { libc::getpwuid_r(uid, pwd.as_mut_ptr(), buf, len, &mut result) };
        (code, !result.is_null())
    })
    .map_err(Error::LibC)?
    .ok_or(Error::InvalidUser)?;

    // SAFETY: `with_buffer` returned a buffer only when `getpwuid_r` found a record, which
    // initialized `pwd` with `pw_name` pointing into `buf` (still alive here).
    let name = unsafe { CStr::from_ptr(pwd.assume_init().pw_name) };
    let name = name.to_string_lossy().into_owned();
    drop(buf);

    Ok(name)
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
            .map(|id| (try_get_user(*id).ok(), try_get_group(*id).ok()))
            .collect();

        let handles: Vec<_> = (0..16)
            .map(|_| {
                let expected = expected.clone();
                thread::spawn(move || {
                    for _ in 0..200 {
                        for (id, want) in ids.iter().zip(&expected) {
                            assert_eq!(&(try_get_user(*id).ok(), try_get_group(*id).ok()), want);
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
        assert!(matches!(try_get_user(987_654_321), Err(Error::InvalidUser)));
        assert!(matches!(
            try_get_group(987_654_321),
            Err(Error::InvalidGroup)
        ));
    }
}

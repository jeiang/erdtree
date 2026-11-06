//! System calls newer than the oldest kernel erdtree runs on (`statx`, `listxattrat`), which
//! a kernel or a sandbox may lack. Each has a [`Probed`] latch, so a missing call is tried once
//! and its fallback used from then on.

use std::{
    io,
    sync::atomic::{AtomicU8, Ordering},
};

const UNKNOWN: u8 = 0;
const AVAILABLE: u8 = 1;
const MISSING: u8 = 2;

/// Whether a system call is available: unknown until it first answers, then settled. The
/// latch is written only when it settles, so calls after that only read it.
///
/// Until the call has once succeeded, it counts as missing when it fails with `ENOSYS` (the
/// kernel does not know it), `EPERM` (a seccomp filter, as container runtimes install, blocks
/// it) or `EINVAL` (the kernel rejects its arguments). After a success, every error is the
/// file's.
pub struct Probed(AtomicU8);

impl Probed {
    pub const fn new() -> Self {
        Self(AtomicU8::new(UNKNOWN))
    }

    /// The result of `call`; `None` when the system call is missing, so the caller must use its
    /// fallback.
    pub fn run<T>(&self, call: impl FnOnce() -> io::Result<T>) -> Option<io::Result<T>> {
        let state = self.0.load(Ordering::Relaxed);

        if state == MISSING {
            return None;
        }

        let result = call();

        if state == UNKNOWN {
            match &result {
                Ok(_) => self.0.store(AVAILABLE, Ordering::Relaxed),
                Err(e)
                    if matches!(
                        e.raw_os_error(),
                        Some(libc::ENOSYS | libc::EPERM | libc::EINVAL)
                    ) =>
                {
                    self.0.store(MISSING, Ordering::Relaxed);
                    return None;
                },
                Err(_) => {},
            }
        }

        Some(result)
    }
}

#[cfg(test)]
mod test {
    use super::Probed;
    use std::io;

    fn err(code: i32) -> io::Result<()> {
        Err(io::Error::from_raw_os_error(code))
    }

    #[test]
    fn missing_settles_only_before_a_success() {
        let p = Probed::new();
        assert!(
            p.run(|| err(libc::ENOENT)).unwrap().is_err(),
            "a file's error"
        );
        assert!(p.run(|| Ok(())).unwrap().is_ok());
        assert!(
            p.run(|| err(libc::EPERM)).unwrap().is_err(),
            "settled as available"
        );

        let p = Probed::new();
        assert!(p.run(|| err(libc::ENOSYS)).is_none());
        assert!(
            p.run(|| -> io::Result<()> { panic!("not called") })
                .is_none()
        );
    }
}

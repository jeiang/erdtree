//! Bounds the directory descriptors that queued jobs keep open. A job opens its directory
//! relative to the descriptor of its parent, so the parent stays open while any of its
//! subdirectories waits. In a deep tree that would exhaust `RLIMIT_NOFILE`, whose hard limit is
//! 1024 in some containers, and the walk would lose the subtrees it could not open.
//!
//! A [`Budget`] counts the descriptors queued jobs may hold. A job that finds the budget spent
//! keeps no descriptor and opens its directory by path. Paths that do not fit `PATH_MAX` cannot
//! be opened by path, so those jobs hold their parent regardless of the budget; under a tiny limit
//! a chain of such directories, each with a sibling still waiting, can still run out of descriptors.

use std::{
    os::fd::{AsRawFd, OwnedFd, RawFd},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicIsize, Ordering},
    },
};

/// Descriptors a walk keeps for itself: the directory each worker reads, the files it reads
/// (`--disk-usage line`), and the process's own (standard streams, configuration, and so on).
const RESERVED: usize = 64;
const PER_WORKER: usize = 4;

/// A subdirectory whose path, its own name included, is this long or longer keeps its parent's
/// descriptor even when the budget is spent, as opening it by path may fail near `PATH_MAX`.
pub const LONG_PATH: usize = crate::fs::PATH_MAX - 96;

/// How many descriptors queued jobs may hold.
#[derive(Debug)]
pub struct Budget {
    free: AtomicIsize,
}

impl Budget {
    /// A budget for a walk of up to `workers` workers under a limit of `limit` open files.
    pub fn new(limit: usize, workers: usize) -> Arc<Self> {
        let reserved = RESERVED + PER_WORKER * workers;
        let free = limit.saturating_sub(reserved).min(1 << 20);

        Arc::new(Self {
            free: AtomicIsize::new(isize::try_from(free).unwrap_or(isize::MAX)),
        })
    }

    fn take(&self, force: bool) -> bool {
        if force {
            self.free.fetch_sub(1, Ordering::Relaxed);
            return true;
        }

        let mut free = self.free.load(Ordering::Relaxed);

        while free > 0 {
            match self.free.compare_exchange_weak(
                free,
                free - 1,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(now) => free = now,
            }
        }

        false
    }

    fn give(&self) {
        self.free.fetch_add(1, Ordering::Relaxed);
    }

    /// Descriptors left; for tests.
    #[cfg(test)]
    pub fn free(&self) -> isize {
        self.free.load(Ordering::Relaxed)
    }
}

/// An open directory that queued jobs may share. It costs one unit of the budget from the first
/// job that holds it until the last one lets go.
#[derive(Debug)]
pub struct DirFd {
    fd: OwnedFd,
    budget: Arc<Budget>,
    held: AtomicBool,
}

impl DirFd {
    pub const fn new(fd: OwnedFd, budget: Arc<Budget>) -> Self {
        Self {
            fd,
            budget,
            held: AtomicBool::new(false),
        }
    }

    pub fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    /// Whether a job may keep this descriptor open until it runs. With `force` it may even when
    /// the budget is spent, because the job cannot open its directory by path.
    pub fn retain(&self, force: bool) -> bool {
        if self.held.load(Ordering::Acquire) {
            return true;
        }

        if !self.budget.take(force) {
            return false;
        }

        // Another worker took it between the load and here: it pays, not this one.
        if self.held.swap(true, Ordering::AcqRel) {
            self.budget.give();
        }

        true
    }
}

impl Drop for DirFd {
    fn drop(&mut self) {
        if *self.held.get_mut() {
            self.budget.give();
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::fs::File;

    #[test]
    fn a_spent_budget_refuses_unless_forced_and_is_given_back() {
        let budget = Budget::new(RESERVED + PER_WORKER + 1, 1);
        assert_eq!(budget.free(), 1);

        let a = DirFd::new(File::open("/").unwrap().into(), Arc::clone(&budget));
        let b = DirFd::new(File::open("/").unwrap().into(), Arc::clone(&budget));

        assert!(
            a.retain(false) && a.retain(false),
            "holding twice costs once"
        );
        assert_eq!(budget.free(), 0);
        assert!(!b.retain(false));
        assert!(b.retain(true));
        assert_eq!(budget.free(), -1);

        drop(a);
        drop(b);
        assert_eq!(budget.free(), 1);
    }
}

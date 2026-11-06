//! Parallel filesystem walk. Each worker reads whole directories and appends one record per kept
//! entry to its own [`Sink`], so no per-entry synchronization occurs. Directories are distributed
//! with per-worker LIFO stacks and a shared queue that is only used when another worker is idle.
//!
//! Entry selection matches [`ignore::WalkParallel`] as `erd` configured it before: the same ignore
//! rules ([`rules`]), hidden-file handling, `--exclude`/`--no-git` overrides, entry filter,
//! depth limit, same-file-system check and symlink loop detection. Entries whose metadata cannot
//! be read are dropped, and so are their contents.
//!
//! A worker lists a directory into a [`Listing`] with its [`Lister`], the platform's way to read
//! a directory: on Linux and macOS relative to its parent's file descriptor, elsewhere with
//! `std::fs::read_dir` (see [`listing`]).

use crate::{
    fs::stat::{Fields, Stat},
    sync::unpoisoned,
};
use std::{
    ffi::{OsStr, OsString},
    fs, mem,
    path::{MAIN_SEPARATOR, Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    thread,
    time::Instant,
};

#[cfg(fd_walk)]
use crate::fs::at::{self, At};
#[cfg(fd_walk)]
use std::os::unix::ffi::OsStrExt;

/// Detects workers that wait for the file system.
mod adaptive;
/// Bounds the descriptors that queued jobs hold.
#[cfg(fd_walk)]
mod fds;
/// Hidden entries, `--exclude`, `--no-git` and the entry filter.
pub mod filter;
/// The entries of one directory, and how each platform lists them.
mod listing;
/// Ignore rules per directory, adapted from the `ignore` crate.
pub mod rules;

use filter::EntryFilter;
use listing::{Access, Lister, Listing, Sample};
use rules::{Found, Rules};

/// Identifies a record: the index of the sink that holds it and its index in that sink.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeRef {
    pub sink: u32,
    pub index: u32,
}

/// An entry kept by the walk.
pub struct Record<'a> {
    /// The containing directory; `None` for the root.
    pub parent: Option<NodeRef>,
    pub depth: usize,
    pub name: &'a OsStr,
    pub path: &'a Path,
    /// Metadata of the entry itself, or of its target when the entry is a followed symlink.
    pub stat: &'a Stat,
    /// Whether the entry is a symlink (followed or not).
    pub is_symlink: bool,
    /// Where to find the entry relative to its open directory; `None` for the root.
    #[cfg(fd_walk)]
    pub at: Option<At<'a>>,
}

impl Record<'_> {
    /// Whether the entry has extended attributes (following a symlink).
    #[cfg(unix)]
    pub fn has_xattrs(&self) -> bool {
        crate::fs::xattr::has_xattrs(
            self.path,
            #[cfg(target_os = "linux")]
            self.at,
        )
    }

    /// Whether the target of the entry, which must be a symlink, exists.
    pub fn target_exists(&self) -> bool {
        #[cfg(fd_walk)]
        if let Some(at) = self.at {
            return at::target_exists(at);
        }

        self.path.exists()
    }

    /// Opens the entry for reading, following a symlink.
    pub fn open(&self) -> std::io::Result<fs::File> {
        #[cfg(fd_walk)]
        if let Some(at) = self.at {
            return at::open_file(at);
        }

        fs::File::open(self.path)
    }

    /// The target of the entry, which must be a symlink.
    pub fn read_link(&self) -> std::io::Result<PathBuf> {
        #[cfg(fd_walk)]
        if let Some(at) = self.at {
            return at::read_link(at);
        }

        fs::read_link(self.path)
    }
}

/// Receives the records of one worker.
pub trait Sink: Send {
    /// Stores `record` and returns its index in this sink, or `None` to drop the entry (and,
    /// for a directory, its contents).
    fn push(&mut self, record: &Record<'_>) -> Option<u32>;

    /// Adds entries that a walked directory listed but the walk skipped (`--report-ignored`).
    fn add_ignored(&mut self, count: usize);
}

pub struct Walker {
    pub root: PathBuf,
    pub rules: Rules,
    pub follow_links: bool,
    pub same_file_system: bool,
    pub max_depth: Option<usize>,
    /// Workers the walk starts with; at least 1 (`Context::try_init` resolves `--threads 0`).
    pub threads: usize,
    /// Most workers the walk may have; at least `threads` (see [`max_threads`]). It starts with
    /// `threads` and adds workers while they wait for the file system (a cold cache, a network
    /// file system); `threads` keeps the number fixed.
    pub max_threads: usize,
    pub filter: Option<EntryFilter>,
    /// Count ignored entries per directory (`--report-ignored`).
    pub count_ignored: bool,
    /// Optional metadata fields to read for every entry.
    pub fields: Fields,
    /// How large directories are shared between workers.
    pub tuning: Tuning,
}

/// Factor by which the walk may add workers when it waits for the file system.
const GROWTH: usize = 8;

/// Most workers when the walk adds workers.
const GROWN_MAX: usize = 64;

/// The [`Walker::max_threads`] of a walk that starts with `threads` workers. With `grow` the walk
/// adds workers that wait for the file system, where it can tell that they do
/// ([`adaptive::MEASURES`]); otherwise it keeps `threads`.
pub fn max_threads(threads: usize, grow: bool) -> usize {
    if grow && adaptive::MEASURES {
        (threads * GROWTH).min(GROWN_MAX).max(threads)
    } else {
        threads
    }
}

/// Directories with at least this many entries can be split between workers.
pub const SPLIT_MIN: usize = 1024;

/// The thresholds when every file is read, chosen by measurement: 2 and 1 took a directory of 8
/// files of 72 MB from 101 ms to 46 ms (`--disk-usage line`), 64 and 8 did not split it at all,
/// and trees of many small files (repo, wide, 1M files) ran as fast as with the defaults.
const READING_SPLIT_MIN: usize = 2;
const READING_SPLIT_BLOCK: usize = 1;

/// Entries per block that idle workers take over from a directory of [`SPLIT_MIN`] entries.
const SPLIT_BLOCK: usize = SPLIT_MIN / 8;

/// Knobs of the walk; tests and walks that read every file change them. The counters are read
/// only by tests; each increment sits next to far costlier work (handing a block to another
/// worker, starting a thread).
pub struct Tuning {
    /// Directories with at least this many entries are visited in blocks that idle workers take
    /// over. Never splits when `usize::MAX`.
    pub split_min: usize,
    /// Entries per block.
    pub block: usize,
    /// Number of times a directory was split; for tests.
    pub splits: AtomicUsize,
    /// When workers count as waiting for the file system.
    pub policy: adaptive::Policy,
    /// Number of workers added; for tests.
    pub grown: AtomicUsize,
    /// Number of workers started besides the first; for tests.
    pub started: AtomicUsize,
}

impl Tuning {
    /// Splits directories of `split_min` entries or more in blocks of `block` entries.
    pub fn splitting(split_min: usize, block: usize) -> Self {
        Self {
            split_min,
            block: block.max(1),
            ..Self::default()
        }
    }

    /// For walks that read every file (`--disk-usage line` and `word`). Each entry then costs a
    /// whole file read, so even a directory of a few large files has work for every worker.
    pub fn reading_files() -> Self {
        Self::splitting(READING_SPLIT_MIN, READING_SPLIT_BLOCK)
    }
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            split_min: SPLIT_MIN,
            block: SPLIT_BLOCK,
            splits: AtomicUsize::new(0),
            policy: adaptive::Policy::default(),
            grown: AtomicUsize::new(0),
            started: AtomicUsize::new(0),
        }
    }
}

/// Where the record of a queued directory is.
#[derive(Clone, Copy)]
enum Me {
    /// Already stored, at `id`. `foreign` tells whether the directory is on another file system
    /// than the root (`-x`).
    Pushed { id: NodeRef, foreign: bool },
    /// Not stored yet: whoever reads the directory stores it, below this parent. Reading the
    /// directory first gives its metadata for free (`statx` or `fstat` of the open directory),
    /// instead of a second lookup of its name.
    #[cfg(fd_walk)]
    Deferred(NodeRef),
}

/// Work for a worker.
enum Task {
    Dir(Job),
    /// Visit part of the entries of a directory another worker listed.
    Chunk(Chunk),
}

/// The entries `lo..hi` of a listed directory.
struct Chunk {
    dir: Arc<Dir>,
    lo: usize,
    hi: usize,
}

/// A directory waiting to be read.
struct Job {
    path: PathBuf,
    me: Me,
    /// Depth of the directory itself.
    depth: usize,
    /// Rules of the parent directory.
    rules: Rules,
    /// How to open and list the directory.
    access: Access,
}

/// Work shared between workers.
struct Shared {
    queue: Mutex<Vec<Task>>,
    wake: Condvar,
    /// Jobs queued or being processed. Zero means the walk is complete.
    pending: AtomicUsize,
    /// Workers blocked on `wake`.
    idle: AtomicUsize,
    quit: AtomicBool,
}

/// Everything the workers of one walk share, including what an extra worker needs to start.
struct Run<'a, S, F> {
    walker: &'a Walker,
    shared: Shared,
    progress: Option<&'a AtomicU64>,
    make_sink: F,
    /// Number of workers the walk may have now: `threads`, and more once it grows.
    workers: AtomicUsize,
    /// The sink of each worker, by worker index: those of the first `threads` workers (worker 0
    /// also holds the root), then those of extra workers once the walk grows. Sinks are made in
    /// index order and never removed, so the slots holding one are a prefix and a sink's index
    /// is its position in what [`Walker::run`] returns. A running worker holds its sink and puts
    /// it back when it is done.
    pool: Vec<Mutex<Option<S>>>,
    /// Index of the next of the first `threads` workers to start. Worker 0 starts with the
    /// walk; the others start when work is available for them.
    next: AtomicUsize,
    /// Device of the root, with `--one-file-system`.
    root_device: Option<u64>,
    /// The lister each worker copies: it holds what the workers' listers share, such as the
    /// budget of directory descriptors queued jobs may hold.
    lister: Lister,
}

/// Runs the worker with index `index` until the walk is done. It takes its sink from
/// `run.pool` and puts it back there.
fn run_worker<'scope, 'env, S, F>(
    scope: &'scope thread::Scope<'scope, 'env>,
    run: &'env Run<'env, S, F>,
    index: usize,
    local: Vec<Task>,
) where
    S: Sink + 'env,
    F: Fn() -> S + Sync + 'env,
{
    let slot = &run.pool[index];
    let Some(sink) = unpoisoned(slot.lock()).take() else {
        return;
    };

    let guard = QuitOnPanic(&run.shared);
    let mut worker = Worker {
        walker: run.walker,
        sink_index: crate::tree::id::id(index),
        shared: &run.shared,
        run,
        scope,
        meter: (run.walker.max_threads > run.walker.threads)
            .then(|| adaptive::Meter::new(run.walker.tuning.policy)),
        sink,
        local,
        progress: run.progress,
        listing: Listing::default(),
        lister: run.lister.for_worker(),
        path_buf: Vec::new(),
    };
    worker.run();
    mem::forget(guard);
    *unpoisoned(slot.lock()) = Some(worker.sink);
}

/// Starts a thread for the worker with index `index`, whose sink is in `run.pool`. A refused
/// thread leaves the sink there.
fn spawn_worker<'scope, 'env, S, F>(
    scope: &'scope thread::Scope<'scope, 'env>,
    run: &'env Run<'env, S, F>,
    index: usize,
) -> std::io::Result<()>
where
    S: Sink + 'env,
    F: Fn() -> S + Sync + 'env,
{
    thread::Builder::new().spawn_scoped(scope, move || {
        run_worker(scope, run, index, vec![]);
    })?;

    Ok(())
}

impl Walker {
    /// Walks the tree, with a sink from `make_sink` per worker. Returns every sink, by worker
    /// index (the index [`NodeRef::sink`] refers to): the root is the first record of the first
    /// sink. `progress` counts kept entries below the root.
    pub fn run<S: Sink, F: Fn() -> S + Sync>(
        &self,
        make_sink: F,
        progress: Option<&AtomicU64>,
    ) -> Vec<S> {
        let threads = self.threads;
        debug_assert!(
            self.max_threads >= threads,
            "max_threads is at least threads"
        );

        // The pool has room for the sinks of extra workers too.
        let mut pool: Vec<Mutex<Option<S>>> = (0..self.max_threads)
            .map(|i| Mutex::new((i < threads).then(&make_sink)))
            .collect();

        let root = unpoisoned(pool[0].get_mut())
            .as_mut()
            .and_then(|sink| self.root_job(sink));

        if let Some((root, root_device)) = root {
            // On Linux and macOS this raises the limit of open files and sets the budget of
            // descriptors, so only a walk that reads a directory does it.
            let lister = Lister::new(self);

            let run = Run {
                walker: self,
                shared: Shared {
                    queue: Mutex::new(Vec::new()),
                    wake: Condvar::new(),
                    pending: AtomicUsize::new(1),
                    idle: AtomicUsize::new(0),
                    quit: AtomicBool::new(false),
                },
                progress,
                make_sink,
                workers: AtomicUsize::new(threads),
                pool,
                next: AtomicUsize::new(1),
                root_device,
                lister,
            };

            // Each worker owns its sink, so no two threads write next to each other in memory.
            // Worker 0 runs on this thread; a tree with no work to share never starts another.
            thread::scope(|s| run_worker(s, &run, 0, vec![Task::Dir(root)]));
            pool = run.pool;
        }

        pool.into_iter()
            .map_while(|slot| unpoisoned(slot.into_inner()))
            .collect()
    }

    /// Records the root in `sink`, the sink of worker 0, and returns the job that reads it and
    /// the device of the root (with `--one-file-system`).
    fn root_job<S: Sink>(&self, sink: &mut S) -> Option<(Job, Option<u64>)> {
        let metadata = fs::symlink_metadata(&self.root).ok()?;
        let stat = Stat::from_metadata(&metadata, self.fields);
        let name = self
            .root
            .file_name()
            .unwrap_or_else(|| self.root.as_os_str());

        let index = sink.push(&Record {
            parent: None,
            depth: 0,
            name,
            path: &self.root,
            stat: &stat,
            is_symlink: stat.kind.is_symlink(),
            #[cfg(fd_walk)]
            at: None,
        })?;

        if !stat.kind.is_dir() || self.max_depth == Some(0) {
            return None;
        }

        // A directory root is not a symlink, so its own metadata is its target's.
        let root_device = if self.same_file_system {
            crate::fs::inode::device_of(&stat, &self.root)
        } else {
            None
        };

        let job = Job {
            path: self.root.clone(),
            me: Me::Pushed {
                id: NodeRef { sink: 0, index },
                foreign: false,
            },
            depth: 0,
            rules: self.rules.add_parents(&self.root),
            access: Access::root(),
        };

        Some((job, root_device))
    }
}

/// Stops the other workers if one panics, so the scope can unwind instead of waiting forever.
struct QuitOnPanic<'a>(&'a Shared);

impl Drop for QuitOnPanic<'_> {
    fn drop(&mut self) {
        self.0.quit.store(true, Ordering::SeqCst);
        let _guard = self.0.queue.lock();
        self.0.wake.notify_all();
    }
}

/// A directory being walked: its entries and what its entries are judged by.
struct Dir {
    listing: Listing,
    path: PathBuf,
    /// The rules for the entries of the directory.
    rules: Rules,
    /// The record of the directory.
    id: NodeRef,
    /// Depth of the directory itself.
    depth: usize,
}

/// An entry the walk kept.
enum Kept {
    /// Recorded, and not descended into.
    Leaf,
    /// A directory to read.
    Dir {
        me: Me,
        /// Whether it is a symlink that was followed.
        follow: bool,
    },
}

/// What a task did with the entries it visited: the progress counter counts the recorded ones,
/// and `--report-ignored` the rest of the listed ones.
#[derive(Clone, Copy, Default)]
struct Tally {
    /// Entries listed. A directory whose own job stores its record is listed again by that job,
    /// which then records it or counts it as ignored.
    listed: usize,
    recorded: usize,
    /// Directories kept whose own job stores their record.
    deferred: usize,
}

impl std::ops::AddAssign for Tally {
    fn add_assign(&mut self, other: Self) {
        self.listed += other.listed;
        self.recorded += other.recorded;
        self.deferred += other.deferred;
    }
}

struct Worker<'scope, 'env, S, F> {
    walker: &'env Walker,
    sink_index: u32,
    shared: &'env Shared,
    run: &'env Run<'env, S, F>,
    scope: &'scope thread::Scope<'scope, 'env>,
    /// Judges whether the worker waits for the file system; `None` once the walk cannot grow.
    meter: Option<adaptive::Meter>,
    sink: S,
    local: Vec<Task>,
    progress: Option<&'env AtomicU64>,
    /// Buffers reused from directory to directory.
    listing: Listing,
    lister: Lister,
    path_buf: Vec<u8>,
}

impl<'env, S, F> Worker<'_, 'env, S, F>
where
    S: Sink + 'env,
    F: Fn() -> S + Sync + 'env,
{
    fn run(&mut self) {
        while let Some(task) = self.next_job() {
            // Tasks are timed only while the walk may grow.
            let started = self.meter.is_some().then(Instant::now);
            let tally = match task {
                Task::Dir(job) => self.read_dir(job),
                Task::Chunk(chunk) => self.walk_chunk(&chunk.dir, chunk.lo, chunk.hi),
            };

            if let (Some(meter), Some(started)) = (&mut self.meter, started)
                && meter.task(started.elapsed())
                && !self.grow()
            {
                self.meter = None;
            }

            self.account(tally);
            self.share();

            if self.shared.pending.fetch_sub(1, Ordering::AcqRel) == 1 {
                // Last job done: wake everyone so they observe completion.
                let _guard = self.shared.queue.lock();
                self.shared.wake.notify_all();
            }
        }
    }

    /// Counts the entries a task visited.
    fn account(&mut self, tally: Tally) {
        if let Some(progress) = self.progress {
            progress.fetch_add(
                u64::try_from(tally.recorded).unwrap_or(u64::MAX),
                Ordering::Relaxed,
            );
        }

        if self.walker.count_ignored {
            self.sink
                .add_ignored(tally.listed - tally.recorded - tally.deferred);
        }
    }

    /// Whether the directory at `path` with metadata `stat` is on another file system than the
    /// root (`--one-file-system`).
    fn is_foreign(&self, stat: &Stat, path: &Path) -> bool {
        self.run.root_device.is_some_and(|root| {
            crate::fs::inode::device_of(stat, path).is_some_and(|dev| dev != root)
        })
    }

    /// Doubles the number of workers, up to the limit, because the workers wait for the file
    /// system. The new workers start idle, so the busy ones hand them directories. Returns
    /// whether the walk may still grow.
    fn grow(&self) -> bool {
        let run = self.run;
        let cap = self.walker.max_threads;
        let mut have = run.workers.load(Ordering::Relaxed);

        loop {
            if have >= cap {
                return false;
            }

            let want = (have * 2).min(cap);
            match run
                .workers
                .compare_exchange(have, want, Ordering::AcqRel, Ordering::Relaxed)
            {
                Ok(_) => break,
                Err(now) => have = now,
            }
        }

        let want = (have * 2).min(cap);
        // Every sink of the range exists before any worker starts, so a refused thread leaves no
        // gap in the numbering.
        for index in have..want {
            *unpoisoned(run.pool[index].lock()) = Some((run.make_sink)());
        }

        for index in have..want {
            self.walker.tuning.grown.fetch_add(1, Ordering::Relaxed);

            // A refused thread leaves the walk with the workers it has.
            if spawn_worker(self.scope, run, index).is_err() {
                run.workers.store(cap, Ordering::Relaxed);
                return false;
            }
        }

        want < cap
    }

    /// Starts the next worker that has not started yet, if there is one. Returns whether it did.
    fn start_helper(&self) -> bool {
        let run = self.run;
        let threads = self.walker.threads;

        if run.next.load(Ordering::Relaxed) >= threads {
            return false;
        }

        let index = run.next.fetch_add(1, Ordering::Relaxed);
        if index >= threads {
            return false;
        }

        if spawn_worker(self.scope, run, index).is_err() {
            // The sink stays in the pool; the walk goes on with the workers it has.
            run.next.store(threads, Ordering::Relaxed);
            return false;
        }

        self.walker.tuning.started.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// Moves half of the local stack to the shared queue when a worker is idle, or when one can
    /// be started for it.
    fn share(&mut self) {
        if self.local.len() < 2 {
            return;
        }

        if self.shared.idle.load(Ordering::Relaxed) == 0 && !self.start_helper() {
            return;
        }

        // The oldest jobs are the shallowest, so they carry the most work.
        let half = self.local.len() / 2;
        let given = self.local.drain(..half);

        let mut queue = unpoisoned(self.shared.queue.lock());
        queue.extend(given);
        drop(queue);
        self.shared.wake.notify_all();
    }

    // The queue lock is held from the emptiness check until `wait` releases it, so a worker
    // cannot miss the notification of the last job finishing.
    #[allow(clippy::significant_drop_tightening)]
    fn next_job(&mut self) -> Option<Task> {
        if let Some(job) = self.local.pop() {
            return Some(job);
        }

        let mut queue = unpoisoned(self.shared.queue.lock());

        loop {
            if self.shared.quit.load(Ordering::SeqCst) {
                return None;
            }

            if let Some(job) = queue.pop() {
                return Some(job);
            }

            if self.shared.pending.load(Ordering::Acquire) == 0 {
                return None;
            }

            self.shared.idle.fetch_add(1, Ordering::Relaxed);
            if let Some(meter) = &mut self.meter {
                meter.waited();
            }
            queue = unpoisoned(self.shared.wake.wait(queue));
            self.shared.idle.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Reads one directory, records its kept entries and queues its subdirectories.
    fn read_dir(&mut self, job: Job) -> Tally {
        let mut tally = Tally::default();
        let Some((id, listing, found, stat)) = self.open(&job, &mut tally) else {
            return tally;
        };

        let rules = job.rules.add_child(
            &job.path,
            found,
            self.walker.follow_links,
            stat.as_ref(),
            listing.fd(),
        );
        let dir = Dir {
            listing,
            path: job.path,
            rules,
            id,
            depth: job.depth,
        };

        let len = dir.listing.len();

        if len >= self.walker.tuning.split_min && self.walker.max_threads > 1 {
            // Idle workers may take over parts of a directory this large.
            let dir = Arc::new(dir);
            tally += self.walk_chunk(&dir, 0, len);

            if let Ok(dir) = Arc::try_unwrap(dir) {
                self.recycle(dir.listing);
            }
        } else {
            tally += self.walk_entries(&dir, 0, len);
            self.recycle(dir.listing);
        }

        tally
    }

    /// Keeps the buffers of `listing` for the next directory, but not the directory's file
    /// descriptor.
    fn recycle(&mut self, mut listing: Listing) {
        listing.clear();
        self.listing = listing;
    }

    /// Visits the entries `lo..hi` of `dir` in blocks, handing the second half of what is left
    /// to an idle worker.
    fn walk_chunk(&mut self, dir: &Arc<Dir>, lo: usize, mut hi: usize) -> Tally {
        let tuning = &self.walker.tuning;
        let block = tuning.block;

        let mut tally = Tally::default();
        let mut at = lo;

        while at < hi {
            let end = (at + block).min(hi);

            let rest = hi - end;
            let idle = self.shared.idle.load(Ordering::Relaxed) > 0;

            // Another worker takes the second half of what is left, or starts to.
            if rest >= 2 * block && (idle || self.start_helper()) {
                let mid = end + rest / 2;
                tuning.splits.fetch_add(1, Ordering::Relaxed);
                self.give(Task::Chunk(Chunk {
                    dir: Arc::clone(dir),
                    lo: mid,
                    hi,
                }));
                hi = mid;
            }

            // Subdirectories found so far can start elsewhere meanwhile.
            self.share();

            let n = end.min(hi) - at;
            tally += self.walk_entries(dir, at, at + n);
            at += n;
        }

        tally
    }

    /// Queues `task` for an idle worker.
    fn give(&self, task: Task) {
        self.shared.pending.fetch_add(1, Ordering::AcqRel);
        unpoisoned(self.shared.queue.lock()).push(task);
        self.shared.wake.notify_one();
    }

    /// Opens and lists the directory of `job`, storing its record if that is still to do, and
    /// counts what it recorded and listed without walking in `tally`. Returns its record,
    /// listing, which ignore files it has and its metadata, if it was read; `None` when it
    /// cannot be read or is not walked.
    ///
    /// A directory the parent listed as one but that cannot be looked up (its parent is
    /// readable but not searchable, or it vanished) has no record. It counts as ignored, as an
    /// entry whose metadata cannot be read does.
    fn open(
        &mut self,
        job: &Job,
        tally: &mut Tally,
    ) -> Option<(NodeRef, Listing, Found, Option<Stat>)> {
        let walker = self.walker;
        let opened = self.lister.open(&job.path, &job.access);

        let (id, foreign, stat) = match job.me {
            // For loop detection (`--follow`).
            Me::Pushed { id, foreign } => {
                let stat = if walker.follow_links {
                    opened.as_ref().and_then(|dir| dir.stat(Fields::default()))
                } else {
                    None
                };

                (id, foreign, stat)
            },
            #[cfg(fd_walk)]
            Me::Deferred(parent) => {
                // The parent counted the directory as deferred; it is recorded or ignored here.
                tally.listed += 1;

                let stat = opened
                    .as_ref()
                    .and_then(|dir| dir.stat(walker.fields))
                    .or_else(|| listing::lstat_dir(&job.path, &job.access, walker.fields))?;
                let index = self.store_dir(job, parent, &stat)?;
                tally.recorded += 1;

                let id = NodeRef {
                    sink: self.sink_index,
                    index,
                };
                (id, self.is_foreign(&stat, &job.path), Some(stat))
            },
        };

        let opened = opened?;

        if foreign && !walker.count_ignored {
            return None;
        }

        let mut listing = mem::take(&mut self.listing);
        let mut found = Found::default();
        listing.clear();
        self.lister
            .fill(&mut listing, opened, stat.as_ref(), foreign, &mut found);

        if foreign {
            // Contents of another file system are not walked, only counted.
            tally.listed += listing.len() + listing.unusable;
            self.recycle(listing);
            return None;
        }

        tally.listed += listing.unusable;
        Some((id, listing, found, stat))
    }

    /// Stores the record of the directory of `job`, a directory its parent deferred
    /// ([`Me::Deferred`]).
    #[cfg(fd_walk)]
    fn store_dir(&mut self, job: &Job, parent: NodeRef, stat: &Stat) -> Option<u32> {
        let name = job.path.file_name()?;
        let parent_fd = job.access.parent_fd();

        at::with_cstr(name.as_bytes(), |cname| {
            Ok(self.sink.push(&Record {
                parent: Some(parent),
                depth: job.depth,
                name,
                path: &job.path,
                stat,
                is_symlink: false,
                at: parent_fd.map(|dir| At { dir, name: cname }),
            }))
        })
        .ok()
        .flatten()
    }

    /// Visits the entries `lo..hi` of `dir`: records the kept ones and queues the subdirectories.
    fn walk_entries(&mut self, dir: &Dir, lo: usize, hi: usize) -> Tally {
        let walker = self.walker;
        let depth = dir.depth + 1;
        let may_descend = walker.max_depth.is_none_or(|max| depth < max);

        let mut buf = mem::take(&mut self.path_buf);
        buf.clear();
        buf.extend_from_slice(dir.path.as_os_str().as_encoded_bytes());
        if buf.last() != Some(&(MAIN_SEPARATOR as u8)) {
            buf.push(MAIN_SEPARATOR as u8);
        }
        let dir_len = buf.len();

        let mut tally = Tally {
            listed: hi - lo,
            ..Tally::default()
        };
        // How many of the entries were recorded tells how to list the subdirectories queued here.
        let mut sample = Sample::new();
        let queued = self.local.len();

        for i in lo..hi {
            buf.truncate(dir_len);
            buf.extend_from_slice(dir.listing.name(i));

            // SAFETY: `buf` is the encoded directory path, an ASCII separator and an encoded
            // name, so it is valid as an `OsStr`.
            let path = Path::new(unsafe { OsStr::from_encoded_bytes_unchecked(&buf) });

            let kept = self.visit(dir, i, path, may_descend);

            // Every kept entry is recorded but a directory whose own job stores it.
            let recorded = match kept {
                None => false,
                Some(
                    Kept::Leaf
                    | Kept::Dir {
                        me: Me::Pushed { .. },
                        ..
                    },
                ) => true,
                #[cfg(fd_walk)]
                Some(Kept::Dir {
                    me: Me::Deferred(_),
                    ..
                }) => false,
            };
            sample.add(dir.listing.kind(i), recorded);

            let Some(Kept::Dir { me, follow }) = kept else {
                tally.recorded += usize::from(recorded);
                continue;
            };

            if recorded {
                tally.recorded += 1;
            } else {
                tally.deferred += 1;
            }

            let access = dir.listing.access(path, follow);
            self.shared.pending.fetch_add(1, Ordering::AcqRel);
            self.local.push(Task::Dir(Job {
                // SAFETY: as above.
                path: PathBuf::from(unsafe { OsString::from_encoded_bytes_unchecked(buf.clone()) }),
                me,
                depth,
                rules: dir.rules.clone(),
                access,
            }));
        }

        sample.apply(
            self.local[queued..]
                .iter_mut()
                .filter_map(|task| match task {
                    Task::Dir(job) => Some(&mut job.access),
                    Task::Chunk(_) => None,
                }),
        );

        self.path_buf = buf;
        tally
    }

    /// Applies the walk rules to entry `i` of `dir` and records it. Returns what is needed to
    /// descend into it when kept.
    fn visit(&mut self, dir: &Dir, i: usize, path: &Path, may_descend: bool) -> Option<Kept> {
        let walker = self.walker;
        let listing = &dir.listing;
        let fields = walker.fields;
        let depth = dir.depth + 1;

        // The type the directory reported; where it did not, one metadata call settles it.
        let mut stat = None;
        let reported = match listing.kind(i) {
            Some(kind) => kind,
            None => stat.insert(listing.lstat(i, fields)?).kind,
        };

        let is_symlink = reported.is_symlink();
        let mut file_type = reported;

        let followed = if walker.follow_links && is_symlink {
            let target = listing.stat(i, path, fields)?;
            file_type = target.kind;

            if file_type.is_dir() && dir.rules.is_loop(path, &target) {
                return None;
            }

            // The path-based walk opened a followed directory to compare it with the walked
            // ones, and dropped it when that failed.
            #[cfg(target_os = "macos")]
            if file_type.is_dir() && listing.at(i).is_none_or(|at| at::open_file(at).is_err()) {
                return None;
            }

            Some(target)
        } else {
            None
        };

        let is_dir = file_type.is_dir();

        if dir.rules.is_ignored(path, is_dir, || listing.is_hidden(i)) {
            return None;
        }

        if let Some(filter) = &walker.filter {
            let candidate = filter::Entry {
                path,
                kind: file_type,
                depth,
                hidden: listing.is_hidden(i),
            };

            if !filter.keep(&candidate) {
                return None;
            }
        }

        // A directory that will be read is stored by the job that reads it. On macOS, one whose
        // path does not fit in `PATH_MAX` cannot be read, as erdtree once opened it by path.
        #[cfg(target_os = "linux")]
        let readable = true;
        #[cfg(target_os = "macos")]
        let readable = crate::fs::fits_path_max(path.as_os_str().len());
        #[cfg(fd_walk)]
        if reported.is_dir() && may_descend && stat.is_none() && followed.is_none() && readable {
            return Some(Kept::Dir {
                me: Me::Deferred(dir.id),
                follow: false,
            });
        }

        let stat = match followed.or(stat) {
            Some(stat) => stat,
            None => listing.lstat(i, fields)?,
        };

        let index = self.sink.push(&Record {
            parent: Some(dir.id),
            depth,
            name: listing.os_name(i),
            path,
            stat: &stat,
            is_symlink,
            #[cfg(target_os = "linux")]
            at: listing.at(i),
            // Beyond `PATH_MAX` the path-based calls fail, as they once did.
            #[cfg(target_os = "macos")]
            at: listing
                .at(i)
                .filter(|_| crate::fs::fits_path_max(path.as_os_str().len())),
        })?;

        if !(stat.kind.is_dir() && may_descend) {
            return Some(Kept::Leaf);
        }

        let foreign = self.is_foreign(&stat, path);

        Some(Kept::Dir {
            me: Me::Pushed {
                id: NodeRef {
                    sink: self.sink_index,
                    index,
                },
                foreign,
            },
            follow: is_symlink && walker.follow_links,
        })
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::fs::stat::Kind;
    use ignore::overrides::Override;
    use std::collections::BTreeSet;

    /// Keeps what the walk reports, so runs can be compared.
    #[derive(Default)]
    struct Collect {
        records: Vec<(PathBuf, Kind, u64, Option<NodeRef>)>,
        ignored: usize,
    }

    impl Sink for Collect {
        fn push(&mut self, record: &Record<'_>) -> Option<u32> {
            self.records.push((
                record.path.to_path_buf(),
                record.stat.kind,
                record.stat.size,
                record.parent,
            ));
            u32::try_from(self.records.len() - 1).ok()
        }

        fn add_ignored(&mut self, count: usize) {
            self.ignored += count;
        }
    }

    /// A directory of `wide` files (some hidden) and subdirectories, each with `inner` files.
    fn tree(wide: usize, subdirs: usize, inner: usize) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        for i in 0..wide {
            let name = if i % 7 == 0 {
                format!(".hidden-{i}")
            } else {
                format!("file-{i}")
            };
            fs::write(root.join(name), "x".repeat(i % 13)).unwrap();
        }
        fs::write(root.join(".gitignore"), "file-1*\n").unwrap();

        for d in 0..subdirs {
            let sub = root.join(format!("dir-{d}"));
            fs::create_dir(&sub).unwrap();
            for i in 0..inner {
                fs::write(sub.join(format!("f{i}")), "y".repeat(i)).unwrap();
            }
        }

        #[cfg(unix)]
        std::os::unix::fs::symlink("file-2", root.join("link")).unwrap();

        dir
    }

    /// A walk of `root` with hidden and ignored entries skipped and counted, and a fixed
    /// number of workers.
    fn walker(root: &Path, threads: usize, tuning: Tuning) -> Walker {
        Walker {
            root: root.to_path_buf(),
            rules: Rules::root(rules::Options::new(true, true), Override::empty(), None),
            follow_links: false,
            same_file_system: false,
            max_depth: None,
            threads,
            max_threads: threads,
            filter: None,
            count_ignored: true,
            fields: Fields::default(),
            tuning,
        }
    }

    /// Every record as (path, kind, size, parent's path), the ignored count and how often a
    /// directory was split.
    fn walk(root: &Path, threads: usize, tuning: Tuning) -> (BTreeSet<String>, usize, usize) {
        walk_with(root, threads, threads, tuning).0
    }

    /// Like [`walk`] with up to `max_threads` workers; also returns how many were added.
    fn walk_with(
        root: &Path,
        threads: usize,
        max_threads: usize,
        tuning: Tuning,
    ) -> ((BTreeSet<String>, usize, usize), usize) {
        let walker = Walker {
            max_threads,
            ..walker(root, threads, tuning)
        };

        let progress = AtomicU64::new(0);
        let sinks = walker.run(Collect::default, Some(&progress));
        let grown = walker.tuning.grown.load(Ordering::Relaxed);
        assert_eq!(sinks.len(), threads + grown, "a sink per worker");
        assert_eq!(
            sinks[0].records[0].3, None,
            "the root is the first record of the first sink"
        );

        let parent_path = |parent: Option<NodeRef>| {
            parent.map(|p| sinks[p.sink as usize].records[p.index as usize].0.clone())
        };

        let records: Vec<String> = sinks
            .iter()
            .flat_map(|sink| &sink.records)
            .map(|(path, kind, size, parent)| {
                format!(
                    "{} {kind:?} {size} {:?}",
                    path.display(),
                    parent_path(*parent)
                )
            })
            .collect();
        let total = records.len();
        let records: BTreeSet<String> = records.into_iter().collect();
        let ignored = sinks.iter().map(|sink| sink.ignored).sum();

        // Every kept entry below the root is counted by the progress counter, once.
        assert_eq!(records.len(), total, "no record appears twice");
        assert_eq!(
            usize::try_from(progress.load(Ordering::Relaxed)).unwrap(),
            total - 1
        );

        (
            (
                records,
                ignored,
                walker.tuning.splits.load(Ordering::Relaxed),
            ),
            grown,
        )
    }

    #[test]
    fn splitting_large_directories_changes_nothing() {
        let dir = tree(3000, 40, 30);
        let root = dir.path().canonicalize().unwrap();

        let (expected, expected_ignored, splits) = walk(&root, 1, Tuning::splitting(32, 16));
        assert_eq!(splits, 0, "a single worker never splits");
        assert!(expected.len() > 3500);
        assert!(
            expected_ignored > 400,
            "hidden and ignored entries are counted"
        );

        let mut split_somewhere = false;

        for _ in 0..20 {
            let (records, ignored, splits) = walk(&root, 4, Tuning::splitting(32, 16));
            assert_eq!(records, expected);
            assert_eq!(ignored, expected_ignored);
            split_somewhere |= splits > 0;
        }

        assert!(
            split_somewhere,
            "idle workers took over parts of a directory"
        );

        // With a threshold above every directory size, nothing is split.
        let (records, ignored, splits) = walk(&root, 4, Tuning::splitting(usize::MAX, 16));
        assert_eq!((records, ignored, splits), (expected, expected_ignored, 0));
    }

    /// A directory that is listed but cannot be looked up in (mode 644) holds entries the walk
    /// cannot stat. All of them count as ignored, subdirectories too, and none as kept.
    #[cfg(unix)]
    #[test]
    fn unreadable_entries_count_as_ignored() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let locked = root.join("locked");

        fs::create_dir(&locked).unwrap();
        fs::write(locked.join("f"), "x").unwrap();
        fs::create_dir(locked.join("sub")).unwrap();
        fs::create_dir(locked.join("sub2")).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).unwrap();
        // The superuser can look up everything, so the walk has nothing to ignore.
        if fs::symlink_metadata(locked.join("f")).is_ok() {
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
            eprintln!("skipped unreadable_entries_count_as_ignored: running as the superuser");
            return;
        }

        let (records, ignored, _) = walk(&root, 1, Tuning::default());
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(records.len(), 2, "{records:?}");
        assert_eq!(ignored, 3);
    }

    /// How many workers a walk with `threads` workers starts.
    fn started(root: &Path, threads: usize) -> usize {
        let walker = Walker {
            count_ignored: false,
            ..walker(root, threads, Tuning::default())
        };

        let sinks = walker.run(Collect::default, None);
        assert_eq!(sinks.len(), threads, "a sink per worker");
        walker.tuning.started.load(Ordering::Relaxed)
    }

    #[test]
    fn workers_start_when_there_is_work_for_them() {
        // Nothing to share: the first worker walks a directory of files alone.
        let flat = tree(3, 0, 0);
        assert_eq!(started(&flat.path().canonicalize().unwrap(), 4), 0);

        // Subdirectories make work for others, but never more workers than `threads`.
        let nested = tree(3, 30, 3);
        let started = started(&nested.path().canonicalize().unwrap(), 4);
        assert!((1..=3).contains(&started), "started {started}");
    }

    #[test]
    fn small_directories_split_when_files_are_read() {
        let dir = tree(40, 3, 5);
        let root = dir.path().canonicalize().unwrap();
        let (expected, expected_ignored, splits) = walk(&root, 4, Tuning::default());
        assert_eq!(splits, 0, "no directory reaches the default threshold");

        let mut split_somewhere = false;
        for _ in 0..50 {
            let (records, ignored, splits) = walk(&root, 4, Tuning::reading_files());
            assert_eq!((records, ignored), (expected.clone(), expected_ignored));
            split_somewhere |= splits > 0;
        }
        assert!(split_somewhere, "a directory of 40 entries was split");
    }

    #[test]
    fn adding_workers_changes_nothing() {
        let dir = tree(300, 60, 20);
        let root = dir.path().canonicalize().unwrap();
        let (expected, _) = walk_with(&root, 1, 1, Tuning::default());

        // Every task counts as blocked, so the walk grows to the limit.
        let always = || Tuning {
            policy: adaptive::Policy {
                window: std::time::Duration::ZERO,
                min_switches: 0,
                min_blocked: 0.0,
            },
            ..Tuning::splitting(32, 16)
        };

        // `walk_with` checks that there is a sink per worker and that parents resolve to the
        // sinks that recorded them; at the limit that is `max_threads` sinks.
        let mut reached = false;
        for _ in 0..50 {
            let (got, added) = walk_with(&root, 2, 16, always());
            assert_eq!((got.0, got.1), (expected.0.clone(), expected.1));
            assert!(added <= 14, "added {added} workers");
            if added == 14 {
                reached = true;
                break;
            }
        }
        assert!(reached, "the walk grew to 16 workers");

        // A fixed number of workers never grows, whatever the policy says.
        let (got, added) = walk_with(&root, 2, 2, always());
        assert_eq!((got.0, added), (expected.0, 0));
    }

    #[test]
    fn workers_grow_only_where_waits_are_measured() {
        assert_eq!(max_threads(4, false), 4);
        assert_eq!(
            max_threads(4, true),
            if adaptive::MEASURES { 32 } else { 4 }
        );
        assert_eq!(
            max_threads(16, true),
            if adaptive::MEASURES { 64 } else { 16 }
        );
        assert_eq!(
            max_threads(100, true),
            100,
            "never fewer than the threads given"
        );
    }
}

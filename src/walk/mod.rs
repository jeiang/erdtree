//! Parallel filesystem walk. Each worker reads whole directories and appends one record per kept
//! entry to its own [`Sink`], so no per-entry synchronization occurs. Directories are distributed
//! with per-worker LIFO stacks and a shared queue that is only used when another worker is idle.
//!
//! Entry selection matches [`ignore::WalkParallel`] as `erd` configured it before: the same ignore
//! rules ([`rules`]), hidden-file handling, `--exclude`/`--no-git` overrides, entry filter,
//! depth limit, same-file-system check and symlink loop detection. Entries whose metadata cannot
//! be read are dropped, and so are their contents.
//!
//! On Linux a directory is opened relative to its parent's file descriptor, read with
//! `getdents64` into a [`Listing`] and each entry is stat'ed with `statx` relative to the open
//! directory (see [`crate::fs::at`]). Elsewhere the walk uses `std::fs::read_dir`.

use crate::fs::stat::{Fields, Kind, Stat};
use std::{
    ffi::{OsStr, OsString},
    fs, mem,
    path::{MAIN_SEPARATOR, Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    thread,
    time::Instant,
};

#[cfg(target_os = "linux")]
use crate::fs::at::{self, At};
#[cfg(target_os = "linux")]
use std::os::{
    fd::{AsRawFd, OwnedFd},
    unix::ffi::OsStrExt,
};

/// Detects workers that wait for the file system.
mod adaptive;
/// Bounds the descriptors that queued jobs hold.
#[cfg(target_os = "linux")]
mod fds;
/// The entries of one directory.
mod listing;
/// Ignore rules per directory, adapted from the `ignore` crate.
pub mod rules;

#[cfg(target_os = "linux")]
use fds::{Budget, DirFd};
use listing::Listing;
use rules::{Found, Rules};

/// Identifies a record: the index of the sink that holds it and its index in that sink.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeRef {
    pub sink: u32,
    pub index: u32,
}

/// A directory entry offered to the entry filter (`--pattern`, `--include`).
pub struct Entry<'a> {
    path: &'a Path,
    file_type: Kind,
    depth: usize,
    hidden: bool,
}

impl Entry<'_> {
    /// Full path of the entry, below the canonical root.
    pub const fn path(&self) -> &Path {
        self.path
    }

    /// Type of the entry; of the link target when symlinks are followed.
    pub const fn file_type(&self) -> Kind {
        self.file_type
    }

    /// Name of the entry.
    pub fn file_name(&self) -> &OsStr {
        self.path.file_name().unwrap_or(self.path.as_os_str())
    }

    /// Depth below the root, which is depth 0.
    pub const fn depth(&self) -> usize {
        self.depth
    }

    /// Whether the hidden-file filter applies: the name starts with `.` or, on Windows, the
    /// entry has the hidden attribute.
    pub const fn is_hidden(&self) -> bool {
        self.hidden
    }
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
    #[cfg(target_os = "linux")]
    pub at: Option<At<'a>>,
}

impl Record<'_> {
    /// Whether the entry has extended attributes (following a symlink).
    #[cfg(unix)]
    pub fn has_xattrs(&self) -> bool {
        #[cfg(target_os = "linux")]
        if let Some(has) = self.at.and_then(at::has_xattrs) {
            return has;
        }

        crate::fs::xattr::has_xattrs(self.path)
    }

    /// Whether the target of the entry, which must be a symlink, exists.
    pub fn target_exists(&self) -> bool {
        #[cfg(target_os = "linux")]
        if let Some(at) = self.at {
            return at::target_exists(at);
        }

        self.path.exists()
    }

    /// Opens the entry for reading, following a symlink.
    pub fn open(&self) -> std::io::Result<fs::File> {
        #[cfg(target_os = "linux")]
        if let Some(at) = self.at {
            return at::open_file(at);
        }

        fs::File::open(self.path)
    }

    /// The target of the entry, which must be a symlink.
    pub fn read_link(&self) -> std::io::Result<PathBuf> {
        #[cfg(target_os = "linux")]
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

/// Predicate deciding whether an entry is kept (and, for a directory, descended into).
pub type Filter = Box<dyn Fn(&Entry<'_>) -> bool + Send + Sync>;

/// Walk configuration.
pub struct Walker {
    pub root: PathBuf,
    pub rules: Rules,
    pub follow_links: bool,
    pub same_file_system: bool,
    pub max_depth: Option<usize>,
    pub threads: usize,
    /// Most workers the walk may have. It starts with `threads` and adds workers while they wait
    /// for the file system (a cold cache, a network file system). `threads` or less keeps the
    /// number fixed.
    pub max_threads: usize,
    pub filter: Option<Filter>,
    /// Count ignored entries per directory (`--report-ignored`).
    pub count_ignored: bool,
    /// Optional metadata fields to read for every entry.
    pub fields: Fields,
    /// How large directories are shared between workers.
    pub tuning: Tuning,
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

/// Knobs of the walk that tests change.
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
    /// Already stored, at this reference.
    Pushed(NodeRef),
    /// Not stored yet: whoever reads the directory stores it, below this parent. Reading the
    /// directory first gives its metadata for free (`statx` of the open directory), instead of
    /// a second lookup of its name.
    #[cfg(target_os = "linux")]
    Deferred(NodeRef),
}

/// Work for a worker.
enum Task {
    /// Read a directory.
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
    /// Whether the directory is on another file system than the root (`-x`). Only known for
    /// [`Me::Pushed`].
    foreign: bool,
    /// The parent directory, which the directory is opened relative to. `None` when the budget
    /// of held descriptors is spent: the directory is then opened by path.
    #[cfg(target_os = "linux")]
    parent: Option<Arc<DirFd>>,
    /// Whether the directory is a followed symlink, so opening it follows the link.
    #[cfg(target_os = "linux")]
    follow: bool,
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
    /// The sinks of workers that have not started, by sink index.
    pool: Vec<Mutex<Option<S>>>,
    /// Sink index of the next of the first `threads` workers to start. Worker 0 starts with the
    /// walk; the others start when work is available for them.
    next: AtomicUsize,
    /// Sinks of the workers other than worker 0, once they are done.
    finished: Mutex<Vec<(u32, S)>>,
    /// How many directory descriptors queued jobs may hold.
    #[cfg(target_os = "linux")]
    budget: Arc<Budget>,
}

/// Runs the worker with sink index `sink_index` until the walk is done, and returns its sink.
fn run_worker<'scope, 'env, S, F>(
    scope: &'scope thread::Scope<'scope, 'env>,
    run: &'env Run<'env, S, F>,
    sink_index: usize,
    sink: S,
    local: Vec<Task>,
) -> S
where
    S: Sink + 'env,
    F: Fn(u32) -> S + Sync + 'env,
{
    let guard = QuitOnPanic(&run.shared);
    let mut worker = Worker {
        walker: run.walker,
        sink_index: u32::try_from(sink_index).unwrap_or(u32::MAX),
        shared: &run.shared,
        run,
        scope,
        meter: adaptive::Meter::new(run.walker.tuning.policy),
        sink,
        local,
        progress: run.progress,
        root_device: None,
        listing: Listing::default(),
        #[cfg(target_os = "linux")]
        dirent_buf: Vec::new(),
        path_buf: Vec::new(),
        deferred: 0,
    };
    worker.run();
    mem::forget(guard);
    worker.sink
}

/// Starts a thread for the worker with sink index `sink_index`, whose sink is in `run.pool`. A
/// refused thread leaves the sink there.
fn spawn_worker<'scope, 'env, S, F>(
    scope: &'scope thread::Scope<'scope, 'env>,
    run: &'env Run<'env, S, F>,
    sink_index: usize,
) -> std::io::Result<()>
where
    S: Sink + 'env,
    F: Fn(u32) -> S + Sync + 'env,
{
    thread::Builder::new().spawn_scoped(scope, move || {
        let sink = run.pool[sink_index]
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();

        if let Some(sink) = sink {
            let sink = run_worker(scope, run, sink_index, sink, vec![]);
            run.finished
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((u32::try_from(sink_index).unwrap_or(u32::MAX), sink));
        }
    })?;

    Ok(())
}

impl Walker {
    /// Most workers a walk may have.
    fn max_workers(&self) -> usize {
        self.threads.max(1).max(self.max_threads)
    }

    /// Walks the tree. `make_sink(i)` creates the sink with index `i`; the root record goes to
    /// the sink with index `threads`. Returns every sink, in index order: the workers' first,
    /// then the root's, then those of extra workers (see [`Walker::max_threads`]). `progress`
    /// counts kept entries.
    pub fn run<S: Sink, F: Fn(u32) -> S + Sync>(
        &self,
        make_sink: F,
        progress: Option<&AtomicU64>,
    ) -> Vec<S> {
        let threads = self.threads.max(1);
        let mut sinks: Vec<S> = (0..=threads)
            .map(|i| make_sink(u32::try_from(i).unwrap_or(u32::MAX)))
            .collect();

        let root_sink_index = u32::try_from(threads).unwrap_or(u32::MAX);
        let Some(root) = self.root_job(&mut sinks[threads], root_sink_index) else {
            return sinks;
        };

        // Parent directories stay open while their subdirectories wait, so allow many files, and
        // hold no more of them than the limit allows.
        #[cfg(target_os = "linux")]
        let budget = Budget::new(at::raise_open_file_limit(), self.max_workers());

        let root_sink = sinks.pop();
        let mut sinks = sinks.into_iter();
        let first = sinks.next();

        // Sink `i` of the workers that start later waits in the pool, which has room for the
        // extra workers too.
        let mut pool: Vec<Mutex<Option<S>>> =
            (0..=self.max_workers()).map(|_| Mutex::new(None)).collect();
        for (i, sink) in sinks.enumerate() {
            *pool[i + 1]
                .get_mut()
                .unwrap_or_else(PoisonError::into_inner) = Some(sink);
        }

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
            finished: Mutex::new(Vec::new()),
            #[cfg(target_os = "linux")]
            budget,
        };

        // Each worker owns its sink, so no two threads write next to each other in memory.
        // Worker 0 runs on this thread; a tree with no work to share never starts another.
        let first = thread::scope(|s| {
            first.map(|sink| run_worker(s, &run, 0, sink, vec![Task::Dir(root)]))
        });

        let mut all: Vec<(u32, S)> = run
            .finished
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner);
        all.extend(first.map(|sink| (0, sink)));
        all.extend(root_sink.map(|sink| (root_sink_index, sink)));

        // Workers that never started still have their sinks, which keep the sink numbering whole.
        for (i, slot) in run.pool.into_iter().enumerate() {
            let sink = slot.into_inner().unwrap_or_else(PoisonError::into_inner);
            all.extend(sink.map(|sink| (u32::try_from(i).unwrap_or(u32::MAX), sink)));
        }

        all.sort_unstable_by_key(|(index, _)| *index);
        all.into_iter().map(|(_, sink)| sink).collect()
    }

    /// Records the root and returns the job that reads it.
    fn root_job<S: Sink>(&self, sink: &mut S, sink_index: u32) -> Option<Job> {
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
            #[cfg(target_os = "linux")]
            at: None,
        })?;

        if !stat.kind.is_dir() || self.max_depth == Some(0) {
            return None;
        }

        Some(Job {
            path: self.root.clone(),
            me: Me::Pushed(NodeRef {
                sink: sink_index,
                index,
            }),
            depth: 0,
            rules: self.rules.add_parents(&self.root),
            foreign: false,
            #[cfg(target_os = "linux")]
            parent: None,
            #[cfg(target_os = "linux")]
            follow: true,
        })
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
struct Kept {
    /// Its index in the worker's sink; `None` for a directory whose record the job that reads it
    /// stores.
    index: Option<u32>,
    kind: Kind,
    /// Whether it is a directory on another file system than the root (`-x`).
    foreign: bool,
    /// Whether it is a symlink that was followed.
    #[cfg(target_os = "linux")]
    followed: bool,
}

struct Worker<'scope, 'env, S, F> {
    walker: &'env Walker,
    sink_index: u32,
    shared: &'env Shared,
    run: &'env Run<'env, S, F>,
    scope: &'scope thread::Scope<'scope, 'env>,
    meter: adaptive::Meter,
    sink: S,
    local: Vec<Task>,
    progress: Option<&'env AtomicU64>,
    /// Device of the root, with `--one-file-system`.
    root_device: Option<u64>,
    /// Buffers reused from directory to directory.
    listing: Listing,
    #[cfg(target_os = "linux")]
    dirent_buf: Vec<u8>,
    path_buf: Vec<u8>,
    /// Directories the current task queued whose record their own job stores. Those jobs count
    /// them as kept (for the progress counter) when they do.
    deferred: u64,
}

impl<'env, S, F> Worker<'_, 'env, S, F>
where
    S: Sink + 'env,
    F: Fn(u32) -> S + Sync + 'env,
{
    fn run(&mut self) {
        self.root_device = if self.walker.same_file_system {
            device_num(&self.walker.root)
        } else {
            None
        };

        while let Some(task) = self.next_job() {
            let started = Instant::now();
            let kept = match task {
                Task::Dir(job) => self.read_dir(job),
                Task::Chunk(chunk) => self.walk_chunk(&chunk.dir, chunk.lo, chunk.hi),
            };

            if self.meter.task(started.elapsed()) {
                self.grow();
            }

            let deferred = mem::take(&mut self.deferred);
            if let Some(progress) = self.progress {
                progress.fetch_add(kept.saturating_sub(deferred), Ordering::Relaxed);
            }

            self.share();

            if self.shared.pending.fetch_sub(1, Ordering::AcqRel) == 1 {
                // Last job done: wake everyone so they observe completion.
                let _guard = self.shared.queue.lock();
                self.shared.wake.notify_all();
            }
        }
    }

    /// Doubles the number of workers, up to the limit, because the workers wait for the file
    /// system. The new workers start idle, so the busy ones hand them directories.
    fn grow(&self) {
        let run = self.run;
        let cap = self.walker.max_workers();
        let mut have = run.workers.load(Ordering::Relaxed);

        loop {
            if have >= cap {
                return;
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
        for k in have..want {
            // Sink `threads` is the root's, so the extra workers follow it.
            let index = k + 1;
            let sink = (run.make_sink)(u32::try_from(index).unwrap_or(u32::MAX));
            *run.pool[index]
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(sink);
            self.walker.tuning.grown.fetch_add(1, Ordering::Relaxed);

            // A refused thread leaves the walk with the workers it has.
            if spawn_worker(self.scope, run, index).is_err() {
                run.workers.store(cap, Ordering::Relaxed);
                return;
            }
        }
    }

    /// Starts the next worker that has not started yet, if there is one. Returns whether it did.
    fn start_helper(&self) -> bool {
        let run = self.run;
        let threads = self.walker.threads.max(1);

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

        let mut queue = self
            .shared
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
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

        let mut queue = self
            .shared
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner);

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
            self.meter.waited();
            queue = self
                .shared
                .wake
                .wait(queue)
                .unwrap_or_else(PoisonError::into_inner);
            self.shared.idle.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Reads one directory, records its kept entries and queues its subdirectories. Returns the
    /// number of kept entries.
    fn read_dir(&mut self, job: Job) -> u64 {
        let Some((id, listing, found, ident)) = self.open(&job) else {
            return 0;
        };

        #[cfg(target_os = "linux")]
        let fd = listing.dir.as_ref().map(|dir| dir.as_raw_fd());
        #[cfg(not(target_os = "linux"))]
        let fd = None;

        let rules =
            job.rules
                .add_child(&job.path, Some(&found), self.walker.follow_links, ident, fd);
        let dir = Dir {
            listing,
            path: job.path,
            rules,
            id,
            depth: job.depth,
        };

        let len = dir.listing.len();

        if self.walker.count_ignored {
            self.sink.add_ignored(dir.listing.unusable);
        }

        if len >= self.walker.tuning.split_min && self.walker.max_workers() > 1 {
            // Idle workers may take over parts of a directory this large.
            let dir = Arc::new(dir);
            let kept = self.walk_chunk(&dir, 0, len);

            if let Ok(dir) = Arc::try_unwrap(dir) {
                self.recycle(dir.listing);
            }

            return kept;
        }

        let kept = self.walk_entries(&dir, 0, len);

        if self.walker.count_ignored {
            self.sink
                .add_ignored(len - usize::try_from(kept).unwrap_or(len));
        }

        self.recycle(dir.listing);
        kept
    }

    /// Keeps the buffers of `listing` for the next directory, but not the directory's file
    /// descriptor.
    fn recycle(&mut self, mut listing: Listing) {
        listing.clear();
        self.listing = listing;
    }

    /// Visits the entries `lo..hi` of `dir` in blocks, handing the second half of what is left
    /// to an idle worker. Returns how many entries were kept.
    fn walk_chunk(&mut self, dir: &Arc<Dir>, lo: usize, mut hi: usize) -> u64 {
        let tuning = &self.walker.tuning;
        let block = tuning.block;

        let mut kept = 0;
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
            kept += self.walk_entries(dir, at, at + n);
            at += n;
        }

        if self.walker.count_ignored {
            let listed = hi - lo;
            self.sink
                .add_ignored(listed - usize::try_from(kept).unwrap_or(listed));
        }

        kept
    }

    /// Queues `task` for an idle worker.
    fn give(&self, task: Task) {
        self.shared.pending.fetch_add(1, Ordering::AcqRel);
        self.shared
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(task);
        self.shared.wake.notify_one();
    }

    /// Opens and lists the directory of `job`, storing its record if that is still to do.
    /// Returns its record, listing, which ignore files it has and its device and inode numbers
    /// (with `--follow`); `None` when it cannot be read or is not walked.
    ///
    /// A directory the parent listed as one but that cannot be looked up (its parent is
    /// readable but not searchable, or it vanished) has no record. It counts as ignored, as an
    /// entry whose metadata cannot be read does.
    #[cfg(target_os = "linux")]
    fn open(&mut self, job: &Job) -> Option<(NodeRef, Listing, Found, Option<rules::Ident>)> {
        let walker = self.walker;
        let fd = open_dir(job);
        let mut ident = None;

        let (id, foreign) = match job.me {
            Me::Pushed(id) => {
                if walker.follow_links {
                    let stat = fd
                        .as_ref()
                        .and_then(|fd| at::stat_fd(fd.as_raw_fd(), Fields::default()).ok());
                    ident = stat.map(|stat| (stat.dev, stat.ino));
                }

                (id, job.foreign)
            },
            Me::Deferred(parent) => {
                let from_fd = fd
                    .as_ref()
                    .and_then(|fd| at::stat_fd(fd.as_raw_fd(), walker.fields).ok());

                let stored = from_fd
                    .or_else(|| lstat_in_parent(job, walker.fields))
                    .and_then(|stat| {
                        let index = self.store_dir(job, parent, &stat)?;
                        Some((stat, index))
                    });

                let Some((stat, index)) = stored else {
                    if walker.count_ignored {
                        self.sink.add_ignored(1);
                    }
                    return None;
                };

                // The parent's task did not count the directory.
                if let Some(progress) = self.progress {
                    progress.fetch_add(1, Ordering::Relaxed);
                }

                ident = Some((stat.dev, stat.ino));
                let foreign = self.root_device.is_some_and(|root| stat.dev != root);

                (
                    NodeRef {
                        sink: self.sink_index,
                        index,
                    },
                    foreign,
                )
            },
        };

        let fd = fd?;

        if foreign && !walker.count_ignored {
            return None;
        }

        let mut listing = mem::take(&mut self.listing);
        let mut found = Found::default();
        listing.clear();
        let dir = DirFd::new(fd, Arc::clone(&self.run.budget));
        listing.fill(Arc::new(dir), &mut self.dirent_buf, &mut found);

        if foreign {
            // Contents of another file system are not walked, only counted.
            self.sink.add_ignored(listing.len() + listing.unusable);
            listing.clear();
            self.listing = listing;
            return None;
        }

        Some((id, listing, found, ident))
    }

    /// Opens and lists the directory of `job`.
    #[cfg(not(target_os = "linux"))]
    fn open(&mut self, job: &Job) -> Option<(NodeRef, Listing, Found, Option<rules::Ident>)> {
        let Me::Pushed(id) = job.me;

        if job.foreign && !self.walker.count_ignored {
            return None;
        }

        let mut listing = mem::take(&mut self.listing);
        let mut found = Found::default();
        listing.clear();
        let readable = listing.fill(&job.path, &mut found);

        if job.foreign {
            // Contents of another file system are not walked, only counted.
            if readable {
                self.sink.add_ignored(listing.len() + listing.unusable);
            }
            listing.clear();
            self.listing = listing;
            return None;
        }

        if !readable {
            self.listing = listing;
            return None;
        }

        Some((id, listing, found, None))
    }

    /// Stores the record of the directory of `job`, whose records are stored by its own job.
    #[cfg(target_os = "linux")]
    fn store_dir(&mut self, job: &Job, parent: NodeRef, stat: &Stat) -> Option<u32> {
        let name = job.path.file_name()?;
        let parent_fd = job.parent.as_deref().map(DirFd::as_raw_fd);

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
    /// Returns how many entries were kept.
    fn walk_entries(&mut self, dir: &Dir, lo: usize, hi: usize) -> u64 {
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

        let mut kept = 0;

        for i in lo..hi {
            buf.truncate(dir_len);
            buf.extend_from_slice(dir.listing.name(i));

            // SAFETY: `buf` is the encoded directory path, an ASCII separator and an encoded
            // name, so it is valid as an `OsStr`.
            let path = Path::new(unsafe { OsStr::from_encoded_bytes_unchecked(&buf) });

            let Some(entry) = self.visit(dir, i, path, may_descend) else {
                continue;
            };
            kept += 1;

            if !(entry.kind.is_dir() && may_descend) {
                continue;
            }

            let me = match entry.index {
                Some(index) => Me::Pushed(NodeRef {
                    sink: self.sink_index,
                    index,
                }),
                #[cfg(target_os = "linux")]
                None => {
                    self.deferred += 1;
                    Me::Deferred(dir.id)
                },
                #[cfg(not(target_os = "linux"))]
                None => continue,
            };

            #[cfg(target_os = "linux")]
            let parent = dir.listing.dir.as_ref().and_then(|parent| {
                // A path this long cannot be opened by path, so the job needs the descriptor.
                parent
                    .retain(buf.len() >= fds::LONG_PATH)
                    .then(|| Arc::clone(parent))
            });

            self.shared.pending.fetch_add(1, Ordering::AcqRel);
            self.local.push(Task::Dir(Job {
                // SAFETY: as above.
                path: PathBuf::from(unsafe { OsString::from_encoded_bytes_unchecked(buf.clone()) }),
                me,
                depth,
                rules: dir.rules.clone(),
                foreign: entry.foreign,
                #[cfg(target_os = "linux")]
                parent,
                #[cfg(target_os = "linux")]
                follow: entry.followed,
            }));
        }

        self.path_buf = buf;
        kept
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
            None => stat.insert(listing.lstat(i, path, fields)?).kind,
        };

        let is_symlink = reported.is_symlink();
        let mut file_type = reported;

        let followed = if walker.follow_links && is_symlink {
            let target = listing.stat(i, path, fields)?;
            file_type = target.kind;

            if file_type.is_dir() && is_loop(&dir.rules, path, &target) {
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
            let candidate = Entry {
                path,
                file_type,
                depth,
                hidden: listing.is_hidden(i),
            };

            if !filter(&candidate) {
                return None;
            }
        }

        // A directory that will be read is stored by the job that reads it.
        #[cfg(target_os = "linux")]
        if reported.is_dir() && may_descend && stat.is_none() && followed.is_none() {
            return Some(Kept {
                index: None,
                kind: Kind::Dir,
                foreign: false,
                followed: false,
            });
        }

        let stat = match followed.or(stat) {
            Some(stat) => stat,
            None => listing.lstat(i, path, fields)?,
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
        })?;

        let foreign = may_descend
            && stat.kind.is_dir()
            && self
                .root_device
                .is_some_and(|root| device_of(&stat, path).is_some_and(|dev| dev != root));

        Some(Kept {
            index: Some(index),
            kind: stat.kind,
            foreign,
            #[cfg(target_os = "linux")]
            followed: is_symlink && walker.follow_links,
        })
    }
}

/// Metadata of the directory of `job` looked up by name in its parent, or by path when the job
/// holds no descriptor of the parent.
#[cfg(target_os = "linux")]
fn lstat_in_parent(job: &Job, fields: Fields) -> Option<Stat> {
    let Some(parent) = &job.parent else {
        return at::lstat_path(&job.path, fields).ok();
    };

    let dir = parent.as_raw_fd();
    let name = job.path.file_name()?.as_bytes();

    at::with_cstr(name, |name| at::stat_at(At { dir, name }, false, fields)).ok()
}

/// How often opening a directory is retried when the process or system is out of descriptors,
/// and the longest wait between tries.
#[cfg(target_os = "linux")]
const OPEN_RETRIES: u32 = 100;

/// Opens the directory of `job`: relative to its parent when it has one, else by path. Running
/// out of descriptors (`EMFILE`, `ENFILE`) is retried for a while, as other workers close theirs;
/// only then the directory counts as unreadable. The budget (see [`fds`]) keeps the walk from
/// causing this by itself, except for paths too long to open by path.
#[cfg(target_os = "linux")]
fn open_dir(job: &Job) -> Option<OwnedFd> {
    for attempt in 1..=OPEN_RETRIES {
        match open_dir_once(job) {
            Err(e) if matches!(e.raw_os_error(), Some(libc::EMFILE | libc::ENFILE)) => {
                thread::sleep(std::time::Duration::from_micros(100 * u64::from(attempt)));
            },
            result => return result.ok(),
        }
    }

    None
}

#[cfg(target_os = "linux")]
fn open_dir_once(job: &Job) -> std::io::Result<OwnedFd> {
    if let Some(parent) = &job.parent {
        let name = job
            .path
            .file_name()
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))?
            .as_bytes();
        let opened = at::with_cstr(name, |name| {
            at::open_dir_at(parent.as_raw_fd(), name, job.follow)
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

    at::open_dir_path(&job.path, job.follow)
}

/// Whether the directory `target` (a followed symlink's target) is one of the walked directories
/// that lead to it. Compares device and inode numbers, so it also works for paths longer than
/// `PATH_MAX`.
#[cfg(target_os = "linux")]
fn is_loop(rules: &Rules, _path: &Path, target: &Stat) -> bool {
    rules.is_walked((target.dev, target.ino))
}

/// Whether the directory at `path` is one of the walked directories that lead to it.
#[cfg(not(target_os = "linux"))]
fn is_loop(rules: &Rules, path: &Path, _target: &Stat) -> bool {
    let Ok(child) = same_file::Handle::from_path(path) else {
        // Upstream reports the error, which drops the entry.
        return true;
    };

    rules
        .walked_dirs()
        .any(|dir| same_file::Handle::from_path(dir).map_or(true, |ancestor| ancestor == child))
}

/// Device number of the file system `path` is on.
fn device_num(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        fs::metadata(path).ok().map(|md| md.dev())
    }

    #[cfg(windows)]
    {
        crate::fs::inode::volume_serial(path)
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        None
    }
}

/// Device number of a directory given its stat.
// Only the Windows lookup can fail or is not `const`.
#[cfg_attr(unix, allow(clippy::unnecessary_wraps, clippy::missing_const_for_fn))]
fn device_of(stat: &Stat, path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        let _ = path;
        Some(stat.dev)
    }

    #[cfg(not(unix))]
    {
        let _ = stat;
        device_num(path)
    }
}

#[cfg(test)]
mod test {
    use super::*;
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

    /// Every record as (path, kind, size, parent's path), the ignored count and how often a
    /// directory was split.
    fn walk(root: &Path, threads: usize, split_min: usize) -> (BTreeSet<String>, usize, usize) {
        walk_with(
            root,
            threads,
            threads,
            split_min,
            adaptive::Policy::default(),
        )
        .0
    }

    /// Like [`walk`] with up to `max_threads` workers; also returns how many were added.
    fn walk_with(
        root: &Path,
        threads: usize,
        max_threads: usize,
        split_min: usize,
        policy: adaptive::Policy,
    ) -> ((BTreeSet<String>, usize, usize), usize) {
        let walker = Walker {
            root: root.to_path_buf(),
            rules: Rules::root(rules::Options::new(true, true), Override::empty(), None),
            follow_links: false,
            same_file_system: false,
            max_depth: None,
            threads,
            max_threads,
            filter: None,
            count_ignored: true,
            fields: Fields::default(),
            tuning: Tuning {
                policy,
                ..Tuning::splitting(
                    split_min,
                    if split_min < 16 {
                        1
                    } else {
                        (split_min / 8).max(16)
                    },
                )
            },
        };

        let progress = AtomicU64::new(0);
        let sinks = walker.run(|_| Collect::default(), Some(&progress));

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
            walker.tuning.grown.load(Ordering::Relaxed),
        )
    }

    #[test]
    fn splitting_large_directories_changes_nothing() {
        let dir = tree(3000, 40, 30);
        let root = dir.path().canonicalize().unwrap();

        let (expected, expected_ignored, splits) = walk(&root, 1, 32);
        assert_eq!(splits, 0, "a single worker never splits");
        assert!(expected.len() > 3500);
        assert!(
            expected_ignored > 400,
            "hidden and ignored entries are counted"
        );

        let mut split_somewhere = false;

        for _ in 0..20 {
            let (records, ignored, splits) = walk(&root, 4, 32);
            assert_eq!(records, expected);
            assert_eq!(ignored, expected_ignored);
            split_somewhere |= splits > 0;
        }

        assert!(
            split_somewhere,
            "idle workers took over parts of a directory"
        );

        // With a threshold above every directory size, nothing is split.
        let (records, ignored, splits) = walk(&root, 4, usize::MAX);
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

        // The superuser can look up everything.
        let searchable = fs::symlink_metadata(locked.join("f")).is_ok();

        let (records, ignored, _) = walk(&root, 1, SPLIT_MIN);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

        if searchable {
            return;
        }

        assert_eq!(records.len(), 2, "{records:?}");
        assert_eq!(ignored, 3);
    }

    /// How many workers a walk with `threads` workers starts.
    fn started(root: &Path, threads: usize) -> usize {
        let walker = Walker {
            root: root.to_path_buf(),
            rules: Rules::root(rules::Options::new(true, true), Override::empty(), None),
            follow_links: false,
            same_file_system: false,
            max_depth: None,
            threads,
            max_threads: threads,
            filter: None,
            count_ignored: false,
            fields: Fields::default(),
            tuning: Tuning::default(),
        };

        let sinks = walker.run(|_| Collect::default(), None);
        assert_eq!(sinks.len(), threads + 1, "a sink per worker and the root's");
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
        let tuning = Tuning::reading_files();
        assert_eq!((tuning.split_min, tuning.block), (2, 1));

        let dir = tree(40, 3, 5);
        let root = dir.path().canonicalize().unwrap();
        let (expected, expected_ignored, _) = walk(&root, 1, SPLIT_MIN);

        let mut split_somewhere = false;
        for _ in 0..50 {
            let (records, ignored, splits) = walk(&root, 4, tuning.split_min);
            assert_eq!((records, ignored), (expected.clone(), expected_ignored));
            split_somewhere |= splits > 0;
        }
        assert!(split_somewhere, "a directory of 40 entries was split");
    }

    #[test]
    fn adding_workers_changes_nothing() {
        let dir = tree(300, 60, 20);
        let root = dir.path().canonicalize().unwrap();
        let (expected, _) = walk_with(&root, 1, 1, SPLIT_MIN, adaptive::Policy::default());

        // Every task counts as blocked, so the walk grows to the limit.
        let always = adaptive::Policy {
            window: std::time::Duration::ZERO,
            min_switches: 0,
            min_blocked: 0.0,
        };

        let mut grown = 0;
        for _ in 0..20 {
            let (got, added) = walk_with(&root, 2, 16, 32, always);
            assert_eq!(got.0, expected.0);
            assert_eq!(got.1, expected.1);
            grown = grown.max(added);
        }
        assert!(grown > 0 && grown <= 14, "added {grown} workers");

        // A fixed number of workers never grows, whatever the policy says.
        let (got, added) = walk_with(&root, 2, 2, 32, always);
        assert_eq!((got.0, added), (expected.0, 0));
    }
}

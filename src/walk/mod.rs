//! Parallel filesystem walk. Each worker reads whole directories and appends one record per kept
//! entry to its own [`Sink`], so no per-entry synchronization occurs. Directories are distributed
//! with per-worker LIFO stacks and a shared queue that is only used when another worker is idle.
//!
//! Entry selection matches [`ignore::WalkParallel`] as `erd` configured it before: the same ignore
//! rules ([`rules`]), hidden-file handling, `--exclude`/`--no-git` overrides, entry filter,
//! depth limit, same-file-system check and symlink loop detection. Entries whose metadata cannot
//! be read are dropped, and so are their contents.

use std::{
    ffi::OsStr,
    fs::{self, FileType, Metadata},
    mem,
    path::{Path, PathBuf},
    sync::{
        Condvar, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    thread,
};

/// Ignore rules per directory, adapted from the `ignore` crate.
pub mod rules;

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
    file_type: FileType,
    depth: usize,
    hidden: bool,
}

impl Entry<'_> {
    /// Full path of the entry, below the canonical root.
    pub const fn path(&self) -> &Path {
        self.path
    }

    /// Type of the entry; of the link target when symlinks are followed.
    pub const fn file_type(&self) -> FileType {
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
    pub metadata: &'a Metadata,
    /// Whether the entry is a symlink (followed or not).
    pub is_symlink: bool,
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
    pub filter: Option<Filter>,
    /// Count ignored entries per directory (`--report-ignored`).
    pub count_ignored: bool,
}

/// A directory waiting to be read.
struct Job {
    path: PathBuf,
    id: NodeRef,
    depth: usize,
    /// Rules of the parent directory.
    rules: Rules,
    /// Whether the directory is on another file system than the root (`-x`).
    foreign: bool,
}

/// Work shared between workers.
struct Shared {
    queue: Mutex<Vec<Job>>,
    wake: Condvar,
    /// Jobs queued or being processed. Zero means the walk is complete.
    pending: AtomicUsize,
    /// Workers blocked on `wake`.
    idle: AtomicUsize,
    quit: AtomicBool,
}

impl Walker {
    /// Walks the tree. `make_sink(i)` creates the sink with index `i`; the root record goes to
    /// the sink with index `threads`. Returns every sink, in index order. `progress` counts
    /// kept entries.
    pub fn run<S: Sink>(
        &self,
        make_sink: impl Fn(u32) -> S,
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

        let shared = Shared {
            queue: Mutex::new(Vec::new()),
            wake: Condvar::new(),
            pending: AtomicUsize::new(1),
            idle: AtomicUsize::new(0),
            quit: AtomicBool::new(false),
        };

        let mut root = Some(root);
        let root_sink = sinks.pop();

        // Each worker owns its sink, so no two threads write next to each other in memory.
        let mut sinks: Vec<S> = thread::scope(|s| {
            let handles: Vec<_> = sinks
                .into_iter()
                .enumerate()
                .map(|(i, sink)| {
                    let local = if i == 0 {
                        root.take().into_iter().collect()
                    } else {
                        vec![]
                    };
                    let shared = &shared;
                    s.spawn(move || {
                        let guard = QuitOnPanic(shared);
                        let mut worker = Worker {
                            walker: self,
                            sink_index: u32::try_from(i).unwrap_or(u32::MAX),
                            shared,
                            sink,
                            local,
                            progress,
                        };
                        worker.run();
                        mem::forget(guard);
                        worker.sink
                    })
                })
                .collect();

            handles
                .into_iter()
                .map(|h| h.join().unwrap_or_else(|e| std::panic::resume_unwind(e)))
                .collect()
        });

        sinks.extend(root_sink);
        sinks
    }

    /// Records the root and returns the job that reads it.
    fn root_job<S: Sink>(&self, sink: &mut S, sink_index: u32) -> Option<Job> {
        let metadata = fs::symlink_metadata(&self.root).ok()?;
        let name = self
            .root
            .file_name()
            .unwrap_or_else(|| self.root.as_os_str());

        let index = sink.push(&Record {
            parent: None,
            depth: 0,
            name,
            path: &self.root,
            metadata: &metadata,
            is_symlink: metadata.file_type().is_symlink(),
        })?;

        if !metadata.is_dir() || self.max_depth == Some(0) {
            return None;
        }

        Some(Job {
            path: self.root.clone(),
            id: NodeRef {
                sink: sink_index,
                index,
            },
            depth: 0,
            rules: self.rules.add_parents(&self.root),
            foreign: false,
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

struct Worker<'a, S> {
    walker: &'a Walker,
    sink_index: u32,
    shared: &'a Shared,
    sink: S,
    local: Vec<Job>,
    progress: Option<&'a AtomicU64>,
}

impl<S: Sink> Worker<'_, S> {
    fn run(&mut self) {
        let root_device = if self.walker.same_file_system {
            device_num(&self.walker.root)
        } else {
            None
        };

        while let Some(job) = self.next_job() {
            let kept = self.read_dir(job, root_device);

            if let Some(progress) = self.progress {
                progress.fetch_add(kept, Ordering::Relaxed);
            }

            self.share();

            if self.shared.pending.fetch_sub(1, Ordering::AcqRel) == 1 {
                // Last job done: wake everyone so they observe completion.
                let _guard = self.shared.queue.lock();
                self.shared.wake.notify_all();
            }
        }
    }

    /// Moves half of the local stack to the shared queue when a worker is idle.
    fn share(&mut self) {
        if self.local.len() < 2 || self.shared.idle.load(Ordering::Relaxed) == 0 {
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
    fn next_job(&mut self) -> Option<Job> {
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
    fn read_dir(&mut self, job: Job, root_device: Option<u64>) -> u64 {
        let walker = self.walker;

        if job.foreign {
            if walker.count_ignored {
                let listed = fs::read_dir(&job.path).map_or(0, Iterator::count);
                self.sink.add_ignored(listed);
            }
            return 0;
        }

        let Ok(read_dir) = fs::read_dir(&job.path) else {
            return 0;
        };

        let mut listed = 0;
        let mut entries = Vec::new();
        let mut found = Found::default();

        for entry in read_dir {
            listed += 1;
            if let Ok(entry) = entry {
                let name = entry.file_name();
                found.note(&name);
                entries.push((entry, name));
            }
        }

        let rules = job.rules.add_child(&job.path, Some(&found));
        let depth = job.depth + 1;
        let mut path = job.path;
        let mut kept = 0;

        for (entry, name) in &entries {
            path.push(name);

            if let Some((index, metadata)) = self.visit(entry, name, &path, &rules, job.id, depth) {
                kept += 1;

                if metadata.is_dir() && walker.max_depth.is_none_or(|max| depth < max) {
                    let foreign = root_device.is_some_and(|root| {
                        device_of(&metadata, &path).is_some_and(|dev| dev != root)
                    });

                    self.shared.pending.fetch_add(1, Ordering::AcqRel);
                    self.local.push(Job {
                        path: path.clone(),
                        id: NodeRef {
                            sink: self.sink_index,
                            index,
                        },
                        depth,
                        rules: rules.clone(),
                        foreign,
                    });
                }
            }

            path.pop();
        }

        if walker.count_ignored {
            self.sink
                .add_ignored(listed - usize::try_from(kept).unwrap_or(listed));
        }

        kept
    }

    /// Applies the walk rules to one entry and records it. Returns its index in this worker's
    /// sink and its metadata when kept.
    fn visit(
        &mut self,
        entry: &fs::DirEntry,
        name: &OsStr,
        path: &Path,
        rules: &Rules,
        parent: NodeRef,
        depth: usize,
    ) -> Option<(u32, Metadata)> {
        let walker = self.walker;
        let mut file_type = entry.file_type().ok()?;
        let is_symlink = file_type.is_symlink();
        let followed = if walker.follow_links && is_symlink {
            let metadata = fs::metadata(path).ok()?;
            file_type = metadata.file_type();

            if file_type.is_dir() && is_loop(rules, path) {
                return None;
            }

            Some(metadata)
        } else {
            None
        };

        let is_dir = file_type.is_dir();
        let hidden = || is_hidden(name, entry);

        if rules.is_ignored(path, is_dir, hidden) {
            return None;
        }

        if let Some(filter) = &walker.filter {
            let candidate = Entry {
                path,
                file_type,
                depth,
                hidden: is_hidden(name, entry),
            };

            if !filter(&candidate) {
                return None;
            }
        }

        let metadata = match followed {
            Some(metadata) => metadata,
            None => entry.metadata().ok()?,
        };

        let index = self.sink.push(&Record {
            parent: Some(parent),
            depth,
            name,
            path,
            metadata: &metadata,
            is_symlink,
        })?;

        Some((index, metadata))
    }
}

/// Whether the directory at `path` is one of the walked directories that lead to it.
fn is_loop(rules: &Rules, path: &Path) -> bool {
    let Ok(child) = same_file::Handle::from_path(path) else {
        // Upstream reports the error, which drops the entry.
        return true;
    };

    rules
        .walked_dirs()
        .any(|dir| same_file::Handle::from_path(dir).map_or(true, |ancestor| ancestor == child))
}

/// Whether `name` is hidden: it starts with `.` or, on Windows, has the hidden attribute.
fn is_hidden(name: &OsStr, entry: &fs::DirEntry) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;

        // Free on Windows: the attributes come with the directory listing.
        if entry
            .metadata()
            .is_ok_and(|md| md.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0)
        {
            return true;
        }
    }

    #[cfg(not(windows))]
    let _ = entry;

    name.as_encoded_bytes().first() == Some(&b'.')
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

/// Device number of a directory given its metadata.
// Only the Windows lookup can fail.
#[cfg_attr(unix, allow(clippy::unnecessary_wraps))]
fn device_of(metadata: &Metadata, path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        let _ = path;
        Some(metadata.dev())
    }

    #[cfg(not(unix))]
    {
        let _ = metadata;
        device_num(path)
    }
}

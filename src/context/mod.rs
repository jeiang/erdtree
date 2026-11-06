use super::disk_usage::{
    file_size::DiskUsage,
    threshold::{MinSize, parse_min_size},
    units::PrefixKind,
};

use crate::walk::Entry;
use args::Reconciler;
use clap::{
    CommandFactory, FromArgMatches, Parser, ValueEnum, error::ErrorKind, parser::ValueSource,
};
use color::Coloring;
use error::Error;
use ignore::overrides::{Override, OverrideBuilder};
use regex::Regex;
use std::{
    borrow::Borrow,
    convert::From,
    io::{IsTerminal, stdout},
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::OnceLock,
    thread::available_parallelism,
};

/// Concerned with figuring out how to reconcile arguments provided via the command-line with
/// arguments that come from a config file.
pub mod args;

/// Operations to load in defaults from configuration file.
pub mod config;

/// Controlling color of output.
pub mod color;

/// Controlling order of directories in output.
pub mod dir;

/// [Context] related errors.
pub mod error;

/// Common cross-platform file-types.
pub mod file;

/// For determining the output layout.
pub mod layout;

/// Utilities to print output.
pub mod column;

/// Printing order kinds.
pub mod sort;

/// Different types of timestamps available in long view.
#[cfg(unix)]
pub mod time;

/// Defines the CLI.
#[derive(Parser, Debug, Clone)]
#[command(name = "erdtree")]
#[command(author = "Benjamin Nguyen. <benjamin.van.nguyen@gmail.com>")]
#[command(version)]
#[command(about = "erdtree (erd) is a cross-platform, multi-threaded, and general purpose filesystem and disk usage utility.", long_about = None)]
pub struct Context {
    /// Directories to traverse, each shown as its own tree; defaults to current working directory
    dir: Vec<PathBuf>,

    /// Use configuration of named table rather than the top-level table in .erdtree.toml
    #[arg(short = 'c', long)]
    pub config: Option<String>,

    /// Mode of coloring output
    #[arg(short = 'C', long, value_enum, default_value_t)]
    pub color: Coloring,

    /// Print physical or logical file size
    #[arg(short, long, value_enum, default_value_t)]
    pub disk_usage: DiskUsage,

    /// Follow symlinks
    #[arg(short = 'f', long)]
    pub follow: bool,

    /// Append an indicator to entry names: / directory, @ symlink, * executable, | FIFO, = socket
    #[arg(short = 'F', long)]
    pub classify: bool,

    /// Print disk usage in human-readable format
    #[arg(short = 'H', long)]
    pub human: bool,

    /// Do not respect .gitignore files
    #[arg(short = 'i', long)]
    pub no_ignore: bool,

    /// Display file icons
    #[arg(short = 'I', long)]
    pub icons: bool,

    /// Show extended metadata and attributes
    #[cfg(unix)]
    #[arg(short, long)]
    pub long: bool,

    /// Show file's groups
    #[cfg(unix)]
    #[arg(long)]
    pub group: bool,

    /// Show each file's ino
    #[cfg(unix)]
    #[arg(long)]
    pub ino: bool,

    /// Show the total number of hardlinks to the underlying inode
    #[cfg(unix)]
    #[arg(long)]
    pub nlink: bool,

    /// Show permissions in numeric octal format instead of symbolic
    #[cfg(unix)]
    #[arg(long)]
    pub octal: bool,

    /// Which kind of timestamp to use; modified by default
    #[cfg(unix)]
    #[arg(long, value_enum)]
    pub time: Option<time::Stamp>,

    /// Which format to use for the timestamp; default by default
    #[cfg(unix)]
    #[arg(long = "time-format", value_enum)]
    pub time_format: Option<time::Format>,

    /// Maximum depth to display
    #[arg(short = 'L', long, value_name = "NUM")]
    level: Option<usize>,

    /// Limit the depth of the filesystem walk; directory sizes and file counts then only include
    /// traversed entries, unlike '--level' which only limits what is displayed
    #[arg(long, value_name = "NUM")]
    pub traversal_depth: Option<usize>,

    /// Regular expression (or glob if '--glob' or '--iglob' is used) used to match files
    #[arg(short, long)]
    pub pattern: Option<String>,

    /// Enables glob based searching
    #[arg(long)]
    pub glob: bool,

    /// Enables case-insensitive glob based searching
    #[arg(long)]
    pub iglob: bool,

    /// Restrict regex or glob search to a particular file-type
    #[arg(short = 't', long, value_enum)]
    pub file_type: Option<file::Type>,

    /// Hide entries smaller than SIZE in the active disk-usage metric (for directories, their
    /// total size); parent directories of shown entries stay. Totals and the summary file count
    /// still cover everything traversed. SIZE is an integer with an optional unit: B, KB, MB,
    /// GB, TB, PB (powers of 1000) or KiB, MiB, GiB, TiB, PiB (powers of 1024), case-insensitive;
    /// units are only valid with byte sizes ('--disk-usage logical|physical')
    #[arg(long, value_name = "SIZE", value_parser = parse_min_size)]
    pub min_size: Option<MinSize>,

    /// Remove empty directories from output
    #[arg(short = 'P', long)]
    pub prune: bool,

    /// How to sort entries
    #[arg(short, long, value_enum, default_value_t)]
    pub sort: sort::Type,

    /// Sort directories before or after all other file types
    #[arg(long, value_enum, default_value_t)]
    pub dir_order: dir::Order,

    /// Number of threads to use; defaults to the number of available CPU cores, and the walk then adds threads while it waits on the file system
    #[arg(short = 'T', long, default_value_t = Context::num_threads(), hide_default_value = true)]
    pub threads: usize,

    /// Report disk usage in binary or SI units
    #[arg(short, long, value_enum, default_value_t)]
    pub unit: PrefixKind,

    /// Prevent traversal into directories that are on different filesystems
    #[arg(short = 'x', long = "one-file-system")]
    pub same_fs: bool,

    /// Which kind of layout to use when rendering the output
    #[arg(short = 'y', long, value_enum, default_value_t)]
    pub layout: layout::Type,

    /// Show hidden files
    #[arg(short = '.', long)]
    pub hidden: bool,

    /// Exclude entries matching a gitignore-style glob (matched relative to the root); excluded
    /// directories are not descended into and do not count towards sizes. Can be repeated
    #[arg(long, value_name = "GLOB", action = clap::ArgAction::Append)]
    pub exclude: Vec<String>,

    /// Show hidden entries matching a gitignore-style glob (matched relative to the root) without
    /// '--hidden'; hidden entries inside a hidden directory need that directory included too.
    /// Does not show gitignored entries ('--no-ignore' does). Can be repeated
    #[arg(long, value_name = "GLOB", action = clap::ArgAction::Append)]
    pub include: Vec<String>,

    /// Disable traversal of .git directory when traversing hidden files
    #[arg(long)]
    pub no_git: bool,

    #[arg(long)]
    /// Print completions for a given shell to stdout
    pub completions: Option<clap_complete::Shell>,

    /// Only print directories
    #[arg(long)]
    pub dirs_only: bool,

    /// Don't read configuration file
    #[arg(long)]
    pub no_config: bool,

    /// Hides the progress indicator
    #[arg(long)]
    pub no_progress: bool,

    /// Omit the directory and file count summary from output
    #[arg(long)]
    pub no_report: bool,

    /// Add the number of entries skipped by hidden, gitignore, '--exclude' and '--no-git' rules
    /// to the summary; an ignored directory counts once. Reads each walked directory again
    #[arg(long)]
    pub report_ignored: bool,

    /// Print only the root and its total disk usage, like 'du -s'; same as '--level 0
    /// --no-report' and overrides '--level'
    #[arg(long)]
    pub summarize: bool,

    /// Omit disk usage from output
    #[arg(long)]
    pub suppress_size: bool,

    /// Truncate output to fit terminal emulator window
    #[arg(long)]
    pub truncate: bool,

    //////////////////////////
    /* INTERNAL USAGE BELOW */
    //////////////////////////
    /// Is stdin in a tty?
    #[clap(skip = stdout().is_terminal())]
    pub stdout_is_tty: bool,

    /// Whether `--threads` was given on the command line or in the config, which fixes the number
    /// of walk workers. Otherwise the walk may add workers when the file system is slow.
    #[clap(skip)]
    pub threads_explicit: bool,

    /// Whether `NO_COLOR` is set to a non-empty value (<https://no-color.org>).
    #[clap(skip)]
    pub no_color_env: bool,

    /// Canonical form of the root, computed on first use by [`Context::dir_canonical`].
    #[clap(skip)]
    canonical_dir: OnceLock<PathBuf>,
}

impl Context {
    /// Initializes [Context], optionally reading in the configuration file to override defaults.
    /// Arguments provided will take precedence over config.
    pub fn try_init() -> Result<Self, Error> {
        Self::compute_args().and_then(|mut args| {
            let no_color_env = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
            let threads_explicit = args.value_source("threads") == Some(ValueSource::CommandLine);
            let mut ctx = Self::from_arg_matches_mut(&mut args).map_err(Error::Config)?;
            ctx.threads_explicit = threads_explicit;
            ctx.no_color_env = no_color_env;

            // `--threads 0` means one thread per core.
            if ctx.threads == 0 {
                ctx.threads =
                    std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
            }

            ctx.validate_min_size()?;

            if ctx.summarize {
                ctx.level = Some(0);
                ctx.no_report = true;
            }

            Ok(ctx)
        })
    }

    /// A unit suffix on `--min-size` is meaningless for line, word, and block counts. This needs
    /// the merged `--disk-usage`, so it cannot be a [`clap`] value parser.
    fn validate_min_size(&self) -> Result<(), Error> {
        if self.min_size.is_some_and(|min| min.has_unit) && !self.byte_metric() {
            return Err(Error::ArgParse(Self::command().error(
                ErrorKind::ValueValidation,
                format!(
                    "invalid value for '--min-size <SIZE>': units are only supported with '--disk-usage logical' or 'physical'; give a bare number for '--disk-usage {}'",
                    self.disk_usage.to_possible_value().map_or_else(String::new, |v| v.get_name().to_owned())
                ),
            )));
        }

        Ok(())
    }

    /// Whether output is plain: always when `NO_COLOR` is set to a non-empty value, even with
    /// `--color force`; otherwise with `--color none`, or `--color auto` when stdout is not a tty.
    pub const fn no_color(&self) -> bool {
        if self.no_color_env {
            return true;
        }

        match self.color {
            Coloring::Auto if !self.stdout_is_tty => true,
            Coloring::None => true,
            Coloring::Auto | Coloring::Force => false,
        }
    }

    /// Returns [Path] of the root directory to be traversed.
    pub fn dir(&self) -> &Path {
        self.dir
            .first()
            .map_or_else(|| Path::new("."), PathBuf::as_path)
    }

    /// One [Context] per root directory given on the command line, in order.
    pub fn split_roots(self) -> Vec<Self> {
        if self.dir.len() <= 1 {
            return vec![self];
        }

        self.dir
            .iter()
            .map(|dir| Self {
                dir: vec![dir.clone()],
                canonical_dir: OnceLock::new(),
                ..self.clone()
            })
            .collect()
    }

    /// Returns canonical [Path] of the root directory to be traversed. The filesystem is asked
    /// once per [`Context`]; the root itself is used when it cannot be canonicalized.
    pub fn dir_canonical(&self) -> &Path {
        self.canonical_dir.get_or_init(|| {
            std::fs::canonicalize(self.dir()).unwrap_or_else(|_| self.dir().to_path_buf())
        })
    }

    /// Canonicalizes the root like [`Self::dir_canonical`], but reports a failure instead of
    /// falling back. A success is cached, so that the walk and its filters ask the filesystem
    /// once.
    pub fn try_dir_canonical(&self) -> std::io::Result<&Path> {
        if let Some(dir) = self.canonical_dir.get() {
            return Ok(dir);
        }
        let dir = std::fs::canonicalize(self.dir())?;
        Ok(self.canonical_dir.get_or_init(|| dir))
    }

    /// The max depth to print. Note that all directories are fully traversed to compute file
    /// sizes; this just determines how much to print.
    pub fn level(&self) -> usize {
        self.level.unwrap_or(usize::MAX)
    }

    /// Which timestamp type to use for long view; defaults to modified.
    #[cfg(unix)]
    pub fn time(&self) -> time::Stamp {
        self.time.unwrap_or_default()
    }

    /// Which format to use for the timestamp; default by default
    #[cfg(unix)]
    pub fn time_format(&self) -> time::Format {
        self.time_format.unwrap_or_default()
    }

    /// Which `FileType` to filter on; defaults to regular file.
    pub fn file_type(&self) -> file::Type {
        self.file_type.unwrap_or_default()
    }

    /// Whether `--include` is given without `--hidden`. Hidden entries are then filtered by the
    /// [`EntryFilter`] instead of the walker's own hidden-entry filter, which cannot make
    /// exceptions.
    pub const fn include_hidden(&self) -> bool {
        !self.hidden && !self.include.is_empty()
    }

    /// The filter applied to each walked entry for `--include` (see [`Self::include_hidden`]),
    /// `--pattern` and `--file-type`; `None` when there is nothing to filter.
    pub fn entry_filter(&self) -> Result<Option<EntryFilter>, Error> {
        let include = if self.include_hidden() {
            let mut builder = OverrideBuilder::new(self.dir_canonical());

            for glob in &self.include {
                builder.add(glob.trim_start_matches('!'))?;
            }

            Some(builder.build()?)
        } else {
            None
        };

        let pattern = match self.pattern.as_deref() {
            None => None,
            Some(glob) if self.glob || self.iglob => {
                let mut builder = OverrideBuilder::new(self.dir_canonical());

                if self.iglob {
                    builder.case_insensitive(true)?;
                }

                let trim = glob.trim_start();
                let negated = trim.starts_with('!');
                builder.add(trim.trim_start_matches('!'))?;

                Some(Pattern::Glob {
                    ovr: builder.build()?,
                    negated,
                })
            },
            Some(re) => Some(Pattern::Regex(Regex::new(re)?)),
        };

        if include.is_none() && pattern.is_none() {
            return Ok(None);
        }

        Ok(Some(EntryFilter {
            include,
            pattern: pattern.map(|pattern| (pattern, self.file_type())),
        }))
    }

    /// Special override to toggle the visibility of the git directory and to apply `--exclude`.
    pub fn no_git_override(&self) -> Result<Override, Error> {
        let mut builder = OverrideBuilder::new(self.dir_canonical());

        if self.no_git {
            builder.add("!.git")?;
        }

        for glob in &self.exclude {
            builder.add(&format!("!{}", glob.trim_start_matches('!')))?;
        }

        Ok(builder.build()?)
    }

    /// Answers whether disk usage is asked to be reported in bytes.
    pub const fn byte_metric(&self) -> bool {
        matches!(self.disk_usage, DiskUsage::Logical | DiskUsage::Physical)
    }

    /// The default number of threads to use for disk-reads and parallel processing. The answer is
    /// cached because `available_parallelism` costs about ten syscalls on Linux (cgroup files) and
    /// clap evaluates the default at every command build.
    pub fn num_threads() -> usize {
        static THREADS: OnceLock<usize> = OnceLock::new();
        *THREADS.get_or_init(|| {
            let available = available_parallelism().map_or(3, NonZeroUsize::get);
            Self::performance_cores().map_or(available, |cores| cores.min(available))
        })
    }

    /// The number of performance cores on Apple silicon, where a walk with more threads than
    /// that is slower: threads on efficiency cores hold the kernel's file system locks longer,
    /// so every thread waits for them and kernel time grows several times over. `None` when the
    /// machine does not tell cores apart.
    #[cfg(target_os = "macos")]
    fn performance_cores() -> Option<usize> {
        let mut cores: libc::c_int = 0;
        let mut len = size_of::<libc::c_int>();
        // SAFETY: the name is NUL-terminated and `cores` has room for the `len` bytes asked for.
        let ret = unsafe {
            libc::sysctlbyname(
                c"hw.perflevel0.physicalcpu".as_ptr(),
                (&raw mut cores).cast(),
                &raw mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        if ret != 0 || len != size_of::<libc::c_int>() {
            return None;
        }
        usize::try_from(cores).ok().filter(|&n| n > 0)
    }

    /// Cores are not told apart outside macOS.
    #[cfg(not(target_os = "macos"))]
    const fn performance_cores() -> Option<usize> {
        None
    }
}

/// What `--pattern` matches: a regular expression against entry names, or a glob (rooted at
/// the root, optionally negated with a leading `!`) against entry paths.
enum Pattern {
    Regex(Regex),
    Glob { ovr: Override, negated: bool },
}

impl Pattern {
    /// Does the entry itself match: its name for a regex, its path for a glob?
    fn leaf(&self, entry: &Entry<'_>) -> bool {
        match self {
            Self::Regex(re) => re.is_match(&entry.file_name().to_string_lossy()),
            Self::Glob { ovr, negated } => {
                ovr.matched(entry.path(), false).is_whitelist() != *negated
            },
        }
    }

    /// Does any component of the entry's path below the root, after dropping the last `skip`,
    /// match? A negated glob holds when none does. An entry at depth `d` has `d` components
    /// below the root.
    fn ancestors(&self, entry: &Entry<'_>, skip: usize) -> bool {
        let mut below_root = entry
            .path()
            .components()
            .rev()
            .take(entry.depth())
            .skip(skip);

        match self {
            Self::Regex(re) => {
                below_root.any(|c| re.is_match(c.as_os_str().to_string_lossy().borrow()))
            },
            Self::Glob { ovr, negated } => {
                below_root.any(|c| ovr.matched(c, false).is_whitelist()) != *negated
            },
        }
    }
}

/// Decides which walked entries are kept; built by [`Context::entry_filter`].
pub struct EntryFilter {
    /// With [`Context::include_hidden`]: the `--include` globs, which hidden entries must match.
    include: Option<Override>,
    /// `--pattern` and `--file-type`.
    pattern: Option<(Pattern, file::Type)>,
}

impl EntryFilter {
    /// Whether `entry` is kept (and, for a directory, descended into). With `--file-type dir`, an
    /// entry is kept when it or a directory above it (below the root) matches. Otherwise
    /// directories are always kept, since matched files are bridged back to the root through
    /// them; empty directories are pruned later by [`Tree`].
    ///
    /// [`Tree`]: crate::tree::Tree
    pub fn keep(&self, entry: &Entry<'_>) -> bool {
        let kind = entry.file_type();

        if let Some(include) = &self.include
            && entry.is_hidden()
            && !include.matched(entry.path(), kind.is_dir()).is_whitelist()
        {
            return false;
        }

        let Some((pattern, file_type)) = &self.pattern else {
            return true;
        };

        match file_type {
            file::Type::Dir => pattern.ancestors(entry, usize::from(!kind.is_dir())),
            _ if kind.is_dir() => true,
            file::Type::File if !kind.is_file() => false,
            file::Type::Link if !kind.is_symlink() => false,
            _ => pattern.leaf(entry),
        }
    }
}

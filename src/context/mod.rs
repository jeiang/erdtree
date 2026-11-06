use super::disk_usage::{
    file_size::DiskUsage,
    threshold::{MinSize, parse_min_size},
    units::PrefixKind,
};

use clap::{
    CommandFactory, FromArgMatches, Parser, ValueEnum, error::ErrorKind, parser::ValueSource,
};
use color::Coloring;
use error::Error;
use std::{
    convert::From,
    io::{IsTerminal, stdout},
    num::NonZeroUsize,
    path::PathBuf,
    sync::OnceLock,
    thread::available_parallelism,
};

/// Concerned with figuring out how to reconcile arguments provided via the command-line with
/// arguments that come from a config file.
pub mod args;

/// Operations to load in defaults from configuration file.
pub mod config;

pub mod color;

pub mod dir;

pub mod error;

pub mod file;

pub mod layout;

pub mod sort;

#[cfg(unix)]
pub mod time;

/// Defines the CLI.
#[derive(Parser, Debug, Clone)]
#[command(name = "erdtree")]
#[command(author = "Benjamin Nguyen. <benjamin.van.nguyen@gmail.com>")]
#[command(version)]
#[command(about = "erdtree (erd) is a cross-platform, multi-threaded, and general purpose filesystem and disk usage utility.", long_about = None)]
pub struct Context {
    // `Context::try_init` moves the roots out and returns them beside the options.
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

    /// How to sort entries; orders read outward from the root: bottom to top in the 'regular'
    /// and 'flat' layouts, top to bottom in 'inverted' and 'iflat'
    #[arg(short, long, value_enum, default_value_t)]
    pub sort: sort::Type,

    /// Sort directories before or after all other file types
    #[arg(long, value_enum, default_value_t)]
    pub dir_order: dir::Order,

    /// Number of threads to use; defaults to the number of available CPU cores (performance cores on macOS), as does 0; on Linux, without this option the walk adds threads while it waits on the file system
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

    /// Print only one line per root with its total disk usage, like 'du -s'; overrides
    /// '--level'
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
    /// Whether stdout is a terminal.
    #[clap(skip = stdout().is_terminal())]
    pub stdout_is_tty: bool,

    /// Whether `--threads` was given on the command line or in the config, which fixes the number
    /// of walk workers. Otherwise the walk may add workers when the file system is slow.
    #[clap(skip)]
    pub threads_explicit: bool,

    /// Whether `NO_COLOR` is set to a non-empty value (<https://no-color.org>).
    #[clap(skip)]
    pub no_color_env: bool,
}

impl Context {
    /// Initializes [Context], optionally reading in the configuration file to override defaults.
    /// Arguments provided will take precedence over config. Also returns the root directories,
    /// in command-line order; the current directory when none is given.
    pub fn try_init() -> Result<(Self, Vec<PathBuf>), Error> {
        let no_color_env = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
        let mut ctx =
            args::compute_args().and_then(|args| Self::from_merged(args, no_color_env))?;

        let mut roots = std::mem::take(&mut ctx.dir);
        if roots.is_empty() {
            roots.push(PathBuf::from("."));
        }

        Ok((ctx, roots))
    }

    /// Builds [Context] from the merged command-line and config matches.
    fn from_merged(mut args: clap::ArgMatches, no_color_env: bool) -> Result<Self, Error> {
        let threads_explicit = args.value_source("threads") == Some(ValueSource::CommandLine);
        let mut ctx = Self::from_arg_matches_mut(&mut args).map_err(Error::Config)?;
        ctx.threads_explicit = threads_explicit;
        ctx.no_color_env = no_color_env;

        if ctx.threads == 0 {
            ctx.threads = Self::num_threads();
        }

        ctx.validate_min_size()?;

        if ctx.summarize {
            ctx.level = Some(0);
            ctx.no_report = true;
        }

        Ok(ctx)
    }

    /// A unit suffix on `--min-size` is meaningless for line, word, and block counts. This needs
    /// the merged `--disk-usage`, so it cannot be a [`clap`] value parser.
    fn validate_min_size(&self) -> Result<(), Error> {
        if self.min_size.is_some_and(|min| min.has_unit) && !self.disk_usage.is_bytes() {
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

    /// The max depth to print, unlimited when `--level` is unset. It does not limit the walk
    /// (`--traversal-depth` does).
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

    /// The number of threads used by default and for `--threads 0`: the available parallelism,
    /// capped at the performance cores. The answer is cached because `available_parallelism`
    /// costs about ten syscalls on Linux (cgroup files) and clap evaluates the default at every
    /// command build.
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

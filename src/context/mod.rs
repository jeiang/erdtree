use super::disk_usage::{
    file_size::DiskUsage,
    threshold::{MinSize, parse_min_size},
    units::PrefixKind,
};

use args::Reconciler;
use clap::{CommandFactory, FromArgMatches, Parser, ValueEnum, error::ErrorKind};
use color::Coloring;
use error::Error;
use ignore::{
    DirEntry,
    overrides::{Override, OverrideBuilder},
};
use regex::Regex;
use std::{
    borrow::Borrow,
    convert::From,
    io::{IsTerminal, stdin, stdout},
    num::NonZeroUsize,
    path::{Path, PathBuf},
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
#[derive(Parser, Debug)]
#[command(name = "erdtree")]
#[command(author = "Benjamin Nguyen. <benjamin.van.nguyen@gmail.com>")]
#[command(version = "3.1.2")]
#[command(about = "erdtree (erd) is a cross-platform, multi-threaded, and general purpose filesystem and disk usage utility.", long_about = None)]
pub struct Context {
    /// Directory to traverse; defaults to current working directory
    dir: Option<PathBuf>,

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

    /// Number of threads to use; defaults to the number of available CPU cores
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
    #[clap(skip = stdin().is_terminal())]
    pub stdin_is_tty: bool,

    /// Is stdin in a tty?
    #[clap(skip = stdout().is_terminal())]
    pub stdout_is_tty: bool,

    /// Restricts column width of size not including units
    #[clap(skip = usize::default())]
    pub max_size_width: usize,

    /// Restricts column width of `disk_usage` units
    #[clap(skip = usize::default())]
    pub max_size_unit_width: usize,

    /// Restricts column width of nlink for long view
    #[clap(skip = usize::default())]
    #[cfg(unix)]
    pub max_nlink_width: usize,

    /// Restricts column width of ino for long view
    #[clap(skip = usize::default())]
    #[cfg(unix)]
    pub max_ino_width: usize,

    /// Restricts column width of block for long view
    #[clap(skip = usize::default())]
    #[cfg(unix)]
    pub max_block_width: usize,

    /// Restricts column width of file owner for long view
    #[clap(skip = usize::default())]
    #[cfg(unix)]
    pub max_owner_width: usize,

    /// Restricts column width of file group for long view
    #[clap(skip = usize::default())]
    #[cfg(unix)]
    pub max_group_width: usize,

    /// Width of the terminal emulator's window
    #[clap(skip)]
    pub window_width: Option<usize>,
}

type Predicate = Result<Box<dyn Fn(&DirEntry) -> bool + Send + Sync + 'static>, Error>;

impl Context {
    /// Initializes [Context], optionally reading in the configuration file to override defaults.
    /// Arguments provided will take precedence over config.
    pub fn try_init() -> Result<Self, Error> {
        Self::compute_args().and_then(|args| {
            color::no_color_env();
            let mut ctx = Self::from_arg_matches(&args).map_err(Error::Config)?;
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

    /// Determines whether or not it's appropriate to display color in output based on
    /// the Coloring, and whether or not stdout is connected to a tty.
    ///
    /// If Coloring is Force then this will always evaluate to `false`.
    pub fn no_color(&self) -> bool {
        if let Some(Some(var)) = color::NO_COLOR.get() {
            return !var.is_empty();
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
            .as_ref()
            .map_or_else(|| Path::new("."), |pb| pb.as_path())
    }

    /// Returns canonical [Path] of the root directory to be traversed.
    pub fn dir_canonical(&self) -> PathBuf {
        std::fs::canonicalize(self.dir()).unwrap_or_else(|_| self.dir().to_path_buf())
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

    /// Predicate used for filtering via regular expressions and file-type. When matching regular
    /// files, directories will always be included since matched files will need to be bridged back
    /// to the root node somehow. Empty sets not producing an output is handled by [`Tree`].
    ///
    /// [`Tree`]: crate::tree::Tree
    pub fn regex_predicate(&self) -> Predicate {
        let Some(pattern) = self.pattern.as_ref() else {
            return Err(Error::PatternNotProvided);
        };

        let re = Regex::new(pattern)?;

        let file_type = self.file_type();

        Ok(match file_type {
            file::Type::Dir => Box::new(move |dir_entry| {
                let is_dir = dir_entry.file_type().is_some_and(|ft| ft.is_dir());
                if is_dir {
                    return Self::ancestor_regex_match(dir_entry.path(), &re, 0);
                }

                Self::ancestor_regex_match(dir_entry.path(), &re, 1)
            }),

            _ => Box::new(move |dir_entry| {
                let entry_type = dir_entry.file_type();
                let is_dir = entry_type.is_some_and(|ft| ft.is_dir());

                if is_dir {
                    return true;
                }

                match file_type {
                    file::Type::File if entry_type.is_none_or(|ft| !ft.is_file()) => {
                        return false;
                    },
                    file::Type::Link if entry_type.is_none_or(|ft| !ft.is_symlink()) => {
                        return false;
                    },
                    _ => {},
                }
                let file_name = dir_entry.file_name().to_string_lossy();
                re.is_match(&file_name)
            }),
        })
    }

    /// Predicate used for filtering via globs and file-types.
    pub fn glob_predicate(&self) -> Predicate {
        let mut builder = OverrideBuilder::new(self.dir_canonical());

        let mut negated_glob = false;

        let overrides = {
            if self.iglob {
                builder.case_insensitive(true)?;
            }

            if let Some(ref glob) = self.pattern {
                let trim = glob.trim_start();
                negated_glob = trim.starts_with('!');

                if negated_glob {
                    builder.add(trim.trim_start_matches('!'))?;
                } else {
                    builder.add(trim)?;
                }
            }

            builder.build()?
        };

        let file_type = self.file_type();

        match file_type {
            file::Type::Dir => Ok(Box::new(move |dir_entry| {
                let is_dir = dir_entry.file_type().is_some_and(|ft| ft.is_dir());

                if is_dir {
                    if negated_glob {
                        return !Self::ancestor_glob_match(dir_entry.path(), &overrides, 0);
                    }
                    return Self::ancestor_glob_match(dir_entry.path(), &overrides, 0);
                }
                let matched = Self::ancestor_glob_match(dir_entry.path(), &overrides, 1);

                if negated_glob { !matched } else { matched }
            })),

            _ => Ok(Box::new(move |dir_entry| {
                let entry_type = dir_entry.file_type();
                let is_dir = entry_type.is_some_and(|ft| ft.is_dir());

                if is_dir {
                    return true;
                }

                match file_type {
                    file::Type::File if entry_type.is_none_or(|ft| !ft.is_file()) => {
                        return false;
                    },
                    file::Type::Link if entry_type.is_none_or(|ft| !ft.is_symlink()) => {
                        return false;
                    },
                    _ => {},
                }

                let matched = overrides.matched(dir_entry.path(), false);

                if negated_glob {
                    !matched.is_whitelist()
                } else {
                    matched.is_whitelist()
                }
            })),
        }
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

    /// Hides hidden entries except those matching `--include`. Used instead of the walker's own
    /// hidden-entry filter, which cannot make exceptions, when `--include` is given without
    /// `--hidden`.
    pub fn include_predicate(&self) -> Predicate {
        let mut builder = OverrideBuilder::new(self.dir_canonical());

        for glob in &self.include {
            builder.add(glob.trim_start_matches('!'))?;
        }

        let include = builder.build()?;

        Ok(Box::new(move |dir_entry: &DirEntry| {
            let is_dir = dir_entry.file_type().is_some_and(|ft| ft.is_dir());

            dir_entry.depth() == 0
                || !is_hidden(dir_entry)
                || include.matched(dir_entry.path(), is_dir).is_whitelist()
        }))
    }

    /// Update column width properties.
    pub const fn update_column_properties(&mut self, col_props: &column::Properties) {
        self.max_size_width = col_props.max_size_width;
        self.max_size_unit_width = col_props.max_size_unit_width;

        #[cfg(unix)]
        {
            self.max_owner_width = col_props.max_owner_width;
            self.max_group_width = col_props.max_group_width;
            self.max_nlink_width = col_props.max_nlink_width;
            self.max_block_width = col_props.max_block_width;
            self.max_ino_width = col_props.max_ino_width;
        }
    }

    /// Setter for `window_width` which is set to the current terminal emulator's window width.
    #[inline]
    pub fn set_window_width(&mut self) {
        self.window_width = crate::tty::get_window_width();
    }

    /// Answers whether disk usage is asked to be reported in bytes.
    pub const fn byte_metric(&self) -> bool {
        matches!(self.disk_usage, DiskUsage::Logical | DiskUsage::Physical)
    }

    /// Do any of the components of a path match the provided glob? This is used for ensuring that
    /// all children of a directory that a glob targets gets captured.
    #[inline]
    fn ancestor_glob_match(path: &Path, ovr: &Override, skip: usize) -> bool {
        path.components()
            .rev()
            .skip(skip)
            .any(|c| ovr.matched(c, false).is_whitelist())
    }

    /// Like [`Self::ancestor_glob_match`] except uses [Regex] rather than [Override].
    #[inline]
    fn ancestor_regex_match(path: &Path, re: &Regex, skip: usize) -> bool {
        path.components()
            .rev()
            .skip(skip)
            .any(|comp| re.is_match(comp.as_os_str().to_string_lossy().borrow()))
    }

    /// The default number of threads to use for disk-reads and parallel processing.
    fn num_threads() -> usize {
        available_parallelism().map_or(3, NonZeroUsize::get)
    }
}

/// Whether the walker's hidden-entry filter would skip `dir_entry`: its name starts with `.` or,
/// on Windows, it has the hidden attribute.
fn is_hidden(dir_entry: &DirEntry) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;

        if dir_entry
            .metadata()
            .is_ok_and(|md| md.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0)
        {
            return true;
        }
    }

    dir_entry.file_name().as_encoded_bytes().first() == Some(&b'.')
}

//! Which walked entries are kept, besides the ignore rules: the hidden-entry policy, the
//! `--exclude` and `--no-git` globs, and the entry filter of `--include`, `--pattern` and
//! `--file-type`. All globs are rooted at the canonical root.

use super::Entry;
use crate::context::file;
use ignore::overrides::{Override, OverrideBuilder};
use regex::Regex;
use std::{borrow::Borrow, path::Path};

/// A glob or regular expression that does not compile.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Glob(#[from] ignore::Error),

    #[error("{0}")]
    Regex(#[from] regex::Error),
}

/// What happens to hidden entries.
pub enum Hidden {
    /// `--hidden`: shown.
    Show,
    /// Skipped by the ignore rules (a rule may still whitelist one).
    Hide,
    /// `--include` without `--hidden`: skipped unless they match the `--include` globs. The ignore
    /// rules cannot make such exceptions, so the [`EntryFilter`] skips them instead.
    HideExcept(Override),
}

impl Hidden {
    /// The policy of `--hidden` (`show`) and the `--include` globs.
    pub fn new(root: &Path, show: bool, include: &[String]) -> Result<Self, Error> {
        if show {
            return Ok(Self::Show);
        }

        if include.is_empty() {
            return Ok(Self::Hide);
        }

        let mut builder = OverrideBuilder::new(root);

        for glob in include {
            builder.add(glob.trim_start_matches('!'))?;
        }

        Ok(Self::HideExcept(builder.build()?))
    }

    /// Whether the ignore rules skip hidden entries.
    pub const fn hidden_by_rules(&self) -> bool {
        matches!(self, Self::Hide)
    }
}

/// The overrides of the ignore rules: `--no-git` skips `.git`, and each `--exclude` glob skips
/// what it matches.
pub fn exclusions(root: &Path, no_git: bool, exclude: &[String]) -> Result<Override, Error> {
    let mut builder = OverrideBuilder::new(root);

    if no_git {
        builder.add("!.git")?;
    }

    for glob in exclude {
        builder.add(&format!("!{}", glob.trim_start_matches('!')))?;
    }

    Ok(builder.build()?)
}

/// What `--pattern` matches: a regular expression against entry names, or a glob (rooted at
/// the root, optionally negated with a leading `!`) against entry paths.
pub enum Pattern {
    Regex(Regex),
    Glob { ovr: Override, negated: bool },
}

impl Pattern {
    /// `pattern` as a glob with `glob` or `iglob` (case-insensitive), else as a regular
    /// expression.
    pub fn new(root: &Path, pattern: &str, glob: bool, iglob: bool) -> Result<Self, Error> {
        if !(glob || iglob) {
            return Ok(Self::Regex(Regex::new(pattern)?));
        }

        let mut builder = OverrideBuilder::new(root);

        if iglob {
            builder.case_insensitive(true)?;
        }

        let trim = pattern.trim_start();
        let negated = trim.starts_with('!');
        builder.add(trim.trim_start_matches('!'))?;

        Ok(Self::Glob {
            ovr: builder.build()?,
            negated,
        })
    }

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

/// Decides which walked entries are kept.
pub struct EntryFilter {
    /// With [`Hidden::HideExcept`]: the `--include` globs, which hidden entries must match.
    include: Option<Override>,
    /// `--pattern` and `--file-type`.
    pattern: Option<(Pattern, file::Type)>,
}

impl EntryFilter {
    /// The filter of the hidden-entry policy and of `--pattern` with `--file-type`; `None` when
    /// it would keep every entry.
    pub fn new(hidden: Hidden, pattern: Option<(Pattern, file::Type)>) -> Option<Self> {
        let include = match hidden {
            Hidden::HideExcept(include) => Some(include),
            Hidden::Show | Hidden::Hide => None,
        };

        if include.is_none() && pattern.is_none() {
            return None;
        }

        Some(Self { include, pattern })
    }

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

//! Per-directory ignore rules, adapted from `dir.rs` of the [`ignore`] crate (0.4.33, by Andrew
//! Gallant, dual-licensed Unlicense OR MIT), so that the custom walker in [`super`] applies the
//! exact `.gitignore`, `.ignore`, `.git/info/exclude`, global gitignore, override and hidden-file
//! precedence of [`ignore::WalkParallel`]. The rules are a persistent linked list: each directory
//! points at the rules of its parent, so workers share them without locks.
//!
//! Differences from upstream: the type matcher, explicit ignore files and custom ignore file names
//! are dropped (`erd` never sets them), and errors while reading ignore files are dropped, as `erd`
//! skipped them before. Matching only answers "ignored" or "not ignored".

use ignore::{
    Match,
    gitignore::{Gitignore, GitignoreBuilder},
    overrides::Override,
};
use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    fs::{File, FileType},
    io::{self, BufRead},
    path::{Path, PathBuf},
    sync::{Arc, RwLock, Weak},
};

/// Which kinds of rules apply. Mirrors the options `erd` passes to [`ignore::WalkBuilder`].
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Skip hidden entries unless a rule whitelists them.
    pub hidden: bool,
    /// Read `.ignore` files.
    pub ignore: bool,
    /// Respect ignore files in the parent directories of the root.
    pub parents: bool,
    /// Read git's global gitignore.
    pub git_global: bool,
    /// Read `.gitignore` files.
    pub git_ignore: bool,
    /// Read `.git/info/exclude`.
    pub git_exclude: bool,
    /// Apply git rules only inside a git (or jj) repository.
    pub require_git: bool,
}

impl Options {
    /// The defaults of [`ignore::WalkBuilder`] with `git_ignore`, `git_global`, `git_exclude`
    /// and `ignore` all set to `respect_ignore`, as `erd` sets them.
    pub const fn new(hidden: bool, respect_ignore: bool) -> Self {
        Self {
            hidden,
            ignore: true,
            parents: true,
            git_global: respect_ignore,
            git_ignore: respect_ignore,
            git_exclude: true,
            require_git: true,
        }
    }
}

/// Ignore rules for one directory and, through `parent`, all of its ancestors.
#[derive(Clone, Debug)]
pub struct Rules {
    inner: Arc<Inner>,
    /// The canonical path given to [`Rules::add_parents`]; used to match against the ignore files
    /// of the root's ancestors.
    absolute_base: Option<Arc<PathBuf>>,
}

#[derive(Debug)]
struct Inner {
    /// Matchers already built for ancestor directories, shared by every root.
    compiled: Arc<RwLock<HashMap<OsString, Weak<Self>>>>,
    dir: PathBuf,
    overrides: Arc<Override>,
    parent: Option<Arc<Self>>,
    is_absolute_parent: bool,
    ignore_matcher: Gitignore,
    git_global_matcher: Arc<Gitignore>,
    git_ignore_matcher: Gitignore,
    git_exclude_matcher: Gitignore,
    /// Whether this directory contains `.git` (or `.jj`).
    has_git: bool,
    /// Whether this directory or an ancestor contains `.git` (or `.jj`).
    any_git: bool,
    /// Whether any matcher of this directory or its ancestors is non-empty. When `false`, no
    /// ignore file can decide, so matching skips the walk up the ancestors.
    any_matcher: bool,
    opts: Options,
}

impl Inner {
    /// Sets the derived flags from this directory's own matchers and its parent.
    fn derive(mut self) -> Self {
        let parent = self.parent.as_deref();
        self.any_git = self.has_git || parent.is_some_and(|p| p.any_git);
        self.any_matcher = !self.ignore_matcher.is_empty()
            || !self.git_ignore_matcher.is_empty()
            || !self.git_exclude_matcher.is_empty()
            || parent.is_some_and(|p| p.any_matcher);
        self
    }
}

/// Which ignore-relevant names a directory listing contains, so that ignore files are only
/// opened when present.
#[derive(Clone, Copy, Debug, Default)]
pub struct Found {
    pub has_ignore: bool,
    pub has_git_ignore: bool,
    pub has_git_dir: bool,
    pub has_jj_dir: bool,
}

impl Found {
    /// Records `name` if it is relevant.
    pub fn note(&mut self, name: &OsStr) {
        match name.as_encoded_bytes() {
            b".ignore" => self.has_ignore = true,
            b".gitignore" => self.has_git_ignore = true,
            b".git" => self.has_git_dir = true,
            b".jj" => self.has_jj_dir = true,
            _ => {},
        }
    }
}

impl Rules {
    /// The rules before any directory is added. `cwd` anchors the global gitignore; without it
    /// the global gitignore is not used.
    pub fn root(opts: Options, overrides: Override, cwd: Option<PathBuf>) -> Self {
        let git_global_matcher = match cwd {
            Some(cwd) if opts.git_global => GitignoreBuilder::new(cwd).build_global().0,
            _ => Gitignore::empty(),
        };

        Self {
            inner: Arc::new(Inner {
                compiled: Arc::new(RwLock::new(HashMap::new())),
                dir: PathBuf::new(),
                overrides: Arc::new(overrides),
                parent: None,
                is_absolute_parent: true,
                ignore_matcher: Gitignore::empty(),
                git_global_matcher: Arc::new(git_global_matcher),
                git_ignore_matcher: Gitignore::empty(),
                git_exclude_matcher: Gitignore::empty(),
                has_git: false,
                any_git: false,
                any_matcher: false,
                opts,
            }),
            absolute_base: None,
        }
    }

    /// Adds the rules of every ancestor of `path`. Only valid on [`Rules::root`].
    pub fn add_parents(&self, path: &Path) -> Self {
        let opts = self.inner.opts;

        if !opts.parents && !opts.git_ignore && !opts.git_exclude && !opts.git_global {
            return self.clone();
        }

        let Ok(absolute_base) = path.canonicalize().map(Arc::new) else {
            return self.clone();
        };

        let mut parents = vec![];
        let mut path = absolute_base.as_path();
        while let Some(parent) = path.parent() {
            parents.push(parent);
            path = parent;
        }

        let mut rules = self.clone();
        for parent in parents.into_iter().rev() {
            let compiled = &self.inner.compiled;
            let lock = || {
                compiled
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
            };

            let prebuilt = lock().get(parent.as_os_str()).and_then(Weak::upgrade);
            if let Some(prebuilt) = prebuilt {
                rules = Self {
                    inner: prebuilt,
                    absolute_base: Some(Arc::clone(&absolute_base)),
                };
                continue;
            }

            let mut inner = rules.child_inner(parent, None);
            inner.is_absolute_parent = true;
            inner.has_git = opts.require_git
                && opts.git_ignore
                && (parent.join(".git").exists() || parent.join(".jj").exists());

            let inner = Arc::new(inner.derive());
            lock().insert(parent.as_os_str().to_os_string(), Arc::downgrade(&inner));
            rules = Self {
                inner,
                absolute_base: Some(Arc::clone(&absolute_base)),
            };
        }

        rules
    }

    /// The rules of child directory `dir`, given which ignore files its listing contains. With
    /// `found` as `None` the ignore files are probed on disk.
    pub fn add_child(&self, dir: &Path, found: Option<&Found>) -> Self {
        Self {
            inner: Arc::new(self.child_inner(dir, found).derive()),
            absolute_base: self.absolute_base.clone(),
        }
    }

    fn child_inner(&self, dir: &Path, found: Option<&Found>) -> Inner {
        let opts = self.inner.opts;
        let check_vcs_dir = opts.require_git && (opts.git_ignore || opts.git_exclude);

        let git_type = if check_vcs_dir && found.is_none_or(|f| f.has_git_dir) {
            dir.join(".git").metadata().ok().map(|md| md.file_type())
        } else {
            None
        };
        let has_jj =
            check_vcs_dir && found.is_none_or(|f| f.has_jj_dir) && dir.join(".jj").exists();
        let has_git = check_vcs_dir && (git_type.is_some() || has_jj);

        let ignore_matcher = if opts.ignore && found.is_none_or(|f| f.has_ignore) {
            create_gitignore(dir, dir, ".ignore")
        } else {
            Gitignore::empty()
        };

        let git_ignore_matcher = if opts.git_ignore && found.is_none_or(|f| f.has_git_ignore) {
            create_gitignore(dir, dir, ".gitignore")
        } else {
            Gitignore::empty()
        };

        let git_exclude_matcher = if opts.git_exclude && found.is_none_or(|f| f.has_git_dir) {
            resolve_git_commondir(dir, git_type).map_or_else(Gitignore::empty, |git_dir| {
                create_gitignore(dir, &git_dir, "info/exclude")
            })
        } else {
            Gitignore::empty()
        };

        Inner {
            compiled: Arc::clone(&self.inner.compiled),
            dir: dir.to_path_buf(),
            overrides: Arc::clone(&self.inner.overrides),
            parent: Some(Arc::clone(&self.inner)),
            is_absolute_parent: false,
            ignore_matcher,
            git_global_matcher: Arc::clone(&self.inner.git_global_matcher),
            git_ignore_matcher,
            git_exclude_matcher,
            has_git,
            any_git: false,
            any_matcher: false,
            opts,
        }
    }

    /// Whether the entry at `path`, a child of the directory these rules belong to, is skipped.
    /// `is_hidden` is only called when no rule decides.
    pub fn is_ignored(&self, path: &Path, is_dir: bool, is_hidden: impl FnOnce() -> bool) -> bool {
        let m = self.matched(path, is_dir);

        if m.is_none() {
            return self.inner.opts.hidden && is_hidden();
        }

        m.is_ignore()
    }

    fn matched(&self, path: &Path, is_dir: bool) -> Match<()> {
        let path = strip_prefix("./", path).unwrap_or(path);

        if !self.inner.overrides.is_empty() {
            let m = self.inner.overrides.matched(path, is_dir).map(|_| ());
            if !m.is_none() {
                return m;
            }
        }

        let opts = self.inner.opts;
        let global =
            (!opts.require_git || self.inner.any_git) && !self.inner.git_global_matcher.is_empty();
        let any_rules = opts.ignore || opts.git_global || opts.git_ignore || opts.git_exclude;

        // With every matcher empty, `matched_ignore` is `Match::None`.
        if any_rules && (self.inner.any_matcher || global) {
            let m = self.matched_ignore(path, is_dir);
            if m.is_ignore() || m.is_whitelist() {
                return m;
            }
        }

        Match::None
    }

    fn parents(&self) -> impl Iterator<Item = &Inner> {
        std::iter::successors(Some(self.inner.as_ref()), |inner| inner.parent.as_deref())
    }

    fn matched_ignore(&self, path: &Path, is_dir: bool) -> Match<()> {
        let mut m_ignore = Match::None;
        let mut m_gi = Match::None;
        let mut m_gi_exclude = Match::None;

        let any_git = !self.inner.opts.require_git || self.inner.any_git;
        let mut saw_git = false;

        let mut visit = |ig: &Inner, path: &Path| {
            if m_ignore.is_none() {
                m_ignore = ig.ignore_matcher.matched(path, is_dir).map(|_| ());
            }
            if any_git && !saw_git && m_gi.is_none() {
                m_gi = ig.git_ignore_matcher.matched(path, is_dir).map(|_| ());
            }
            if any_git && !saw_git && m_gi_exclude.is_none() {
                m_gi_exclude = ig.git_exclude_matcher.matched(path, is_dir).map(|_| ());
            }
            saw_git = saw_git || ig.has_git;
        };

        for ig in self.parents().take_while(|ig| !ig.is_absolute_parent) {
            visit(ig, path);
        }

        if self.inner.opts.parents
            && let Some(abs_parent_path) = self.absolute_base.as_deref()
        {
            let topmost = self
                .parents()
                .take_while(|ig| !ig.is_absolute_parent)
                .last();

            // Upstream rewrites `path` relative to the topmost walked directory and joins it onto
            // the canonical root. When that directory is the canonical root itself, as for every
            // walk `erd` starts, the rewrite is the identity, so the allocation is skipped.
            let rebased;
            let path = match topmost {
                Some(ig) if ig.dir.as_path() == abs_parent_path.as_path() => path,
                Some(ig) if ig.dir.as_path() == Path::new(".") => {
                    rebased = abs_parent_path.join(path);
                    rebased.as_path()
                },
                topmost => {
                    let relative = topmost.map_or(path, |ig| {
                        let without_dot_slash = strip_if_is_prefix("./", ig.dir.as_path());
                        let relative_base = strip_if_is_prefix(without_dot_slash, path);
                        strip_if_is_prefix("/", relative_base)
                    });
                    rebased = abs_parent_path.join(relative);
                    rebased.as_path()
                },
            };

            for ig in self.parents().skip_while(|ig| !ig.is_absolute_parent) {
                visit(ig, path);
            }
        }

        let m_global = if any_git {
            self.inner
                .git_global_matcher
                .matched(path, is_dir)
                .map(|_| ())
        } else {
            Match::None
        };

        m_ignore.or(m_gi).or(m_gi_exclude).or(m_global)
    }

    /// The directory these rules were built for, and those of its walked ancestors (not the
    /// ancestors of the root). Used for symlink loop detection.
    pub fn walked_dirs(&self) -> impl Iterator<Item = &Path> {
        self.parents()
            .take_while(|ig| !ig.is_absolute_parent)
            .map(|ig| ig.dir.as_path())
    }
}

/// A gitignore matcher for files below `dir` from the file `name` in `dir_for_ignorefile`. I/O
/// and syntax errors yield whatever rules were read.
fn create_gitignore(dir: &Path, dir_for_ignorefile: &Path, name: &str) -> Gitignore {
    let mut builder = GitignoreBuilder::new(dir);
    let path = dir_for_ignorefile.join(name);

    // Upstream probes for existence first except on Windows.
    if cfg!(windows) || path.exists() {
        let _ = builder.add(path);
    }

    builder.build().unwrap_or_else(|_| {
        GitignoreBuilder::new(dir)
            .build()
            .unwrap_or_else(|_| Gitignore::empty())
    })
}

/// The `GIT_COMMON_DIR` of the worktree at `dir`, which may hold `info/exclude`.
fn resolve_git_commondir(dir: &Path, git_type: Option<FileType>) -> Option<PathBuf> {
    let git_dir = dir.join(".git");

    if !git_type.is_some_and(|ft| ft.is_file()) {
        return Some(git_dir);
    }

    let first_line = |path: &Path| -> Option<String> {
        io::BufReader::new(File::open(path).ok()?)
            .lines()
            .next()?
            .ok()
    };

    let dot_git_line = first_line(&git_dir)?;
    let real_git_dir = PathBuf::from(dot_git_line.strip_prefix("gitdir: ")?);
    let commondir_line = first_line(&real_git_dir.join("commondir"))?;

    Some(if commondir_line.starts_with('.') {
        real_git_dir.join(commondir_line)
    } else {
        PathBuf::from(commondir_line)
    })
}

/// `path` without `prefix`, compared as bytes on Unix as upstream does.
fn strip_prefix<'a, P: AsRef<Path> + ?Sized>(prefix: &'a P, path: &'a Path) -> Option<&'a Path> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;

        let prefix = prefix.as_ref().as_os_str().as_bytes();
        let bytes = path.as_os_str().as_bytes();

        bytes
            .strip_prefix(prefix)
            .map(|rest| Path::new(OsStr::from_bytes(rest)))
    }

    #[cfg(not(unix))]
    {
        path.strip_prefix(prefix).ok()
    }
}

fn strip_if_is_prefix<'a, P: AsRef<Path> + ?Sized>(prefix: &'a P, path: &'a Path) -> &'a Path {
    strip_prefix(prefix, path).unwrap_or(path)
}

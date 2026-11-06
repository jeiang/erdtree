//! Per-directory ignore rules, adapted from `dir.rs` of the [`ignore`] crate (0.4.33, by Andrew
//! Gallant, dual-licensed Unlicense OR MIT), so that the custom walker in [`super`] applies the
//! exact `.gitignore`, `.ignore`, `.git/info/exclude`, global gitignore, override and hidden-file
//! precedence of [`ignore::WalkParallel`]. The rules are a persistent linked list: each directory
//! points at the rules of its parent, so workers share them without locks.
//!
//! Differences from upstream: the type matcher, explicit ignore files and custom ignore file names
//! are dropped (`erd` never sets them), and errors while reading ignore files are dropped, as `erd`
//! skipped them before. Matching only answers "ignored" or "not ignored".

use crate::fs::stat::Kind;
use ignore::{
    Match,
    gitignore::{Gitignore, GitignoreBuilder},
    overrides::Override,
};
use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    fs::File,
    io::{self, BufRead},
    path::{Path, PathBuf},
    sync::{Arc, OnceLock, RwLock, Weak},
};

/// Which kinds of rules apply. Of the options `erd` passes to [`ignore::WalkBuilder`], only
/// these vary: `.ignore` files, the ignore files of the root's ancestors and
/// `.git/info/exclude` always apply, and git rules only inside a git (or jj) repository.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Skip hidden entries unless a rule whitelists them.
    pub hidden: bool,
    /// Read `.gitignore` files and git's global gitignore.
    pub respect_ignore: bool,
}

impl Options {
    /// The defaults of [`ignore::WalkBuilder`] with `git_ignore` and `git_global` set to
    /// `respect_ignore`, as `erd` sets them; `ignore` and `git_exclude` stay on.
    pub const fn new(hidden: bool, respect_ignore: bool) -> Self {
        Self {
            hidden,
            respect_ignore,
        }
    }
}

/// Device and inode numbers of a directory.
pub type Ident = (u64, u64);

/// A directory descriptor to read ignore files relative to (on Linux), so that they are found
/// below paths longer than `PATH_MAX` too. The value is ignored elsewhere.
pub type DirFd = Option<i32>;

/// Ignore rules for one directory and, through `parent`, all of its ancestors.
#[derive(Clone, Debug)]
pub struct Rules {
    inner: Arc<Inner>,
}

/// What every directory of one walk shares.
#[derive(Debug)]
struct Common {
    /// Matchers already built for ancestor directories.
    compiled: RwLock<HashMap<OsString, Weak<Inner>>>,
    overrides: Override,
    /// Git's global gitignore, read when a repository is first seen: building it compiles a
    /// regular expression, which a walk outside any repository never needs.
    git_global: OnceLock<Gitignore>,
    /// Anchors the global gitignore; without it the global gitignore is not used.
    cwd: Option<PathBuf>,
}

impl Common {
    fn git_global(&self, opts: Options) -> &Gitignore {
        self.git_global.get_or_init(|| match &self.cwd {
            Some(cwd) if opts.respect_ignore => GitignoreBuilder::new(cwd).build_global().0,
            _ => Gitignore::empty(),
        })
    }
}

#[derive(Debug)]
struct Inner {
    common: Arc<Common>,
    dir: PathBuf,
    /// Device and inode of `dir`, for symlink loop detection; `None` if unknown.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    ident: Option<Ident>,
    parent: Option<Arc<Self>>,
    /// The canonical path given to [`Rules::add_parents`]; used to match against the ignore files
    /// of the root's ancestors.
    absolute_base: Option<Arc<PathBuf>>,
    is_absolute_parent: bool,
    ignore_matcher: Gitignore,
    git_ignore_matcher: Gitignore,
    git_exclude_matcher: Gitignore,
    /// Whether this directory contains `.git` (or `.jj`).
    has_git: bool,
    /// Whether this directory or an ancestor contains `.git` (or `.jj`).
    any_git: bool,
    /// Whether the `.ignore` matcher of this directory or an ancestor is non-empty.
    any_ignore: bool,
    /// Whether a gitignore or exclude matcher of this directory or an ancestor is non-empty.
    /// When neither this nor `any_ignore` is set (and there is no global gitignore to apply),
    /// no ignore file can decide, so matching skips the walk up the ancestors.
    any_git_matcher: bool,
    opts: Options,
}

impl Inner {
    /// Sets the derived flags from this directory's own matchers and its parent.
    fn derive(mut self) -> Self {
        let parent = self.parent.as_deref();
        self.any_git = self.has_git || parent.is_some_and(|p| p.any_git);
        self.any_ignore = !self.ignore_matcher.is_empty() || parent.is_some_and(|p| p.any_ignore);
        self.any_git_matcher = !self.git_ignore_matcher.is_empty()
            || !self.git_exclude_matcher.is_empty()
            || parent.is_some_and(|p| p.any_git_matcher);
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
    /// Every name present, so that every ignore file is probed on disk.
    pub const ALL: Self = Self {
        has_ignore: true,
        has_git_ignore: true,
        has_git_dir: true,
        has_jj_dir: true,
    };

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
        Self {
            inner: Arc::new(Inner {
                common: Arc::new(Common {
                    compiled: RwLock::new(HashMap::new()),
                    overrides,
                    git_global: OnceLock::new(),
                    cwd,
                }),
                dir: PathBuf::new(),
                #[cfg(any(target_os = "linux", target_os = "macos"))]
                ident: None,
                parent: None,
                absolute_base: None,
                is_absolute_parent: true,
                ignore_matcher: Gitignore::empty(),
                git_ignore_matcher: Gitignore::empty(),
                git_exclude_matcher: Gitignore::empty(),
                has_git: false,
                any_git: false,
                any_ignore: false,
                any_git_matcher: false,
                opts,
            }),
        }
    }

    /// Adds the rules of every ancestor of `path`. Only valid on [`Rules::root`].
    pub fn add_parents(&self, path: &Path) -> Self {
        let opts = self.inner.opts;

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
            let compiled = &self.inner.common.compiled;
            let lock = || {
                compiled
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
            };

            let prebuilt = lock()
                .get(parent.as_os_str())
                .and_then(Weak::upgrade)
                .filter(|inner| inner.absolute_base.as_deref() == Some(&*absolute_base));
            if let Some(prebuilt) = prebuilt {
                rules = Self { inner: prebuilt };
                continue;
            }

            let mut inner = rules.child_inner(parent, Found::ALL, None, None);
            inner.is_absolute_parent = true;
            inner.absolute_base = Some(Arc::clone(&absolute_base));
            // `child_inner` already looked for `.git` and `.jj`. As upstream does, an ancestor
            // of the root only counts as a repository when `.gitignore` files are read, though
            // walked directories count either way.
            inner.has_git = opts.respect_ignore && inner.has_git;

            let inner = Arc::new(inner.derive());
            lock().insert(parent.as_os_str().to_os_string(), Arc::downgrade(&inner));
            rules = Self { inner };
        }

        rules
    }

    /// The rules of child directory `dir`, given which ignore files its listing contains (with
    /// [`Found::ALL`] they are probed on disk). A directory without ignore files shares the
    /// rules of its parent, unless `own_rules` asks for rules of its own (symlink loop detection
    /// lists the walked directories) or the parent is an ancestor of the root. `ident`
    /// identifies the directory for loop detection; `fd` is its open descriptor.
    pub fn add_child(
        &self,
        dir: &Path,
        found: Found,
        own_rules: bool,
        ident: Option<Ident>,
        fd: DirFd,
    ) -> Self {
        let no_files =
            !found.has_ignore && !found.has_git_ignore && !found.has_git_dir && !found.has_jj_dir;

        if no_files && !own_rules && !self.inner.is_absolute_parent {
            return self.clone();
        }

        Self {
            inner: Arc::new(self.child_inner(dir, found, ident, fd).derive()),
        }
    }

    fn child_inner(&self, dir: &Path, found: Found, ident: Option<Ident>, fd: DirFd) -> Inner {
        // Loops are found by path elsewhere.
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let _ = ident;

        let opts = self.inner.opts;

        let git_type = if found.has_git_dir {
            kind_in(dir, fd, ".git")
        } else {
            None
        };
        let has_jj = found.has_jj_dir && kind_in(dir, fd, ".jj").is_some();
        let has_git = git_type.is_some() || has_jj;

        let ignore_matcher = if found.has_ignore {
            create_gitignore(dir, || open_in(dir, fd, ".ignore"))
        } else {
            Gitignore::empty()
        };

        let git_ignore_matcher = if opts.respect_ignore && found.has_git_ignore {
            create_gitignore(dir, || open_in(dir, fd, ".gitignore"))
        } else {
            Gitignore::empty()
        };

        let git_exclude_matcher = if found.has_git_dir {
            if git_type.is_some_and(Kind::is_file) {
                resolve_git_commondir(dir, fd).map_or_else(Gitignore::empty, |git_dir| {
                    create_gitignore(dir, || File::open(git_dir.join("info/exclude")))
                })
            } else {
                create_gitignore(dir, || open_in(dir, fd, ".git/info/exclude"))
            }
        } else {
            Gitignore::empty()
        };

        Inner {
            common: Arc::clone(&self.inner.common),
            dir: dir.to_path_buf(),
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            ident,
            parent: Some(Arc::clone(&self.inner)),
            absolute_base: self.inner.absolute_base.clone(),
            is_absolute_parent: false,
            ignore_matcher,
            git_ignore_matcher,
            git_exclude_matcher,
            has_git,
            any_git: false,
            any_ignore: false,
            any_git_matcher: false,
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

        if !self.inner.common.overrides.is_empty() {
            let m = self
                .inner
                .common
                .overrides
                .matched(path, is_dir)
                .map(|_| ());
            if !m.is_none() {
                return m;
            }
        }

        let opts = self.inner.opts;

        // With every matcher that can apply empty, `matched_ignore` is `Match::None`. Git
        // matchers only apply inside a repository.
        if self.inner.any_ignore
            || self.inner.any_git
                && (self.inner.any_git_matcher || !self.inner.common.git_global(opts).is_empty())
        {
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

        let any_git = self.inner.any_git;
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

        if let Some(abs_parent_path) = self.inner.absolute_base.as_deref() {
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
                .common
                .git_global(self.inner.opts)
                .matched(path, is_dir)
                .map(|_| ())
        } else {
            Match::None
        };

        m_ignore.or(m_gi).or(m_gi_exclude).or(m_global)
    }

    /// The directory these rules were built for, and those of its walked ancestors (not the
    /// ancestors of the root). Used for symlink loop detection.
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub fn walked_dirs(&self) -> impl Iterator<Item = &Path> {
        self.parents()
            .take_while(|ig| !ig.is_absolute_parent)
            .map(|ig| ig.dir.as_path())
    }

    /// Whether `ident` is that of the directory these rules were built for, or of one of its
    /// walked ancestors (not the ancestors of the root). Used for symlink loop detection; unlike
    /// opening the directories by path, it works at any depth.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub fn is_walked(&self, ident: Ident) -> bool {
        self.parents()
            .take_while(|ig| !ig.is_absolute_parent)
            .any(|ig| ig.ident == Some(ident))
    }
}

/// Opens the file `name` (a relative path) of `dir`: relative to its descriptor `fd` if there is
/// one, else by path.
fn open_in(dir: &Path, fd: DirFd, name: &str) -> io::Result<File> {
    #[cfg(target_os = "macos")]
    let fd = fd.filter(|_| crate::fs::at::fits_path_max(dir.as_os_str().len() + 1 + name.len()));

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if let Some(dir) = fd {
        use crate::fs::at::{self, At};

        return at::with_cstr(name.as_bytes(), |name| at::open_file(At { dir, name }));
    }

    let _ = fd;
    File::open(dir.join(name))
}

/// The type of `name` in `dir`, following a symlink; `None` if it does not exist.
fn kind_in(dir: &Path, fd: DirFd, name: &str) -> Option<Kind> {
    #[cfg(target_os = "macos")]
    let fd = fd.filter(|_| crate::fs::at::fits_path_max(dir.as_os_str().len() + 1 + name.len()));

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if let Some(dir) = fd {
        use crate::fs::{
            at::{self, At},
            stat::Fields,
        };

        return at::with_cstr(name.as_bytes(), |name| {
            at::stat_at(At { dir, name }, true, Fields::default())
        })
        .ok()
        .map(|stat| stat.kind);
    }

    let _ = fd;
    dir.join(name)
        .metadata()
        .ok()
        .map(|md| Kind::from(md.file_type()))
}

/// A gitignore matcher for files below `dir` from the file `open` opens, if it exists. Like
/// [`GitignoreBuilder::add`], whose lines it adds, it yields whatever rules were read before an
/// I/O or syntax error.
fn create_gitignore(dir: &Path, open: impl FnOnce() -> io::Result<File>) -> Gitignore {
    let mut builder = GitignoreBuilder::new(dir);

    if let Ok(file) = open() {
        for (i, line) in io::BufReader::new(file).lines().enumerate() {
            let Ok(line) = line else {
                break;
            };

            // Match Git's handling of files that begin with the Unicode BOM.
            let line = if i == 0 {
                line.trim_start_matches('\u{feff}')
            } else {
                &line
            };

            let _ = builder.add_line(None, line);
        }
    }

    builder.build().unwrap_or_else(|_| {
        GitignoreBuilder::new(dir)
            .build()
            .unwrap_or_else(|_| Gitignore::empty())
    })
}

/// The `GIT_COMMON_DIR` of the worktree at `dir`, whose `.git` is a file, which may hold
/// `info/exclude`.
fn resolve_git_commondir(dir: &Path, fd: DirFd) -> Option<PathBuf> {
    let first_line = |file: io::Result<File>| -> Option<String> {
        io::BufReader::new(file.ok()?).lines().next()?.ok()
    };

    let dot_git_line = first_line(open_in(dir, fd, ".git"))?;
    let real_git_dir = PathBuf::from(dot_git_line.strip_prefix("gitdir: ")?);
    let commondir_line = first_line(File::open(real_git_dir.join("commondir")))?;

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

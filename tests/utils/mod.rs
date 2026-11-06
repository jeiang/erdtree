//! The one way integration tests run `erd`.
#![allow(
    dead_code,
    reason = "every test target compiles this module and uses a different part of it"
)]

use std::{
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
};
use tempfile::TempDir;

/// An `erd` run isolated from the machine running the tests: `HOME`, `XDG_CONFIG_HOME`,
/// `APPDATA` and `USERPROFILE` point into an empty directory of its own, and git's global and
/// system config files are empty. The `ignore` crate looks for the global gitignore through
/// `GIT_CONFIG_GLOBAL`, `$HOME/.gitconfig`, `$XDG_CONFIG_HOME/git/config`, `GIT_CONFIG_SYSTEM`
/// and finally `$XDG_CONFIG_HOME/git/ignore`, so none of the user's applies; tests may write
/// their own below [`Erd::home`]. `NO_COLOR` and `LS_COLORS` are unset unless given to
/// [`Erd::env`].
///
/// Arguments start with `--threads 1`, and unless a config is used also with
/// `--disk-usage logical --sort name --no-config`; later arguments override them.
pub struct Erd {
    home: TempDir,
    threads: usize,
    config: Config,
    args: Vec<OsString>,
    envs: Vec<(OsString, OsString)>,
}

enum Config {
    Off,
    File(PathBuf),
    Discover,
}

impl Erd {
    pub fn new() -> Self {
        Self {
            home: TempDir::new().unwrap(),
            threads: 1,
            config: Config::Off,
            args: vec![],
            envs: vec![],
        }
    }

    pub fn arg(mut self, arg: impl AsRef<OsStr>) -> Self {
        self.args.push(arg.as_ref().to_owned());
        self
    }

    pub fn args<I: IntoIterator<Item: AsRef<OsStr>>>(mut self, args: I) -> Self {
        self.args
            .extend(args.into_iter().map(|a| a.as_ref().to_owned()));
        self
    }

    pub fn threads(mut self, threads: usize) -> Self {
        self.threads = threads;
        self
    }

    pub fn env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        self.envs
            .push((key.as_ref().to_owned(), value.as_ref().to_owned()));
        self
    }

    /// Uses `toml` as the config file, through `ERDTREE_TOML_PATH`.
    pub fn config(mut self, toml: &str) -> Self {
        let path = self.home.path().join("config-under-test.toml");
        fs::write(&path, toml).unwrap();
        self.config = Config::File(path);
        self
    }

    /// Lets `erd` look for its config in the isolated [`home`](Self::home).
    pub fn discover_config(mut self) -> Self {
        self.config = Config::Discover;
        self
    }

    /// The isolated home directory; `XDG_CONFIG_HOME` is its `xdg` and `APPDATA` its `appdata`.
    pub fn home(&self) -> &Path {
        self.home.path()
    }

    /// The command to run, and the home directory it needs to outlive.
    pub fn command(self) -> (Command, TempDir) {
        let home = self.home.path();
        let empty = home.join("empty-gitconfig");
        fs::write(&empty, "").unwrap();

        let mut cmd = Command::new(env!("CARGO_BIN_EXE_erd"));
        cmd.env_remove("NO_COLOR")
            .env_remove("LS_COLORS")
            .env_remove("ERDTREE_TOML_PATH")
            .env("HOME", home)
            .env("USERPROFILE", home)
            .env("XDG_CONFIG_HOME", home.join("xdg"))
            .env("APPDATA", home.join("appdata"))
            .env("GIT_CONFIG_GLOBAL", &empty)
            .env("GIT_CONFIG_SYSTEM", &empty)
            .args(["--threads", &self.threads.to_string()]);

        match &self.config {
            Config::Off => {
                cmd.args(["--disk-usage", "logical", "--sort", "name", "--no-config"]);
            },
            Config::File(path) => {
                cmd.env("ERDTREE_TOML_PATH", path);
            },
            Config::Discover => {},
        }

        cmd.args(&self.args).envs(self.envs).stdin(Stdio::null());
        (cmd, self.home)
    }

    pub fn run(self) -> Run {
        let (mut cmd, _home) = self.command();
        let out = cmd.output().unwrap();
        Run {
            status: out.status,
            stdout: out.stdout,
            stderr: String::from_utf8(out.stderr).unwrap(),
        }
    }

    /// Runs `erd`, which must succeed.
    pub fn ok(self) -> Run {
        let run = self.run();
        assert!(run.status.success(), "{}: {}", run.status, run.stderr);
        run
    }

    /// Runs `erd`, which must exit with `code` and print nothing; returns its stderr.
    pub fn fails(self, code: i32) -> String {
        let run = self.run();
        assert_eq!(run.code(), Some(code), "{}", run.stderr);
        assert_eq!(run.raw(), "", "{}", run.stderr);
        run.stderr
    }
}

pub struct Run {
    pub status: ExitStatus,
    stdout: Vec<u8>,
    stderr: String,
}

impl Run {
    pub fn code(&self) -> Option<i32> {
        self.status.code()
    }

    /// Stdout with escape sequences.
    pub fn raw(&self) -> String {
        String::from_utf8(self.stdout.clone()).unwrap()
    }

    /// Stdout without escape sequences, with `/` separators on Windows too.
    pub fn stdout(&self) -> String {
        let out = String::from_utf8(strip_ansi_escapes::strip(&self.stdout)).unwrap();

        #[cfg(windows)]
        let out = out.replace('\\', "/");

        out
    }

    /// [`stdout`](Self::stdout) without leading and trailing whitespace.
    pub fn trimmed(&self) -> String {
        self.stdout().trim().to_owned()
    }

    pub fn stderr(&self) -> &str {
        &self.stderr
    }
}

/// Stdout of a successful `erd args`, trimmed and without escape sequences.
pub fn run_cmd(args: &[&str]) -> String {
    Erd::new().args(args).ok().trimmed()
}

/// [`run_cmd`] keeping leading and trailing whitespace.
pub fn run_cmd_untrimmed(args: &[&str]) -> String {
    Erd::new().args(args).ok().stdout()
}

/// Paths of the entries below `root` in the order `erd --layout iflat` lists them.
pub fn paths(root: impl AsRef<OsStr>, args: &[&str]) -> Vec<String> {
    Erd::new()
        .args(["--layout", "iflat", "--suppress-size", "--no-report"])
        .args(args)
        .arg(root)
        .ok()
        .stdout()
        .lines()
        .skip(1)
        .map(str::to_owned)
        .collect()
}

/// The rows of `--suppress-size` output above the summary, without tree prefixes, root included.
pub fn rows(out: &str) -> Vec<&str> {
    out.lines()
        .take_while(|l| !l.is_empty())
        .map(|l| l.trim_start_matches([' ', '│', '├', '└', '┌', '─']))
        .collect()
}

/// Whether `out`, printed with `-C force`, draws a branch in red, the color of the link theme
/// of `--follow`. The target of a link (`→ target`) is red in every theme, so it does not count.
pub fn in_link_theme(out: &str) -> bool {
    out.match_indices("\x1b[31m")
        .any(|(at, red)| out[at + red.len()..].starts_with(['┌', '├', '└', '│']))
}

/// `t/` holding `a/b/c.txt` (3 bytes), `a/d.txt` (2 bytes) and `e.txt` (4 bytes): three levels,
/// so that `--level 2` and `--traversal-depth 2` cut something. Returns the tempdir and `t`.
pub fn three_levels() -> (TempDir, String) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("t");
    fs::create_dir_all(root.join("a/b")).unwrap();
    fs::write(root.join("a/b/c.txt"), "ccc").unwrap();
    fs::write(root.join("a/d.txt"), "dd").unwrap();
    fs::write(root.join("e.txt"), "eeee").unwrap();
    let root = root.to_str().unwrap().to_owned();
    (tmp, root)
}

/// Creates a chain of `levels` directories `name` below `root` without ever using a path longer
/// than one name, so the chain can be deeper than `PATH_MAX`. Calls `each` with every directory
/// before creating the next one in it, and returns the deepest.
#[cfg(target_os = "linux")]
pub fn chain(
    root: &Path,
    name: &str,
    levels: usize,
    mut each: impl FnMut(std::os::fd::RawFd),
) -> fs::File {
    use std::os::fd::{AsRawFd, FromRawFd};

    let mut dir = fs::File::open(root).unwrap();
    let name = std::ffi::CString::new(name).unwrap();

    for _ in 0..levels {
        each(dir.as_raw_fd());
        // SAFETY: `dir` is an open descriptor and `name` is NUL-terminated.
        unsafe {
            assert_eq!(libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), 0o755), 0);
            let fd = libc::openat(dir.as_raw_fd(), name.as_ptr(), libc::O_RDONLY);
            assert!(fd >= 0);
            dir = fs::File::from_raw_fd(fd);
        }
    }
    dir
}

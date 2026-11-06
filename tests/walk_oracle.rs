//! Differential tests of the walk: the entries `erd` shows must be exactly those that
//! `ignore::WalkBuilder` yields when configured as `erd` configured it before it had its own
//! walker. Fixtures are a handcrafted tree and seeded random trees with random ignore files.

use ignore::{WalkBuilder, overrides::OverrideBuilder};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};
use tempfile::TempDir;

mod utils;

/// Walk options shared by both sides.
#[derive(Clone, Copy, Debug, Default)]
struct Opts {
    hidden: bool,
    no_ignore: bool,
    follow: bool,
    no_git: bool,
    exclude: Option<&'static str>,
    traversal_depth: Option<usize>,
}

impl Opts {
    fn args(self) -> Vec<String> {
        let mut args = vec![];
        if self.hidden {
            args.push("--hidden".into());
        }
        if self.no_ignore {
            args.push("--no-ignore".into());
        }
        if self.follow {
            args.push("--follow".into());
        }
        if self.no_git {
            args.push("--no-git".into());
        }
        if let Some(glob) = self.exclude {
            args.extend(["--exclude".into(), glob.into()]);
        }
        if let Some(depth) = self.traversal_depth {
            args.extend(["--traversal-depth".into(), depth.to_string()]);
        }
        args
    }
}

const OPTION_SETS: &[Opts] = &[
    Opts {
        hidden: false,
        no_ignore: false,
        follow: false,
        no_git: false,
        exclude: None,
        traversal_depth: None,
    },
    Opts {
        hidden: true,
        no_ignore: false,
        follow: false,
        no_git: false,
        exclude: None,
        traversal_depth: None,
    },
    Opts {
        hidden: false,
        no_ignore: true,
        follow: false,
        no_git: false,
        exclude: None,
        traversal_depth: None,
    },
    Opts {
        hidden: true,
        no_ignore: true,
        follow: false,
        no_git: true,
        exclude: Some("*.rs"),
        traversal_depth: None,
    },
    Opts {
        hidden: true,
        no_ignore: false,
        follow: true,
        no_git: false,
        exclude: None,
        traversal_depth: None,
    },
    Opts {
        hidden: false,
        no_ignore: false,
        follow: false,
        no_git: false,
        exclude: Some("d1"),
        traversal_depth: Some(2),
    },
];

/// Relative paths (with `/`) of the entries below `root` that `erd` shows.
fn erd_entries(root: &Path, opts: Opts) -> BTreeSet<String> {
    let mut args = vec![
        "--layout".to_string(),
        "iflat".into(),
        "--suppress-size".into(),
        "--no-report".into(),
    ];
    args.extend(opts.args());
    args.push(root.display().to_string());

    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = utils::run_cmd_untrimmed(&args);

    // The first line is the root.
    out.lines().skip(1).map(str::to_string).collect()
}

/// Relative paths of the entries below `root` that `ignore` yields.
fn oracle_entries(root: &Path, opts: Opts) -> BTreeSet<String> {
    let root = fs::canonicalize(root).unwrap();
    let mut overrides = OverrideBuilder::new(&root);

    if opts.no_git {
        overrides.add("!.git").unwrap();
    }
    if let Some(glob) = opts.exclude {
        overrides.add(&format!("!{glob}")).unwrap();
    }

    let mut builder = WalkBuilder::new(&root);
    builder
        .follow_links(opts.follow)
        .git_ignore(!opts.no_ignore)
        .git_global(!opts.no_ignore)
        .hidden(!opts.hidden)
        .overrides(overrides.build().unwrap())
        .max_depth(opts.traversal_depth);

    builder
        .build()
        .filter_map(Result::ok)
        .filter(|entry| entry.depth() > 0)
        .map(|entry| {
            let rel = entry.path().strip_prefix(&root).unwrap();
            rel.components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        })
        .collect()
}

fn assert_same_entries(root: &Path) {
    for &opts in OPTION_SETS {
        // Symlinks are only created on Unix.
        if opts.follow && cfg!(not(unix)) {
            continue;
        }

        let erd = erd_entries(root, opts);
        let oracle = oracle_entries(root, opts);

        assert_eq!(
            erd,
            oracle,
            "{opts:?} on {}\nonly erd: {:?}\nonly ignore: {:?}",
            root.display(),
            erd.difference(&oracle).collect::<Vec<_>>(),
            oracle.difference(&erd).collect::<Vec<_>>(),
        );
    }
}

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

#[test]
fn handcrafted_ignore_rules() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    for (path, contents) in [
        (".git/info/exclude", "secret.txt\n"),
        (
            ".gitignore",
            "target\n*.log\n!keep.log\nbuild/\n/node_modules\n",
        ),
        ("a.log", ""),
        ("keep.log", ""),
        ("secret.txt", ""),
        ("readme.md", ""),
        ("target", ""),
        ("build/out.bin", ""),
        ("node_modules/pkg/index.js", ""),
        ("src/.gitignore", "*.tmp\n"),
        ("src/main.rs", ""),
        ("src/x.tmp", ""),
        ("src/deep/.gitignore", "!important.tmp\n"),
        ("src/deep/important.tmp", ""),
        ("src/deep/other.tmp", ""),
        ("src/deep/node_modules/kept", ""),
        ("sub/.git/HEAD", ""),
        ("sub/.gitignore", "*.o\n"),
        ("sub/lib/m.o", ""),
        ("sub/lib/m.c", ""),
        ("docs/.ignore", "draft*\n"),
        ("docs/draft.md", ""),
        ("docs/guide.md", ""),
        (".hidden_dir/inner/f", ""),
        (".hidden_file", ""),
        ("d1/d2/d3/deep.rs", ""),
    ] {
        write(&root.join(path), contents);
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        fs::create_dir_all(root.join("links")).unwrap();
        symlink("../src", root.join("links/to_dir")).unwrap();
        symlink("../readme.md", root.join("links/to_file")).unwrap();
        symlink("nowhere", root.join("links/broken")).unwrap();
        symlink("..", root.join("links/loop")).unwrap();
    }

    assert_same_entries(root);
}

/// Repositories marked by a `.git` file (a worktree, whose `info/exclude` is in the common git
/// directory) or by `.jj`, and a root inside a repository, whose ancestors' rules apply.
#[test]
fn repository_markers() {
    let dir = TempDir::new().unwrap();
    let top = fs::canonicalize(dir.path()).unwrap();

    for (path, contents) in [
        ("real/info/exclude", "wt_excluded\n"),
        ("real/worktrees/wt/commondir", "../..\n"),
        ("tree/wt/.gitignore", "*.o\n"),
        ("tree/wt/wt_excluded", ""),
        ("tree/wt/a.o", ""),
        ("tree/wt/kept", ""),
        ("tree/jj/.jj/repo", ""),
        ("tree/jj/.gitignore", "*.o\n"),
        ("tree/jj/a.o", ""),
        ("tree/jj/kept", ""),
        ("tree/plain/.gitignore", "*.o\n"),
        ("tree/plain/a.o", ""),
        ("repo/.git/info/exclude", "excluded\n"),
        ("repo/.gitignore", "parent_ignored\n"),
        ("repo/root/excluded", ""),
        ("repo/root/parent_ignored", ""),
        ("repo/root/kept", ""),
    ] {
        write(&top.join(path), contents);
    }
    let git_dir = top.join("real/worktrees/wt");
    write(
        &top.join("tree/wt/.git"),
        &format!("gitdir: {}\n", git_dir.display()),
    );

    assert_same_entries(&top.join("tree"));
    assert_same_entries(&top.join("repo/root"));
}

/// A small deterministic generator (64-bit LCG), so failures reproduce.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next()).unwrap() % n
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
}

const NAMES: &[&str] = &[
    "a", "b", "ab", "x1", "build", "deep", "keep.log", "a.log", "m.rs", "n.tmp", ".h", ".git",
    "top", "d1", "sub",
];

const PATTERNS: &[&str] = &[
    "*.log",
    "!keep.log",
    "build/",
    "/top",
    "a*",
    "**/deep",
    "x?",
    "*.tmp",
    "!b*",
    "sub/",
    "d1/**",
    "# comment",
    "!*.rs",
    "*",
    "!a",
    ".h",
    "!.h",
];

fn random_tree(rng: &mut Rng, dir: &Path, depth: usize) {
    fs::create_dir_all(dir).unwrap();

    if rng.below(3) == 0 {
        let rules: Vec<&str> = (0..=rng.below(4)).map(|_| rng.pick(PATTERNS)).collect();
        let file = if rng.below(2) == 0 {
            ".gitignore"
        } else {
            ".ignore"
        };
        write(&dir.join(file), &(rules.join("\n") + "\n"));
    }

    for _ in 0..=rng.below(5) {
        let name = rng.pick(NAMES);
        let path: PathBuf = dir.join(name);

        if path.exists() {
            continue;
        }

        if depth < 4 && rng.below(3) == 0 {
            random_tree(rng, &path, depth + 1);
        } else if name == ".git" {
            fs::create_dir_all(&path).unwrap();
        } else {
            write(&path, "");
        }
    }
}

#[test]
fn random_ignore_rules() {
    for seed in 1..=12 {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("root");
        let mut rng = Rng(seed);

        // Half of the trees are git repositories, which enables .gitignore rules.
        if seed % 2 == 0 {
            fs::create_dir_all(root.join(".git")).unwrap();
        }

        random_tree(&mut rng, &root, 0);
        assert_same_entries(&root);
    }
}

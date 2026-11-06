use std::{
    collections::HashMap,
    ffi::OsStr,
    hash::{BuildHasherDefault, Hasher},
    sync::LazyLock,
};

pub mod fs;

/// Multiplicative hasher for the icon tables. Their keys are short file names and extensions
/// that come from the user's own tree, so `SipHash`'s resistance to crafted collisions would
/// only cost time on every row.
#[derive(Default)]
struct FastHasher(u64);

impl Hasher for FastHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0.rotate_left(5) ^ u64::from(b)).wrapping_mul(0x517c_c1b7_2722_0a95);
        }
    }
}

type Map<K, V> = HashMap<K, V, BuildHasherDefault<FastHasher>>;

/// Default fallback icon: its 8-bit color code and glyph.
const DEFAULT_ICON: (u8, &str) = (66, "\u{f15b}");

/// Where the icon of a whole file name ranks against the icon of its extension.
#[derive(Clone, Copy)]
enum Rank {
    /// Before the extension, painted in this 8-bit color: `test.js` would otherwise get the icon
    /// of `js`.
    BeforeExt(u8),

    /// After the extension, painted like the file as `LS_COLORS` styles it.
    AfterExt,
}

/// Icons of whole file names. An entry ranked after the extension only applies to names whose
/// extension has no icon.
///
/// Dev icons sourced from [`exa`](https://github.com/ogham/exa/blob/master/src/output/icons.rs)
const NAME_ICONS: [(&str, Rank, &str); 41] = [
    (".Trash", Rank::AfterExt, "\u{f1f8}"),
    (".atom", Rank::AfterExt, "\u{e764}"),
    (".bashprofile", Rank::AfterExt, "\u{e615}"),
    (".bashrc", Rank::AfterExt, "\u{f489}"),
    (".git", Rank::AfterExt, "\u{f1d3}"),
    (".gitattributes", Rank::AfterExt, "\u{f1d3}"),
    (".gitconfig", Rank::AfterExt, "\u{f1d3}"),
    (".github", Rank::AfterExt, "\u{f408}"),
    (".gitignore", Rank::AfterExt, "\u{f1d3}"),
    (".gitmodules", Rank::AfterExt, "\u{f1d3}"),
    (".rvm", Rank::AfterExt, "\u{e21e}"),
    (".vimrc", Rank::AfterExt, "\u{e62b}"),
    (".vscode", Rank::AfterExt, "\u{e70c}"),
    (".zshrc", Rank::AfterExt, "\u{f489}"),
    ("bin", Rank::AfterExt, "\u{e5fc}"),
    ("config", Rank::AfterExt, "\u{e5fc}"),
    ("Dockerfile", Rank::AfterExt, "\u{f308}"),
    (".DS_Store", Rank::AfterExt, "\u{f179}"),
    ("gitignore_global", Rank::AfterExt, "\u{f1d3}"),
    ("go.mod", Rank::AfterExt, "\u{e626}"),
    ("go.sum", Rank::AfterExt, "\u{e626}"),
    ("gradle", Rank::AfterExt, "\u{e256}"),
    ("gruntfile.ls", Rank::AfterExt, "\u{e611}"),
    ("gulpfile.ls", Rank::AfterExt, "\u{e610}"),
    ("hidden", Rank::AfterExt, "\u{f023}"),
    ("include", Rank::AfterExt, "\u{e5fc}"),
    ("lib", Rank::AfterExt, "\u{f121}"),
    ("license", Rank::AfterExt, "\u{e60a}"),
    ("LICENSE", Rank::AfterExt, "\u{e60a}"),
    ("licence", Rank::AfterExt, "\u{e60a}"),
    ("LICENCE", Rank::AfterExt, "\u{e60a}"),
    ("localized", Rank::AfterExt, "\u{f179}"),
    ("node_modules", Rank::AfterExt, "\u{e718}"),
    ("npmignore", Rank::AfterExt, "\u{e71e}"),
    ("PKGBUILD", Rank::AfterExt, "\u{f303}"),
    ("rubydoc", Rank::AfterExt, "\u{e73b}"),
    ("config.ru", Rank::BeforeExt(52), "\u{e791}"),
    ("GNUmakefile", Rank::BeforeExt(66), "\u{e779}"),
    ("Makefile", Rank::BeforeExt(66), "\u{e779}"),
    ("makefile", Rank::BeforeExt(66), "\u{e779}"),
    ("test.js", Rank::BeforeExt(173), "\u{e60c}"),
];

static NAME_ICON_MAP: LazyLock<Map<&OsStr, (Rank, &str)>> = LazyLock::new(|| {
    NAME_ICONS
        .iter()
        .map(|&(name, rank, icon)| (OsStr::new(name), (rank, icon)))
        .collect()
});

/// Icons of file extensions with their 8-bit color codes.
///
/// Dev icons and their color palettes sourced from [`nvim-web-devicons`](https://github.com/nvim-tree/nvim-web-devicons/blob/master/lua/nvim-web-devicons.lua).
const EXT_ICONS: [(&str, (u8, &str)); 195] = [
    ("ai", (185, "\u{e7b4}")),
    ("awk", (59, "\u{e795}")),
    ("bash", (113, "\u{e795}")),
    ("bat", (154, "\u{e615}")),
    ("bmp", (140, "\u{e60d}")),
    ("cbl", (25, "\u{2699}")),
    ("c++", (204, "\u{e61d}")),
    ("c", (75, "\u{e61e}")),
    ("cc", (204, "\u{e61d}")),
    ("cfg", (231, "\u{e7a3}")),
    ("cljc", (107, "\u{e768}")),
    ("clj", (107, "\u{e768}")),
    ("cljd", (67, "\u{e76a}")),
    ("cljs", (67, "\u{e76a}")),
    ("cmake", (66, "\u{e615}")),
    ("cob", (25, "\u{2699}")),
    ("cobol", (25, "\u{2699}")),
    ("coffee", (185, "\u{e61b}")),
    ("conf", (66, "\u{e615}")),
    ("cp", (67, "\u{e61d}")),
    ("cpp", (67, "\u{e61d}")),
    ("cpy", (25, "\u{2699}")),
    ("cr", (16, "\u{e24f}")),
    ("cs", (58, "\u{f031b}")),
    ("csh", (59, "\u{e795}")),
    ("cson", (185, "\u{e60b}")),
    ("css", (39, "\u{e749}")),
    ("csv", (113, "\u{f0219}")),
    ("cxx", (67, "\u{e61d}")),
    ("dart", (25, "\u{e798}")),
    ("db", (188, "\u{e706}")),
    ("d", (64, "\u{e7af}")),
    ("desktop", (60, "\u{f108}")),
    ("diff", (59, "\u{e728}")),
    ("doc", (25, "\u{f022c}")),
    ("drl", (217, "\u{e28c}")),
    ("dropbox", (27, "\u{e707}")),
    ("dump", (188, "\u{e706}")),
    ("edn", (67, "\u{e76a}")),
    ("eex", (140, "\u{e62d}")),
    ("ejs", (185, "\u{e60e}")),
    ("elm", (67, "\u{e62c}")),
    ("epp", (255, "\u{e631}")),
    ("erb", (52, "\u{e60e}")),
    ("erl", (132, "\u{e7b1}")),
    ("ex", (140, "\u{e62d}")),
    ("exs", (140, "\u{e62d}")),
    ("f#", (67, "\u{e7a7}")),
    ("fish", (59, "\u{e795}")),
    ("fnl", (230, "\u{1f31c}")),
    ("fs", (67, "\u{e7a7}")),
    ("fsi", (67, "\u{e7a7}")),
    ("fsscript", (67, "\u{e7a7}")),
    ("fsx", (67, "\u{e7a7}")),
    ("gd", (66, "\u{e615}")),
    ("gemspec", (52, "\u{e791}")),
    ("gif", (140, "\u{e60d}")),
    ("git", (202, "\u{e702}")),
    ("glb", (215, "\u{f1b2}")),
    ("go", (67, "\u{e627}")),
    ("godot", (66, "\u{e7a3}")),
    ("gql", (199, "\u{f20e}")),
    ("graphql", (199, "\u{f20e}")),
    ("haml", (188, "\u{e60e}")),
    ("hbs", (208, "\u{e60f}")),
    ("h", (140, "\u{f0fd}")),
    ("heex", (140, "\u{e62d}")),
    ("hh", (140, "\u{f0fd}")),
    ("hpp", (140, "\u{f0fd}")),
    ("hrl", (132, "\u{e7b1}")),
    ("hs", (140, "\u{e61f}")),
    ("htm", (166, "\u{e60e}")),
    ("html", (202, "\u{e736}")),
    ("hxx", (140, "\u{f0fd}")),
    ("ico", (185, "\u{e60d}")),
    ("import", (231, "\u{f0c6}")),
    ("ini", (66, "\u{e615}")),
    ("java", (167, "\u{e738}")),
    ("jl", (133, "\u{e624}")),
    ("jpeg", (140, "\u{e60d}")),
    ("jpg", (140, "\u{e60d}")),
    ("js", (185, "\u{e60c}")),
    ("json5", (185, "\u{f0626}")),
    ("json", (185, "\u{e60b}")),
    ("jsx", (67, "\u{e625}")),
    ("ksh", (59, "\u{e795}")),
    ("kt", (99, "\u{e634}")),
    ("kts", (99, "\u{e634}")),
    ("leex", (140, "\u{e62d}")),
    ("less", (60, "\u{e614}")),
    ("lhs", (140, "\u{e61f}")),
    ("license", (185, "\u{e60a}")),
    ("licence", (185, "\u{e60a}")),
    ("lock", (250, "\u{f13e}")),
    ("log", (255, "\u{f0331}")),
    ("lua", (74, "\u{e620}")),
    ("luau", (74, "\u{e620}")),
    ("markdown", (67, "\u{e609}")),
    ("material", (132, "\u{f0509}")),
    ("md", (255, "\u{f48a}")),
    ("mdx", (67, "\u{f48a}")),
    ("mint", (108, "\u{f032a}")),
    ("mjs", (221, "\u{e60c}")),
    ("mk", (66, "\u{e779}")),
    ("ml", (173, "\u{3bb}")),
    ("mli", (173, "\u{3bb}")),
    ("mo", (99, "\u{221e}")),
    ("mustache", (173, "\u{e60f}")),
    ("nim", (220, "\u{e677}")),
    ("nix", (110, "\u{f313}")),
    ("opus", (208, "\u{f0223}")),
    ("otf", (231, "\u{f031}")),
    ("pck", (66, "\u{f487}")),
    ("pdf", (124, "\u{f0226}")),
    ("php", (140, "\u{e608}")),
    ("pl", (67, "\u{e769}")),
    ("pm", (67, "\u{e769}")),
    ("png", (140, "\u{e60d}")),
    ("pp", (255, "\u{e631}")),
    ("ppt", (167, "\u{f0227}")),
    ("prisma", (255, "\u{e684}")),
    ("pro", (179, "\u{e7a1}")),
    ("ps1", (69, "\u{f0a0a}")),
    ("psb", (67, "\u{e7b8}")),
    ("psd1", (105, "\u{f0a0a}")),
    ("psd", (67, "\u{e7b8}")),
    ("psm1", (105, "\u{f0a0a}")),
    ("pyc", (67, "\u{e606}")),
    ("py", (61, "\u{e606}")),
    ("pyd", (67, "\u{e606}")),
    ("pyo", (67, "\u{e606}")),
    ("query", (154, "\u{e21c}")),
    ("rake", (52, "\u{e791}")),
    ("rb", (52, "\u{e791}")),
    ("r", (65, "\u{f07d4}")),
    ("rlib", (180, "\u{e7a8}")),
    ("rmd", (67, "\u{e609}")),
    ("rproj", (65, "\u{f07d4}")),
    ("rs", (180, "\u{e7a8}")),
    ("rss", (215, "\u{e619}")),
    ("sass", (204, "\u{e603}")),
    ("sbt", (167, "\u{e737}")),
    ("scala", (167, "\u{e737}")),
    ("scm", (16, "\u{f0627}")),
    ("scss", (204, "\u{e603}")),
    ("sh", (59, "\u{e795}")),
    ("sig", (173, "\u{3bb}")),
    ("slim", (166, "\u{e60e}")),
    ("sln", (98, "\u{e70c}")),
    ("sml", (173, "\u{3bb}")),
    ("sol", (67, "\u{f07bb}")),
    ("sql", (188, "\u{e706}")),
    ("sqlite3", (188, "\u{e706}")),
    ("sqlite", (188, "\u{e706}")),
    ("styl", (107, "\u{e600}")),
    ("sublime", (98, "\u{e7aa}")),
    ("suo", (98, "\u{e70c}")),
    ("sv", (29, "\u{f035b}")),
    ("svelte", (202, "\u{f260}")),
    ("svg", (215, "\u{f0721}")),
    ("svh", (29, "\u{f035b}")),
    ("swift", (173, "\u{e755}")),
    ("tbc", (67, "\u{f06d3}")),
    ("t", (67, "\u{e769}")),
    ("tcl", (67, "\u{f06d3}")),
    ("terminal", (71, "\u{f489}")),
    ("tex", (58, "\u{f0669}")),
    ("tf", (57, "\u{e2a6}")),
    ("tfvars", (57, "\u{f15b}")),
    ("toml", (66, "\u{e615}")),
    ("tres", (185, "\u{e706}")),
    ("ts", (67, "\u{e628}")),
    ("tscn", (140, "\u{f0381}")),
    ("tsx", (67, "\u{e7ba}")),
    ("twig", (107, "\u{e61c}")),
    ("txt", (113, "\u{f0219}")),
    ("vala", (5, "\u{e69e}")),
    ("v", (29, "\u{f035b}")),
    ("vh", (29, "\u{f035b}")),
    ("vhd", (29, "\u{f035b}")),
    ("vhdl", (29, "\u{f035b}")),
    ("vim", (29, "\u{e62b}")),
    ("vue", (107, "\u{f0844}")),
    ("wasm", (99, "\u{e6a1}")),
    ("webmanifest", (221, "\u{e60b}")),
    ("webpack", (67, "\u{f072b}")),
    ("webp", (140, "\u{e60d}")),
    ("xcplayground", (173, "\u{e755}")),
    ("xls", (23, "\u{f021b}")),
    ("xml", (173, "\u{f05c0}")),
    ("xul", (173, "\u{e745}")),
    ("yaml", (66, "\u{e615}")),
    ("yml", (66, "\u{e615}")),
    ("zig", (208, "\u{f0e7}")),
    ("zsh", (113, "\u{e795}")),
];

static EXT_ICON_MAP: LazyLock<Map<&OsStr, (u8, &str)>> = LazyLock::new(|| {
    EXT_ICONS
        .iter()
        .map(|&(ext, icon)| (OsStr::new(ext), icon))
        .collect()
});

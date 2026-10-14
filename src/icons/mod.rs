use crate::hash;
use nu_ansi_term::Color;
use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    fs::FileType,
    sync::LazyLock,
};

/// Concerned with computing icons given filesystem parameters.
pub mod fs;

/// Attempts to return an icon given a file extension along with its default color code 8-bit
/// value.
fn icon_from_ext(ext: &OsStr) -> Option<(u8, &'static str)> {
    EXT_ICON_MAP.get(ext).copied()
}

/// Attempts to return an icon based on file type.
fn icon_from_file_type(ft: FileType) -> Option<&'static str> {
    if ft.is_dir() {
        return FILE_TYPE_ICON_MAP.get("dir").copied();
    } else if ft.is_symlink() {
        return FILE_TYPE_ICON_MAP.get("symlink").copied();
    }

    None
}

/// Attempts to get the icon associated with the special file kind.
fn icon_from_file_name(name: &OsStr) -> Option<&'static str> {
    FILE_NAME_ICON_MAP.get(name).copied()
}

/// Returns the default fallback icon.
fn get_default_icon<'a>() -> (u8, &'a str) {
    *DEFAULT_ICON
}

/// Convenience method to paint fixed colors.
fn col(num: u8, code: &str) -> String {
    Color::Fixed(num).paint(code).to_string()
}

/// Default fallback icon.
static DEFAULT_ICON: LazyLock<(u8, &str)> = LazyLock::new(|| (66, "\u{f15b}"));

/// Lazily evaluated static hash-map of special file-types and their corresponding styled icons.
/// These icons will take on the color properties of their associated file which is based on
/// `LS_COLORS`.
///
/// Dev icons sourced from [`exa`](https://github.com/ogham/exa/blob/master/src/output/icons.rs)
static FILE_TYPE_ICON_MAP: LazyLock<HashMap<&str, &str>> = LazyLock::new(|| {
    hash!(
        "dir"     => "\u{f413}", // 
        "symlink" => "\u{f482}"  // 
    )
});

/// Lazily evaluated static hash-map of special named and their corresponding icons. These icons
/// will take on the color properties of their associated file which is based on `LS_COLORS`.
///
/// Dev icons sourced from [`exa`](https://github.com/ogham/exa/blob/master/src/output/icons.rs)
static FILE_NAME_ICON_MAP: LazyLock<HashMap<OsString, &str>> = LazyLock::new(|| {
    hash!(
        OsString::from(".Trash")             => "\u{f1f8}", // 
        OsString::from(".atom")              => "\u{e764}", // 
        OsString::from(".bashprofile")       => "\u{e615}", // 
        OsString::from(".bashrc")            => "\u{f489}", // 
        OsString::from(".git")               => "\u{f1d3}", // 
        OsString::from(".gitattributes")     => "\u{f1d3}", // 
        OsString::from(".gitconfig")         => "\u{f1d3}", // 
        OsString::from(".github")            => "\u{f408}", // 
        OsString::from(".gitignore")         => "\u{f1d3}", // 
        OsString::from(".gitmodules")        => "\u{f1d3}", // 
        OsString::from(".rvm")               => "\u{e21e}", // 
        OsString::from(".vimrc")             => "\u{e62b}", // 
        OsString::from(".vscode")            => "\u{e70c}", // 
        OsString::from(".zshrc")             => "\u{f489}", // 
        OsString::from("Cargo.lock")         => "\u{e7a8}", // 
        OsString::from("bin")                => "\u{e5fc}", // 
        OsString::from("config")             => "\u{e5fc}", // 
        OsString::from("docker-compose.yml") => "\u{f308}", // 
        OsString::from("Dockerfile")         => "\u{f308}", // 
        OsString::from(".DS_Store")          => "\u{f179}", // 
        OsString::from("gitignore_global")   => "\u{f1d3}", // 
        OsString::from("go.mod")             => "\u{e626}", // 
        OsString::from("go.sum")             => "\u{e626}", // 
        OsString::from("gradle")             => "\u{e256}", // 
        OsString::from("gruntfile.coffee")   => "\u{e611}", // 
        OsString::from("gruntfile.js")       => "\u{e611}", // 
        OsString::from("gruntfile.ls")       => "\u{e611}", // 
        OsString::from("gulpfile.coffee")    => "\u{e610}", // 
        OsString::from("gulpfile.js")        => "\u{e610}", // 
        OsString::from("gulpfile.ls")        => "\u{e610}", // 
        OsString::from("hidden")             => "\u{f023}", // 
        OsString::from("include")            => "\u{e5fc}", // 
        OsString::from("lib")                => "\u{f121}", // 
        OsString::from("license")            => "\u{e60a}",   // 
        OsString::from("LICENSE")            => "\u{e60a}",   // 
        OsString::from("licence")            => "\u{e60a}",   // 
        OsString::from("LICENCE")            => "\u{e60a}",   // 
        OsString::from("localized")          => "\u{f179}", // 
        OsString::from("Makefile")           => "\u{f489}", // 
        OsString::from("node_modules")       => "\u{e718}", // 
        OsString::from("npmignore")          => "\u{e71e}", // 
        OsString::from("PKGBUILD")           => "\u{f303}", // 
        OsString::from("rubydoc")            => "\u{e73b}", // 
        OsString::from("yarn.lock")          => "\u{e718}"  // 
    )
});

/// Lazily evaluated static hash-map of various file extensions and their corresponding icons. The
/// key is the file extension while the associated value is a tuple containing the 8-bit color code
/// as well as the Unicode scalar value for the corresponding icon.
///
/// Dev icons and their color palettes sourced from [`nvim-web-devicons`](https://github.com/nvim-tree/nvim-web-devicons/blob/master/lua/nvim-web-devicons.lua).
static EXT_ICON_MAP: LazyLock<HashMap<OsString, (u8, &str)>> = LazyLock::new(|| {
    hash!(
        OsString::from("ai")            => (185, "\u{e7b4}"),   // 
        OsString::from("awk")           => (59, "\u{e795}"),    // 
        OsString::from("bash")          => (113, "\u{e795}"),   // 
        OsString::from("bat")           => (154, "\u{e615}"),   // 
        OsString::from("bmp")           => (140, "\u{e60d}"),   // 
        OsString::from("cbl")           => (25, "\u{2699}"),    // ⚙
        OsString::from("c++")           => (204, "\u{e61d}"),   // 
        OsString::from("c")             => (75, "\u{e61e}"),    // 
        OsString::from("cc")            => (204, "\u{e61d}"),   // 
        OsString::from("cfg")           => (231, "\u{e7a3}"),   // 
        OsString::from("cljc")          => (107, "\u{e768}"),   // 
        OsString::from("clj")           => (107, "\u{e768}"),   // 
        OsString::from("cljd")          => (67, "\u{e76a}"),    // 
        OsString::from("cljs")          => (67, "\u{e76a}"),    // 
        OsString::from("cmake")         => (66, "\u{e615}"),    // 
        OsString::from("cob")           => (25, "\u{2699}"),    // ⚙
        OsString::from("cobol")         => (25, "\u{2699}"),    // ⚙
        OsString::from("coffee")        => (185, "\u{e61b}"),   // 
        OsString::from("conf")          => (66, "\u{e615}"),    // 
        OsString::from("config.ru")     => (52, "\u{e791}"),    // 
        OsString::from("cp")            => (67, "\u{e61d}"),    // 
        OsString::from("cpp")           => (67, "\u{e61d}"),    // 
        OsString::from("cpy")           => (25, "\u{2699}"),    // ⚙
        OsString::from("cr")            => (16, "\u{e24f}"),    // 
        OsString::from("cs")            => (58, "\u{f031b}"),   // 󰌛
        OsString::from("csh")           => (59, "\u{e795}"),    // 
        OsString::from("cson")          => (185, "\u{e60b}"),   // 
        OsString::from("css")           => (39, "\u{e749}"),    // 
        OsString::from("csv")           => (113, "\u{f0219}"),  // 󰈙
        OsString::from("cxx")           => (67, "\u{e61d}"),    // 
        OsString::from("dart")          => (25, "\u{e798}"),    // 
        OsString::from("db")            => (188, "\u{e706}"),   // 
        OsString::from("d")             => (64, "\u{e7af}"),    // 
        OsString::from("desktop")       => (60, "\u{f108}"),    // 
        OsString::from("diff")          => (59, "\u{e728}"),    // 
        OsString::from("doc")           => (25, "\u{f022c}"),   // 󰈬
        OsString::from("drl")           => (217, "\u{e28c}"),   // 
        OsString::from("dropbox")       => (27, "\u{e707}"),    // 
        OsString::from("dump")          => (188, "\u{e706}"),   // 
        OsString::from("edn")           => (67, "\u{e76a}"),    // 
        OsString::from("eex")           => (140, "\u{e62d}"),   // 
        OsString::from("ejs")           => (185, "\u{e60e}"),   // 
        OsString::from("elm")           => (67, "\u{e62c}"),    // 
        OsString::from("epp")           => (255, "\u{e631}"),   // 
        OsString::from("erb")           => (52, "\u{e60e}"),    // 
        OsString::from("erl")           => (132, "\u{e7b1}"),   // 
        OsString::from("ex")            => (140, "\u{e62d}"),   // 
        OsString::from("exs")           => (140, "\u{e62d}"),   // 
        OsString::from("f#")            => (67, "\u{e7a7}"),    // 
        OsString::from("fish")          => (59, "\u{e795}"),    // 
        OsString::from("fnl")           => (230, "\u{1f31c}"),  // 🌜
        OsString::from("fs")            => (67, "\u{e7a7}"),    // 
        OsString::from("fsi")           => (67, "\u{e7a7}"),    // 
        OsString::from("fsscript")      => (67, "\u{e7a7}"),    // 
        OsString::from("fsx")           => (67, "\u{e7a7}"),    // 
        OsString::from("GNUmakefile")   => (66, "\u{e779}"),    // 
        OsString::from("gd")            => (66, "\u{e615}"),    // 
        OsString::from("gemspec")       => (52, "\u{e791}"),    // 
        OsString::from("gif")           => (140, "\u{e60d}"),   // 
        OsString::from("git")           => (202, "\u{e702}"),   // 
        OsString::from("glb")           => (215, "\u{f1b2}"),   // 
        OsString::from("go")            => (67, "\u{e627}"),    // 
        OsString::from("godot")         => (66, "\u{e7a3}"),    // 
        OsString::from("gql")           => (199, "\u{f20e}"),   // 
        OsString::from("graphql")       => (199, "\u{f20e}"),   // 
        OsString::from("haml")          => (188, "\u{e60e}"),   // 
        OsString::from("hbs")           => (208, "\u{e60f}"),   // 
        OsString::from("h")             => (140, "\u{f0fd}"),   // 
        OsString::from("heex")          => (140, "\u{e62d}"),   // 
        OsString::from("hh")            => (140, "\u{f0fd}"),   // 
        OsString::from("hpp")           => (140, "\u{f0fd}"),   // 
        OsString::from("hrl")           => (132, "\u{e7b1}"),   // 
        OsString::from("hs")            => (140, "\u{e61f}"),   // 
        OsString::from("htm")           => (166, "\u{e60e}"),   // 
        OsString::from("html")          => (202, "\u{e736}"),   // 
        OsString::from("hxx")           => (140, "\u{f0fd}"),   // 
        OsString::from("ico")           => (185, "\u{e60d}"),   // 
        OsString::from("import")        => (231, "\u{f0c6}"),   // 
        OsString::from("ini")           => (66, "\u{e615}"),    // 
        OsString::from("java")          => (167, "\u{e738}"),   // 
        OsString::from("jl")            => (133, "\u{e624}"),   // 
        OsString::from("jpeg")          => (140, "\u{e60d}"),   // 
        OsString::from("jpg")           => (140, "\u{e60d}"),   // 
        OsString::from("js")            => (185, "\u{e60c}"),   // 
        OsString::from("json5")         => (185, "\u{f0626}"),  // 󰘦
        OsString::from("json")          => (185, "\u{e60b}"),   // 
        OsString::from("jsx")           => (67, "\u{e625}"),    // 
        OsString::from("ksh")           => (59, "\u{e795}"),    // 
        OsString::from("kt")            => (99, "\u{e634}"),    // 
        OsString::from("kts")           => (99, "\u{e634}"),    // 
        OsString::from("leex")          => (140, "\u{e62d}"),   // 
        OsString::from("less")          => (60, "\u{e614}"),    // 
        OsString::from("lhs")           => (140, "\u{e61f}"),   // 
        OsString::from("license")       => (185, "\u{e60a}"),   // 
        OsString::from("licence")       => (185, "\u{e60a}"),   // 
        OsString::from("lock")          => (250, "\u{f13e}"),   // 
        OsString::from("log")           => (255, "\u{f0331}"),  // 󰮩
        OsString::from("lua")           => (74, "\u{e620}"),    // 
        OsString::from("luau")          => (74, "\u{e620}"),    // 
        OsString::from("makefile")      => (66, "\u{e779}"),    // 
        OsString::from("markdown")      => (67, "\u{e609}"),    // 
        OsString::from("Makefile")      => (66, "\u{e779}"),    // 
        OsString::from("material")      => (132, "\u{f0509}"),  // 󰔉
        OsString::from("md")            => (255, "\u{f48a}"),   // 
        OsString::from("mdx")           => (67, "\u{f48a}"),    // 
        OsString::from("mint")          => (108, "\u{f032a}"),  // 󰌪
        OsString::from("mjs")           => (221, "\u{e60c}"),   // 
        OsString::from("mk")            => (66, "\u{e779}"),    // 
        OsString::from("ml")            => (173, "\u{3bb}"),    // λ
        OsString::from("mli")           => (173, "\u{3bb}"),    // λ
        OsString::from("mo")            => (99, "\u{221e}"),    // ∞
        OsString::from("mustache")      => (173, "\u{e60f}"),   // 
        OsString::from("nim")           => (220, "\u{e677}"),   // 
        OsString::from("nix")           => (110, "\u{f313}"),   // 
        OsString::from("opus")          => (208, "\u{f0223}"),  // 󰈣
        OsString::from("otf")           => (231, "\u{f031}"),   // 
        OsString::from("pck")           => (66, "\u{f487}"),    // 
        OsString::from("pdf")           => (124, "\u{f0226}"),  // 󰈦
        OsString::from("php")           => (140, "\u{e608}"),   // 
        OsString::from("pl")            => (67, "\u{e769}"),    // 
        OsString::from("pm")            => (67, "\u{e769}"),    // 
        OsString::from("png")           => (140, "\u{e60d}"),   // 
        OsString::from("pp")            => (255, "\u{e631}"),   // 
        OsString::from("ppt")           => (167, "\u{f0227}"),  // 󰈧
        OsString::from("prisma")        => (255, "\u{e684}"),   // 
        OsString::from("pro")           => (179, "\u{e7a1}"),   // 
        OsString::from("ps1")           => (69, "\u{f0a0a}"),   // 󰨊
        OsString::from("psb")           => (67, "\u{e7b8}"),    // 
        OsString::from("psd1")          => (105, "\u{f0a0a}"),  // 󰨊
        OsString::from("psd")           => (67, "\u{e7b8}"),    // 
        OsString::from("psm1")          => (105, "\u{f0a0a}"),  // 󰨊
        OsString::from("pyc")           => (67, "\u{e606}"),    // 
        OsString::from("py")            => (61, "\u{e606}"),    // 
        OsString::from("pyd")           => (67, "\u{e606}"),    // 
        OsString::from("pyo")           => (67, "\u{e606}"),    // 
        OsString::from("query")         => (154, "\u{e21c}"),   // 
        OsString::from("rake")          => (52, "\u{e791}"),    // 
        OsString::from("rb")            => (52, "\u{e791}"),    // 
        OsString::from("r")             => (65, "\u{f07d4}"),   // 󰟔
        OsString::from("rlib")          => (180, "\u{e7a8}"),   // 
        OsString::from("rmd")           => (67, "\u{e609}"),    // 
        OsString::from("rproj")         => (65, "\u{f07d4}"),   // 󰟔
        OsString::from("rs")            => (180, "\u{e7a8}"),   // 
        OsString::from("rss")           => (215, "\u{e619}"),   // 
        OsString::from("sass")          => (204, "\u{e603}"),   // 
        OsString::from("sbt")           => (167, "\u{e737}"),   // 
        OsString::from("scala")         => (167, "\u{e737}"),   // 
        OsString::from("scm")           => (16, "\u{f0627}"),   // 󰘧
        OsString::from("scss")          => (204, "\u{e603}"),   // 
        OsString::from("sh")            => (59, "\u{e795}"),    // 
        OsString::from("sig")           => (173, "\u{3bb}"),    // λ
        OsString::from("slim")          => (166, "\u{e60e}"),   // 
        OsString::from("sln")           => (98, "\u{e70c}"),    // 
        OsString::from("sml")           => (173, "\u{3bb}"),    // λ
        OsString::from("sol")           => (67, "\u{f07bb}"),   // 󰞻
        OsString::from("sql")           => (188, "\u{e706}"),   // 
        OsString::from("sqlite3")       => (188, "\u{e706}"),   // 
        OsString::from("sqlite")        => (188, "\u{e706}"),   // 
        OsString::from("styl")          => (107, "\u{e600}"),   // 
        OsString::from("sublime")       => (98, "\u{e7aa}"),    // 
        OsString::from("suo")           => (98, "\u{e70c}"),    // 
        OsString::from("sv")            => (29, "\u{f035b}"),   // 󰍛
        OsString::from("svelte")        => (202, "\u{f260}"),   // 
        OsString::from("svg")           => (215, "\u{f0721}"),  // 󰜡
        OsString::from("svh")           => (29, "\u{f035b}"),   // 󰍛
        OsString::from("swift")         => (173, "\u{e755}"),   // 
        OsString::from("tbc")           => (67, "\u{f06d3}"),   // 󰛓
        OsString::from("t")             => (67, "\u{e769}"),    // 
        OsString::from("tcl")           => (67, "\u{f06d3}"),   // 󰛓
        OsString::from("terminal")      => (71, "\u{f489}"),    // 
        OsString::from("test.js")       => (173, "\u{e60c}"),   // 
        OsString::from("tex")           => (58, "\u{f0669}"),   // 󰙩
        OsString::from("tf")            => (57, "\u{e2a6}"),    // 
        OsString::from("tfvars")        => (57, "\u{f15b}"),    // 
        OsString::from("toml")          => (66, "\u{e615}"),    // 
        OsString::from("tres")          => (185, "\u{e706}"),   // 
        OsString::from("ts")            => (67, "\u{e628}"),    // 
        OsString::from("tscn")          => (140, "\u{f0381}"),  // 󰎁
        OsString::from("tsx")           => (67, "\u{e7ba}"),    // 
        OsString::from("twig")          => (107, "\u{e61c}"),   // 
        OsString::from("txt")           => (113, "\u{f0219}"),  // 󰈙
        OsString::from("vala")          => (5, "\u{e69e}"),     // 
        OsString::from("v")             => (29, "\u{f035b}"),   // 󰍛
        OsString::from("vh")            => (29, "\u{f035b}"),   // 󰍛
        OsString::from("vhd")           => (29, "\u{f035b}"),   // 󰍛
        OsString::from("vhdl")          => (29, "\u{f035b}"),   // 󰍛
        OsString::from("vim")           => (29, "\u{e62b}"),    // 
        OsString::from("vue")           => (107, "\u{f0844}"),  // 󰡄
        OsString::from("wasm")          => (99, "\u{e6a1}"),    // 
        OsString::from("webmanifest")   => (221, "\u{e60b}"),   // 
        OsString::from("webpack")       => (67, "\u{f072b}"),   // 󰜫
        OsString::from("webp")          => (140, "\u{e60d}"),   // 
        OsString::from("xcplayground")  => (173, "\u{e755}"),   // 
        OsString::from("xls")           => (23, "\u{f021b}"),   // 󰈛
        OsString::from("xml")           => (173, "\u{f05c0}"),  // 󰗀
        OsString::from("xul")           => (173, "\u{e745}"),   // 
        OsString::from("yaml")          => (66, "\u{e615}"),    // 
        OsString::from("yml")           => (66, "\u{e615}"),    // 
        OsString::from("zig")           => (208, "\u{f0e7}"),   // 
        OsString::from("zsh")           => (113, "\u{e795}")    // 
    )
});

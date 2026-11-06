use crate::{disk_usage::units::PREFIXES, sync::unpoisoned};
use lscolors::LsColors;
use nu_ansi_term::{Color, Style};
use std::{
    sync::{Mutex, OnceLock},
    thread::JoinHandle,
};

/// `LS_COLORS` style of a directory entry.
pub mod entry;

/// Used as general placeholder for an empty field.
pub const PLACEHOLDER: &str = "-";

/// Used for padding between tree branches.
pub const SEP: &str = "   ";

/// The `│` box drawing character.
pub const VT: &str = "\u{2502}  ";

/// The `┌─` box drawing character.
pub const DRT: &str = "\u{250C}\u{2500} ";

/// The `└─` box drawing characters.
pub const UPRT: &str = "\u{2514}\u{2500} ";

/// The `├─` box drawing characters.
pub const VTRT: &str = "\u{251C}\u{2500} ";

/// Everything output is drawn with: the branches, and the colors unless output is plain.
pub struct Theme {
    /// Branches of the tree.
    pub tree: Glyphs,

    /// Branches of symlinks followed with `--follow` and of their descendants.
    pub link: Glyphs,

    /// `None` without color; the only record of whether output is colored.
    pub colors: Option<Colors>,
}

/// Box-drawing pieces of a theme, colored or not.
pub struct Glyphs {
    pub vt: Box<[u8]>,
    pub vtrt: Box<[u8]>,
    pub uprt: Box<[u8]>,
    pub drt: Box<[u8]>,
}

/// The styles of colored output.
pub struct Colors {
    /// File name styles from the `LS_COLORS` environment variable, or the default of the
    /// `lscolors` crate if it is not set.
    ///
    /// **Note for `MacOS`**: `MacOS` uses the `LSCOLORS` environment variable which is in a format
    /// not supported by the `lscolors` crate. Mac users can either set their own `LS_COLORS`
    /// environment variable to customize output color or rely on the default.
    pub ls: LsColors,

    /// Size colors by unit, from bytes up to tera.
    pub du: [Style; PREFIXES],

    /// Style of the general use placeholder [`PLACEHOLDER`].
    pub placeholder: Style,

    pub link_arrow: Style,

    #[cfg(unix)]
    pub long: Long,
}

/// Styles of the columns of the long view.
#[cfg(unix)]
#[derive(Clone, Copy)]
pub struct Long {
    pub octal: Style,
    pub ino: Style,
    pub nlink: Style,
    pub owner: Style,
    pub group: Style,
    pub datetime: Style,
}

impl Glyphs {
    fn new(color: Option<Color>) -> Self {
        let paint = |s: &str| {
            color
                .map_or_else(|| s.to_owned(), |c| c.paint(s).to_string())
                .into_bytes()
                .into_boxed_slice()
        };

        Self {
            vt: paint(VT),
            vtrt: paint(VTRT),
            uprt: paint(UPRT),
            drt: paint(DRT),
        }
    }
}

/// The theme of the run, kept for the whole process (walk workers hold its `LS_COLORS`); the
/// first call builds it. If `plain`, output has no color and `LS_COLORS` is not read.
pub fn init(plain: bool) -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();

    THEME.get_or_init(|| build(plain))
}

/// A new theme, plain or in color.
fn build(plain: bool) -> Theme {
    #[cfg(windows)]
    let _ = nu_ansi_term::enable_ansi_support();

    if plain {
        Theme {
            tree: Glyphs::new(None),
            link: Glyphs::new(None),
            colors: None,
        }
    } else {
        Theme {
            tree: Glyphs::new(Some(Color::Purple)),
            link: Glyphs::new(Some(Color::Red)),
            colors: Some(Colors {
                ls: ls_colors(),
                du: [
                    Color::Cyan.bold(),
                    Color::Yellow.bold(),
                    Color::Green.bold(),
                    Color::Red.bold(),
                    Color::Blue.bold(),
                ],
                placeholder: Color::Purple.normal(),
                link_arrow: Color::Red.normal(),
                #[cfg(unix)]
                long: Long {
                    octal: Color::Purple.bold(),
                    ino: Color::Cyan.bold(),
                    nlink: Color::Red.bold(),
                    owner: Color::Cyan.bold(),
                    group: Color::Green.bold(),
                    datetime: Color::Purple.bold(),
                },
            }),
        }
    }
}

/// Reads the `LS_COLORS` environment variable, or takes the result of [`prefetch_ls_colors`]. If
/// it isn't set, a default determined by `lscolors` crate is used.
fn ls_colors() -> LsColors {
    let handle = unpoisoned(PREFETCH.lock()).take();
    let prefetched = handle.and_then(|handle| handle.join().ok());

    prefetched
        .unwrap_or_else(LsColors::from_env)
        .unwrap_or_default()
}

/// A helper thread started by [`prefetch_ls_colors`].
static PREFETCH: Mutex<Option<JoinHandle<Option<LsColors>>>> = Mutex::new(None);

/// Starts parsing `LS_COLORS` on a helper thread so that it overlaps with argument parsing; a
/// large `LS_COLORS` takes milliseconds to parse. [`init`] picks up the result, and parses on
/// the calling thread when this was not called or the thread failed.
pub fn prefetch_ls_colors() {
    *unpoisoned(PREFETCH.lock()) = Some(std::thread::spawn(LsColors::from_env));
}

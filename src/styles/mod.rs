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

/// The theme of the run, set once by [`init`].
static THEME: OnceLock<Theme> = OnceLock::new();

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
    pub vt: String,
    pub vtrt: String,
    pub uprt: String,
    pub drt: String,
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
    pub du: [Style; 5],

    /// Style of the general use placeholder [`PLACEHOLDER`].
    pub placeholder: Style,

    #[cfg(unix)]
    pub long: Long,
}

/// Styles of the columns of the long view.
#[cfg(unix)]
pub struct Long {
    pub octal: Style,
    pub ino: Style,
    pub nlink: Style,
    pub owner: Style,
    pub group: Style,
    pub datetime: Style,
}

#[cfg(unix)]
impl Long {
    /// The style of a character of symbolic permissions; characters without one are not shown.
    pub fn permission(ch: char) -> Option<Style> {
        Some(match ch {
            '-' | '.' | '?' => Color::Purple.normal(),
            'd' => Color::Blue.bold(),
            'r' => Color::Green.bold(),
            'w' => Color::Yellow.bold(),
            'l' | 'x' | 's' | 'S' | 't' | 'T' => Color::Red.bold(),
            '@' => Color::Cyan.bold(),
            ' ' => Color::White.normal(),
            _ => return None,
        })
    }
}

impl Theme {
    /// Branches of the link theme if `link`, else of the tree theme.
    pub const fn glyphs(&self, link: bool) -> &Glyphs {
        if link { &self.link } else { &self.tree }
    }
}

impl Glyphs {
    /// The pieces, each painted in `color` if given.
    fn new(color: Option<Color>) -> Self {
        let paint = |s: &str| color.map_or_else(|| s.to_owned(), |c| c.paint(s).to_string());

        Self {
            vt: paint(VT),
            vtrt: paint(VTRT),
            uprt: paint(UPRT),
            drt: paint(DRT),
        }
    }
}

/// Sets the theme of the run. If `plain`, output has no color and `LS_COLORS` is not read.
pub fn init(plain: bool) {
    #[cfg(windows)]
    let _ = nu_ansi_term::enable_ansi_support();

    let theme = if plain {
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
    };

    assert!(THEME.set(theme).is_ok(), "the theme is set once");
}

/// The theme of the run.
///
/// # Panics
///
/// If [`init`] has not run.
pub fn theme() -> &'static Theme {
    THEME.get().expect("styles::init runs first")
}

/// The colors of the run, `None` without color.
pub fn colors() -> Option<&'static Colors> {
    theme().colors.as_ref()
}

/// Reads the `LS_COLORS` environment variable, or takes the result of [`prefetch_ls_colors`]. If
/// it isn't set, a default determined by `lscolors` crate is used.
fn ls_colors() -> LsColors {
    let prefetched = PREFETCH
        .lock()
        .ok()
        .and_then(|mut slot| slot.take())
        .and_then(|handle| handle.join().ok());

    prefetched
        .unwrap_or_else(LsColors::from_env)
        .unwrap_or_default()
}

/// A helper thread started by [`prefetch_ls_colors`].
static PREFETCH: Mutex<Option<JoinHandle<Option<LsColors>>>> = Mutex::new(None);

/// Starts parsing `LS_COLORS` on a helper thread so that it overlaps with argument parsing; a
/// large theme takes milliseconds to build. [`init`] picks up the result, and parses on the
/// calling thread when this was not called or the thread failed.
pub fn prefetch_ls_colors() {
    if let Ok(mut slot) = PREFETCH.lock() {
        *slot = Some(std::thread::spawn(LsColors::from_env));
    }
}

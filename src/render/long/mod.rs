use super::paint::{Esc, paint, placeholder};
use crate::{
    bytes::{push_spaces, push_str_padded, push_u64_padded},
    context::{Context, time},
    styles::{self, Colors, PLACEHOLDER},
    tree::{node::Node, widths::Widths},
};
use chrono::{DateTime, FixedOffset, Utc};
use nu_ansi_term::{Color, Style};
use std::time::SystemTime;

/// Timestamps without chrono's per-row conversion.
mod clock;

/// The local time zone as chrono finds it, without linking CoreFoundation.
#[cfg(target_os = "macos")]
mod zone;

/// Permission cells kept per thread; modes are few, so this is not reached in practice.
const PERMS_CAP: usize = 512;

/// The columns of the long view and how to write them. Built once per run.
pub struct Long {
    group: bool,
    ino: bool,
    nlink: bool,
    octal: bool,
    owner_width: usize,
    group_width: usize,
    ino_width: usize,
    nlink_width: usize,
    stamp: time::Stamp,
    format: time::Format,
    /// The long-view styles; `None` without color.
    styles: Option<styles::Long>,
    ino_esc: Option<Esc>,
    nlink_esc: Option<Esc>,
    owner_esc: Option<Esc>,
    group_esc: Option<Esc>,
    time_esc: Option<Esc>,
}

/// Per-thread memory of the long view: finished permission cells and the time zone offsets.
#[derive(Default)]
pub struct State {
    /// Cells by (mode, has extended attributes).
    perms: Vec<(u32, bool, Box<[u8]>)>,
    last_perm: usize,
    clock: clock::Clock,
    time: Vec<u8>,
}

/// The style of a character of symbolic permissions.
fn permission_style(ch: char) -> Style {
    match ch {
        'd' => Color::Blue.bold(),
        'p' => Color::Yellow.normal(),
        'r' => Color::Green.bold(),
        'w' | 'c' | 'b' => Color::Yellow.bold(),
        'l' | 'x' | 's' | 'S' | 't' | 'T' => Color::Red.bold(),
        '@' => Color::Cyan.bold(),
        ' ' => Color::White.normal(),
        // `-`, `.` and `?`.
        _ => Color::Purple.normal(),
    }
}

fn style_sym_permissions(mode: u32, has_xattrs: bool, colored: bool) -> String {
    // A trailing space aligns entries without extended attributes with those with them.
    let symb: String = crate::fs::permissions::symbolic(mode)
        .iter()
        .map(|&b| char::from(b))
        .chain([if has_xattrs { '@' } else { ' ' }])
        .collect();

    if colored {
        symb.chars()
            .map(|ch| permission_style(ch).paint(ch.to_string()).to_string())
            .collect()
    } else {
        symb
    }
}

fn style_oct_permissions(mode: u32, style: Option<Style>) -> String {
    let oct = crate::fs::permissions::octal(mode);

    if let Some(style) = style {
        style.paint(oct).to_string()
    } else {
        oct
    }
}

impl Long {
    pub fn new(ctx: &Context, widths: &Widths, colors: Option<&Colors>) -> Self {
        let styles = colors.map(|colors| colors.long);
        // The sequences of a long-view style, `None` without color.
        let esc = |style: fn(&styles::Long) -> Style| styles.as_ref().map(|s| Esc::new(style(s)));

        Self {
            group: ctx.group,
            ino: ctx.ino,
            nlink: ctx.nlink,
            octal: ctx.octal,
            owner_width: widths.max_owner_width,
            group_width: widths.max_group_width,
            ino_width: widths.max_ino_width,
            nlink_width: widths.max_nlink_width,
            stamp: ctx.time(),
            format: ctx.time_format(),
            styles,
            ino_esc: esc(|long| long.ino),
            nlink_esc: esc(|long| long.nlink),
            owner_esc: esc(|long| long.owner),
            group_esc: esc(|long| long.group),
            time_esc: esc(|long| long.datetime),
        }
    }

    /// Appends the long columns of `node`, without a trailing separator: inode, permissions,
    /// link count, owner, group and time; inode, link count and group only when selected.
    pub fn write(&self, state: &mut State, node: Node<'_>, out: &mut Vec<u8>) {
        if self.ino {
            Self::write_number(out, node.ino(), self.ino_width, self.ino_esc.as_ref());
            out.push(b' ');
        }

        self.write_perms(state, node, out);
        out.push(b' ');

        if self.nlink {
            Self::write_number(out, node.nlink(), self.nlink_width, self.nlink_esc.as_ref());
            out.push(b' ');
        }

        let names = node.owner_and_group();

        paint(out, self.owner_esc.as_ref(), |out| {
            push_str_padded(out, names.map_or(PLACEHOLDER, |n| n.0), self.owner_width);
        });
        out.push(b' ');

        if self.group {
            paint(out, self.group_esc.as_ref(), |out| {
                push_str_padded(out, names.map_or(PLACEHOLDER, |n| n.1), self.group_width);
            });
            out.push(b' ');
        }

        self.write_time(state, node, out);
    }

    /// An optional number right-aligned to `width`, or the placeholder.
    fn write_number(out: &mut Vec<u8>, value: Option<u64>, width: usize, esc: Option<&Esc>) {
        paint(out, esc, |out| {
            if let Some(n) = value {
                push_u64_padded(out, n, width);
            } else {
                placeholder(out, width);
            }
        });
    }

    /// Permissions, symbolic or octal, from the per-thread cache.
    fn write_perms(&self, state: &mut State, node: Node<'_>, out: &mut Vec<u8>) {
        let mode = node.st_mode();
        // The octal cell does not depend on extended attributes.
        let xattrs = !self.octal && node.has_xattrs();

        let hit = if state
            .perms
            .get(state.last_perm)
            .is_some_and(|(m, x, _)| *m == mode && *x == xattrs)
        {
            Some(state.last_perm)
        } else {
            state
                .perms
                .iter()
                .position(|(m, x, _)| *m == mode && *x == xattrs)
        };

        if let Some(at) = hit {
            state.last_perm = at;
            out.extend_from_slice(&state.perms[at].2);
            return;
        }

        let cell = if self.octal {
            style_oct_permissions(mode, self.styles.map(|s| s.octal))
        } else {
            style_sym_permissions(mode, xattrs, self.styles.is_some())
        };

        out.extend_from_slice(cell.as_bytes());
        if state.perms.len() < PERMS_CAP {
            state.perms.push((mode, xattrs, cell.into_bytes().into()));
        }
    }

    /// The timestamp right-aligned to the column width, or the placeholder.
    fn write_time(&self, state: &mut State, node: Node<'_>, out: &mut Vec<u8>) {
        let time = match self.stamp {
            time::Stamp::Create => node.created(),
            time::Stamp::Access => node.accessed(),
            time::Stamp::Mod => node.modified(),
        };

        paint(out, self.time_esc.as_ref(), |out| {
            push_time(out, state, time, self.format);
        });
    }
}

/// Appends `time` in `format` right-aligned to the width of the format, or the placeholder.
fn push_time(out: &mut Vec<u8>, state: &mut State, time: Option<SystemTime>, format: time::Format) {
    let Some(time) = time else {
        placeholder(out, format.width());
        return;
    };

    state.time.clear();
    if !state.clock.write(&mut state.time, time, format) {
        chrono_format(&mut state.time, time, format);
    }

    push_spaces(out, format.width().saturating_sub(state.time.len()));
    out.extend_from_slice(&state.time);
}

/// Appends `time` in `format` the way chrono does it.
fn chrono_format(out: &mut Vec<u8>, time: SystemTime, format: time::Format) {
    use std::io::Write;

    let utc = DateTime::<Utc>::from(time);
    let offset = FixedOffset::east_opt(clock::local_offset(utc.timestamp()))
        .expect("offsets are checked by local_offset");
    let _ = write!(
        out,
        "{}",
        utc.with_timezone(&offset).format(format.pattern())
    );
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn numbers_are_aligned_like_format() {
        for width in [0, 1, 5] {
            for value in [None, Some(0), Some(7), Some(123_456_789)] {
                let mut out = vec![];
                Long::write_number(&mut out, value, width, None);
                let expected = value.map_or_else(
                    || format!("{PLACEHOLDER:>width$}"),
                    |n| format!("{n:>width$}"),
                );
                assert_eq!(String::from_utf8(out).unwrap(), expected);
            }
        }
    }

    /// A missing timestamp takes the column width of a present one in every format.
    #[test]
    fn missing_time_is_as_wide_as_a_time() {
        let mut state = State::default();
        let time = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_735_689_600);
        for format in [
            time::Format::Default,
            time::Format::Iso,
            time::Format::IsoStrict,
            time::Format::Short,
        ] {
            let (mut present, mut missing) = (vec![], vec![]);
            push_time(&mut present, &mut state, Some(time), format);
            push_time(&mut missing, &mut state, None, format);
            assert_eq!(missing.len(), present.len(), "{format:?}");
            assert_eq!(missing.len(), format.width(), "{format:?}");
            assert!(missing.ends_with(PLACEHOLDER.as_bytes()));
        }
    }
}

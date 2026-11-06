use super::{
    bytes::{push_spaces, push_str_padded, push_u64_padded},
    theme::{self, Esc},
};
use crate::{
    context::{Context, column, time},
    styles::{self, PLACEHOLDER},
    tree::node::Node,
};
use chrono::DateTime;
#[cfg(not(target_os = "macos"))]
use chrono::Local;
use nu_ansi_term::Style;
use std::time::SystemTime;

/// Timestamps without chrono's per-row conversion.
mod clock;

/// The local time zone as chrono finds it, without linking CoreFoundation.
#[cfg(target_os = "macos")]
mod zone;

/// Width of the timestamp column.
const TIME_WIDTH: usize = 12;

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

/// Writes `body` between the sequences of `esc`, if there is one.
#[inline]
fn painted(out: &mut Vec<u8>, esc: Option<&Esc>, body: impl FnOnce(&mut Vec<u8>)) {
    if let Some(esc) = esc {
        out.extend_from_slice(&esc.pre);
        body(out);
        out.extend_from_slice(&esc.suf);
    } else {
        body(out);
    }
}

/// The sequences of a style the theme may not have (without color).
fn esc(style: Result<&'static Style, styles::error::Error<'static>>) -> Option<Esc> {
    style.ok().map(|style| Esc::new(*style))
}

impl Long {
    /// The long view as selected by `ctx`, with the column widths `widths`.
    pub fn new(ctx: &Context, widths: &column::Properties) -> Self {
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
            ino_esc: esc(styles::get_ino_style()),
            nlink_esc: esc(styles::get_nlink_style()),
            owner_esc: esc(styles::get_owner_style()),
            group_esc: esc(styles::get_group_style()),
            time_esc: esc(styles::get_datetime_style()),
        }
    }

    /// Appends the long columns of `node`, without a trailing separator: inode, permissions,
    /// link count, owner, group and time, of which only the selected ones.
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

        painted(out, self.owner_esc.as_ref(), |out| {
            push_str_padded(out, names.map_or(PLACEHOLDER, |n| n.0), self.owner_width);
        });
        out.push(b' ');

        if self.group {
            painted(out, self.group_esc.as_ref(), |out| {
                push_str_padded(out, names.map_or(PLACEHOLDER, |n| n.1), self.group_width);
            });
            out.push(b' ');
        }

        self.write_time(state, node, out);
    }

    /// An optional number right-aligned to `width`, or the placeholder.
    fn write_number(out: &mut Vec<u8>, value: Option<u64>, width: usize, esc: Option<&Esc>) {
        painted(out, esc, |out| {
            if let Some(n) = value {
                push_u64_padded(out, n, width);
            } else {
                push_spaces(out, width.saturating_sub(1));
                out.extend_from_slice(PLACEHOLDER.as_bytes());
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

        let perms = node.mode().expect("Expected permissions to be initialized");
        let cell = if self.octal {
            theme::style_oct_permissions(&perms)
        } else {
            theme::style_sym_permissions(&perms, xattrs)
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

        painted(out, self.time_esc.as_ref(), |out| {
            let Some(time) = time else {
                push_spaces(out, TIME_WIDTH - 1);
                out.extend_from_slice(PLACEHOLDER.as_bytes());
                return;
            };

            state.time.clear();
            if !state.clock.write(&mut state.time, time, self.format) {
                chrono_format(&mut state.time, time, self.format);
            }

            push_spaces(out, TIME_WIDTH.saturating_sub(state.time.len()));
            out.extend_from_slice(&state.time);
        });
    }
}

/// Appends `time` in `format` the way chrono does it.
fn chrono_format(out: &mut Vec<u8>, time: SystemTime, format: time::Format) {
    use std::io::Write;

    #[cfg(not(target_os = "macos"))]
    let dt = DateTime::<Local>::from(time);

    #[cfg(target_os = "macos")]
    let dt = {
        let utc = DateTime::<chrono::Utc>::from(time);
        let offset = chrono::FixedOffset::east_opt(zone::local_offset(utc.timestamp()))
            .expect("offsets are checked by local_offset");
        utc.with_timezone(&offset)
    };

    let _ = match format {
        time::Format::Default => write!(out, "{}", dt.format("%d %h %H:%M %g")),
        time::Format::Iso => write!(out, "{}", dt.format("%Y-%m-%d %H:%M:%S")),
        time::Format::IsoStrict => write!(out, "{}", dt.format("%Y-%m-%dT%H:%M:%S%Z")),
        time::Format::Short => write!(out, "{}", dt.format("%Y-%m-%d")),
    };
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
}

//! Writes rows as bytes into a buffer. One [`RowFmt`] is built per run; everything that can be
//! decided before the first row (colors, widths, which columns exist) is decided there.

use super::{
    bytes::{push_lossy, push_spaces, push_str_padded, push_u64_padded},
    prefix,
    theme::{Esc, StyleCache},
};
use crate::{
    ansi::Escaped,
    context::Context,
    disk_usage::{
        file_size::{DiskUsage, FileSize},
        units::PrefixKind,
    },
    icons::{self, fs::Paint},
    styles,
    tree::{Tree, node::Node, topo::NONE},
};
use std::path::{MAIN_SEPARATOR, Path};

#[cfg(unix)]
use super::long;
#[cfg(unix)]
use crate::disk_usage::file_size::BLOCK_SIZE_BYTES;

/// How the size column is written.
struct Size {
    width: usize,
    unit_width: usize,
    unit: PrefixKind,

    /// Padding of the placeholder; `None` if rows without a size have no size column at all.
    placeholder: Option<usize>,
    placeholder_esc: Option<Esc>,

    /// Sequences of the unit colors from bytes up; `None` without color.
    du: Option<[Esc; 5]>,
}

impl Size {
    fn new(ctx: &Context, color: bool) -> Self {
        let placeholder = (!ctx.suppress_size && ctx.max_size_width != 0).then(|| {
            let padding = ctx.max_size_width + 1;

            match ctx.disk_usage {
                DiskUsage::Logical | DiskUsage::Physical => match ctx.unit {
                    PrefixKind::Si if ctx.human => padding + 2,
                    PrefixKind::Bin if ctx.human => padding + 3,
                    PrefixKind::Si => padding,
                    PrefixKind::Bin => padding + 1,
                },
                _ => padding - 1,
            }
        });

        let du = color.then(|| {
            let theme = styles::get_du_theme().unwrap();
            ctx.unit
                .names()
                .map(|name| Esc::new(*theme.get(name).unwrap()))
        });

        Self {
            width: ctx.max_size_width,
            unit_width: ctx.max_size_unit_width,
            unit: ctx.unit,
            placeholder,
            placeholder_esc: color
                .then(|| styles::get_placeholder_style().ok().map(|s| Esc::new(*s)))
                .flatten(),
            du,
        }
    }
}

/// Position in the unit colors of a displayed human-readable unit.
const fn unit_index(unit: &str) -> usize {
    match unit.as_bytes()[0] {
        b'B' => 0,
        b'K' => 1,
        b'M' => 2,
        b'G' => 3,
        _ => 4,
    }
}

/// Memory one thread needs to write rows: caches and scratch space. Contents carry no meaning
/// from one row to the next other than as caches.
#[derive(Default)]
pub struct Scratch {
    styles: StyleCache,
    pub cursor: prefix::Cursor,
    ancestors: Vec<u32>,

    #[cfg(unix)]
    long: long::State,
}

/// Everything shared by all rows of a run.
pub struct RowFmt<'a> {
    tree: &'a Tree,
    color: bool,
    icons: bool,
    classify: bool,

    /// The width to cut rows at, if they are cut.
    truncate_to: Option<usize>,
    size: Size,
    red: Esc,

    #[cfg(unix)]
    long: Option<long::Long>,
}

impl<'a> RowFmt<'a> {
    /// The row format `ctx` selects for `tree`.
    pub fn new(tree: &'a Tree, ctx: &Context) -> Self {
        let color = !ctx.no_color();

        Self {
            tree,
            color,
            icons: ctx.icons,
            classify: ctx.classify,
            truncate_to: ctx.window_width.filter(|_| ctx.truncate),
            size: Size::new(ctx, color),
            red: Esc::new(nu_ansi_term::Color::Red.normal()),
            #[cfg(unix)]
            long: ctx.long.then(|| long::Long::new(ctx)),
        }
    }

    /// Appends the row of node `id` of a tree layout. The prefix is appended by `prefix`, which
    /// is given the cursor of `scratch`.
    pub fn tree_row(
        &self,
        scratch: &mut Scratch,
        id: u32,
        out: &mut Vec<u8>,
        prefix: impl FnOnce(&mut prefix::Cursor, &mut Vec<u8>),
    ) {
        let start = out.len();
        let node = self.tree.node(id);

        #[cfg(unix)]
        if let Some(long) = &self.long {
            long.write(&mut scratch.long, node, out);
            out.push(b' ');
        }

        self.write_size(node, out);
        out.push(b' ');
        prefix(&mut scratch.cursor, out);

        if self.icons {
            self.write_icon(scratch, node, out);
            out.push(b' ');
        }

        self.write_name(scratch, node, out);
        self.end_row(out, start);
    }

    /// Appends the row of node `id` of a flat layout.
    pub fn flat_row(&self, scratch: &mut Scratch, id: u32, out: &mut Vec<u8>) {
        let start = out.len();
        let node = self.tree.node(id);

        #[cfg(unix)]
        if let Some(long) = &self.long {
            long.write(&mut scratch.long, node, out);

            // Three spaces, unless there is no size column.
            let mark = out.len();
            out.extend_from_slice(b"   ");
            let sized = out.len();
            self.write_size(node, out);
            if out.len() == sized {
                out.truncate(mark);
            }
            out.push(b' ');
            return self.finish_flat(scratch, id, node, out, start);
        }

        let sized = out.len();
        self.write_size(node, out);
        if out.len() != sized {
            out.extend_from_slice(b"   ");
        }

        self.finish_flat(scratch, id, node, out, start);
    }

    /// The path column of a flat row, and the end of the row.
    fn finish_flat(
        &self,
        scratch: &mut Scratch,
        id: u32,
        node: Node<'_>,
        out: &mut Vec<u8>,
        start: usize,
    ) {
        if self.icons {
            self.write_icon(scratch, node, out);
            out.push(b' ');
        }

        let style = node.style();
        if let Some(style) = style {
            out.extend_from_slice(&scratch.styles.get(style).plain.pre);
        }

        self.write_relative_path(&mut scratch.ancestors, id, out);

        if let Some(style) = style {
            out.extend_from_slice(&scratch.styles.get(style).plain.suf);
        }

        if self.classify {
            out.extend_from_slice(node.indicator().unwrap_or_default().as_bytes());
        }

        self.end_row(out, start);
    }

    /// Ends the row that starts at `start`: cuts it to the window if asked to, and adds the
    /// newline.
    fn end_row(&self, out: &mut Vec<u8>, start: usize) {
        if let Some(width) = self.truncate_to {
            let row = String::from_utf8_lossy(&out[start..]);
            let cut = <str as Escaped>::truncate(&row, width);
            out.truncate(start);
            out.extend_from_slice(cut.as_bytes());
        }

        out.push(b'\n');
    }

    /// The path of `node` below the root, or the name of the root for the root. Components are
    /// converted one by one, which is the same as converting the whole path.
    fn write_relative_path(&self, ancestors: &mut Vec<u32>, id: u32, out: &mut Vec<u8>) {
        let tree = self.tree;
        let root = tree.root();
        if id == root {
            push_lossy(out, tree.node(id).file_name());
            return;
        }

        ancestors.clear();
        let mut cur = id;
        while cur != root && cur != NONE {
            ancestors.push(cur);
            cur = tree.parent(cur);
        }

        for (k, &ancestor) in ancestors.iter().rev().enumerate() {
            if k > 0 {
                out.push(MAIN_SEPARATOR as u8);
            }
            push_lossy(out, tree.node(ancestor).file_name());
        }
    }

    /// The icon of `node` in its colors.
    fn write_icon(&self, scratch: &mut Scratch, node: Node<'_>, out: &mut Vec<u8>) {
        let name = node.file_name();
        let file_type = node.file_type();
        let ext_source = node
            .symlink_target_path()
            .unwrap_or_else(|| Path::new(name));
        let (glyph, paint) =
            icons::fs::lookup(file_type.is_dir(), file_type.is_symlink(), ext_source, name);

        if !self.color {
            out.extend_from_slice(glyph.as_bytes());
            return;
        }

        match paint {
            Paint::Fixed(code) => icons::fs::push_fixed(out, code, glyph),
            Paint::FileStyle => match node.style().filter(|style| !style.is_plain()) {
                Some(style) => {
                    let painter = scratch.styles.get(style);
                    out.extend_from_slice(&painter.bold.pre);
                    out.extend_from_slice(glyph.as_bytes());
                    out.extend_from_slice(&painter.bold.suf);
                },
                None => out.extend_from_slice(glyph.as_bytes()),
            },
        }
    }

    /// The name of `node` in its style, followed by the indicator and, for a symlink, its target.
    fn write_name(&self, scratch: &mut Scratch, node: Node<'_>, out: &mut Vec<u8>) {
        let name = node.file_name();
        let style = node.style();
        let ind = if self.classify {
            node.indicator().unwrap_or_default().as_bytes()
        } else {
            b""
        };

        let Some(target) = node.symlink_target_file_name() else {
            if let Some(style) = style.filter(|style| !style.is_plain()) {
                let bold = &scratch.styles.get(style).bold;
                out.extend_from_slice(&bold.pre);
                push_lossy(out, name);
                out.extend_from_slice(&bold.suf);
            } else {
                push_lossy(out, name);
            }

            out.extend_from_slice(ind);
            return;
        };

        if let Some(style) = style {
            let plain = &scratch.styles.get(style).plain;
            out.extend_from_slice(&plain.pre);
            push_lossy(out, name);
            out.extend_from_slice(&plain.suf);
            out.extend_from_slice(ind);
            out.extend_from_slice(b" ");
            out.extend_from_slice(&self.red.pre);
            out.extend_from_slice("\u{2192} ".as_bytes());
            push_lossy(out, target);
            out.extend_from_slice(&self.red.suf);
        } else {
            push_lossy(out, name);
            out.extend_from_slice(ind);
            out.extend_from_slice(" \u{2192} ".as_bytes());
            push_lossy(out, target);
        }
    }

    /// The size column of `node`; nothing at all if rows without sizes have no column.
    fn write_size(&self, node: Node<'_>, out: &mut Vec<u8>) {
        let size = &self.size;

        let Some(file_size) = node.file_size() else {
            if let Some(padding) = size.placeholder {
                let esc = size.placeholder_esc.as_ref();
                out.extend_from_slice(esc.map_or(&[][..], |e| &e.pre));
                push_spaces(out, padding.saturating_sub(1));
                out.extend_from_slice(styles::PLACEHOLDER.as_bytes());
                out.extend_from_slice(esc.map_or(&[][..], |e| &e.suf));
            }
            return;
        };

        // The unit colors are present exactly with color.
        let paint = |index: usize| size.du.as_ref().map(|du| &du[index]);

        match &file_size {
            FileSize::Byte(metric) => {
                let mut buf = [0; 32];
                let (number, unit) = metric.parts(&mut buf);
                let esc = paint(if metric.human_readable {
                    unit_index(unit)
                } else {
                    size.unit.index_of(metric.value)
                });

                out.extend_from_slice(esc.map_or(&[][..], |e| &e.pre));
                push_str_padded(out, number, size.width);
                out.push(b' ');
                push_str_padded(out, unit, size.unit_width);
                out.extend_from_slice(esc.map_or(&[][..], |e| &e.suf));
            },
            FileSize::Line(_) | FileSize::Word(_) => {
                self.write_unitless(file_size.value(), paint(0), out);
            },

            #[cfg(unix)]
            FileSize::Block(metric) => {
                let bytes = metric.value * u64::from(BLOCK_SIZE_BYTES);
                self.write_unitless(metric.value, paint(size.unit.index_of(bytes)), out);
            },
        }
    }

    /// A count right-aligned to the size column.
    fn write_unitless(&self, value: u64, esc: Option<&Esc>, out: &mut Vec<u8>) {
        out.extend_from_slice(esc.map_or(&[][..], |e| &e.pre));
        push_u64_padded(out, value, self.size.width);
        out.extend_from_slice(esc.map_or(&[][..], |e| &e.suf));
    }
}

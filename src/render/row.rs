//! Writes rows as bytes into a buffer. One [`RowFmt`] is built per run; everything that can be
//! decided before the first row (colors, widths, which columns exist) is decided there.

use super::{
    paths::DirPaths,
    prefix,
    theme::{Esc, StyleCache},
};
use crate::{
    ansi::Escaped,
    bytes::{push_fixed, push_lossy, push_spaces, push_str_padded, push_u64_padded},
    context::Context,
    disk_usage::{
        file_size::{DiskUsage, byte},
        units::{PREFIXES, PrefixKind},
    },
    icons::{self, fs::Paint},
    styles,
    tree::{Tree, node::Node, widths::Widths},
};

#[cfg(unix)]
use super::long;
#[cfg(unix)]
use crate::fs::stat::Stat;

/// How the size column is written. Rows without a size show the placeholder in it.
struct Size {
    width: usize,
    unit_width: usize,
    disk_usage: DiskUsage,
    human: bool,
    /// The unit of byte sizes; also picks the color of block counts, by the prefix of their
    /// size in bytes.
    unit: PrefixKind,

    placeholder_esc: Option<Esc>,

    /// Sequences of the unit colors from bytes up; `None` without color.
    du: Option<[Esc; PREFIXES]>,
}

impl Size {
    /// The size column of `ctx` with `widths`, if there is one: only if some row has a size.
    fn new(ctx: &Context, widths: &Widths) -> Option<Self> {
        if !widths.sized {
            return None;
        }
        let colors = styles::colors();

        Some(Self {
            width: widths.max_size_width,
            unit_width: widths.max_size_unit_width,
            disk_usage: ctx.disk_usage,
            human: ctx.human,
            unit: ctx.unit,
            placeholder_esc: colors.map(|colors| Esc::new(colors.placeholder)),
            du: colors.map(|colors| colors.du.map(Esc::new)),
        })
    }

    /// Width of the size column: the number, then a space and the unit for byte metrics.
    const fn column_width(&self) -> usize {
        if self.disk_usage.is_bytes() {
            self.width + 1 + self.unit_width
        } else {
            self.width
        }
    }

    /// The size of `node`, or the placeholder if it has none.
    fn write(&self, node: Node<'_>, out: &mut Vec<u8>) {
        let Some(value) = node.size_value() else {
            let esc = self.placeholder_esc.as_ref();
            out.extend_from_slice(esc.map_or(&[][..], |e| &e.pre));
            push_spaces(out, self.column_width() - 1);
            out.extend_from_slice(styles::PLACEHOLDER.as_bytes());
            out.extend_from_slice(esc.map_or(&[][..], |e| &e.suf));
            return;
        };

        // The unit colors are present exactly with color.
        let paint = |index: usize| self.du.as_ref().map(|du| &du[index]);

        if self.disk_usage.is_bytes() {
            let mut buf = [0; 32];
            let (number, unit, index) =
                byte::Metric::new(self.unit, self.human).parts(value, &mut buf);
            let esc = paint(index);

            out.extend_from_slice(esc.map_or(&[][..], |e| &e.pre));
            push_str_padded(out, number, self.width);
            out.push(b' ');
            push_str_padded(out, unit, self.unit_width);
            out.extend_from_slice(esc.map_or(&[][..], |e| &e.suf));
            return;
        }

        // Counts take the color of bytes, except blocks, which take that of their size in bytes.
        #[cfg(unix)]
        let index = if matches!(self.disk_usage, DiskUsage::Block) {
            self.unit.index_of(value * Stat::BLOCK_BYTES)
        } else {
            0
        };
        #[cfg(not(unix))]
        let index = 0;

        let esc = paint(index);
        out.extend_from_slice(esc.map_or(&[][..], |e| &e.pre));
        push_u64_padded(out, value, self.width);
        out.extend_from_slice(esc.map_or(&[][..], |e| &e.suf));
    }
}

/// Memory one thread needs to write rows: caches and scratch space. Contents carry no meaning
/// from one row to the next other than as caches.
#[derive(Default)]
pub struct Scratch {
    styles: StyleCache,
    pub cursor: prefix::Cursor,

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
    /// The size column; `None` if there is none.
    size: Option<Size>,
    red: Esc,

    #[cfg(unix)]
    long: Option<long::Long>,
}

impl<'a> RowFmt<'a> {
    /// The row format `ctx` selects for `tree`.
    pub fn new(tree: &'a Tree, ctx: &Context) -> Self {
        let color = styles::colors().is_some();

        Self {
            tree,
            color,
            icons: ctx.icons,
            classify: ctx.classify,
            truncate_to: ctx.truncate.then(crate::tty::get_window_width).flatten(),
            size: Size::new(ctx, tree.widths()),
            red: Esc::new(nu_ansi_term::Color::Red.normal()),
            #[cfg(unix)]
            long: ctx.long.then(|| long::Long::new(ctx, tree.widths())),
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

        if let Some(size) = &self.size {
            size.write(node, out);
        }
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
    pub fn flat_row(&self, scratch: &mut Scratch, paths: &DirPaths, id: u32, out: &mut Vec<u8>) {
        let start = out.len();
        let node = self.tree.node(id);

        #[cfg(unix)]
        if let Some(long) = &self.long {
            long.write(&mut scratch.long, node, out);

            if let Some(size) = &self.size {
                out.extend_from_slice(b"   ");
                size.write(node, out);
            }
            out.push(b' ');
            return self.finish_flat(scratch, paths, id, node, out, start);
        }

        if let Some(size) = &self.size {
            size.write(node, out);
            out.extend_from_slice(b"   ");
        }

        self.finish_flat(scratch, paths, id, node, out, start);
    }

    /// The path column of a flat row, and the end of the row.
    fn finish_flat(
        &self,
        scratch: &mut Scratch,
        paths: &DirPaths,
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

        self.write_relative_path(paths, id, out);

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

    /// The path of `node` below the root, or the name of the root for the root: the path of its
    /// parent directory and its name.
    fn write_relative_path(&self, paths: &DirPaths, id: u32, out: &mut Vec<u8>) {
        let tree = self.tree;

        if id != tree.root() {
            out.extend_from_slice(paths.prefix(tree.parent(id)));
        }

        push_lossy(out, tree.node(id).file_name());
    }

    /// The icon of `node` in its colors.
    fn write_icon(&self, scratch: &mut Scratch, node: Node<'_>, out: &mut Vec<u8>) {
        let (glyph, paint) = icons::fs::lookup(
            node.file_type(),
            node.symlink_target_path(),
            node.file_name(),
        );

        if !self.color {
            out.extend_from_slice(glyph.as_bytes());
            return;
        }

        match paint {
            Paint::Fixed(code) => push_fixed(out, code, glyph),
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
}

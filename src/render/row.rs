//! Writes rows as bytes into a buffer. One [`RowFmt`] is built per run; everything that can be
//! decided before the first row (colors, widths, which columns exist) is decided there.

use super::{
    ansi,
    paint::{Esc, Painter, paint, placeholder},
    paths::DirPaths,
    prefix,
};
use crate::{
    bytes::{push_fixed, push_lossy, push_str_padded, push_u64_padded},
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
            paint(out, self.placeholder_esc.as_ref(), |out| {
                placeholder(out, self.column_width());
            });
            return;
        };

        // The unit colors are present exactly with color.
        let paint = |index: usize| self.du.as_ref().map(|du| &du[index]);

        if self.disk_usage.is_bytes() {
            let mut buf = [0; 32];
            let (number, unit, index) =
                byte::Metric::new(self.unit, self.human).parts(value, &mut buf);
            super::paint::paint(out, paint(index), |out| {
                push_str_padded(out, number, self.width);
                out.push(b' ');
                push_str_padded(out, unit, self.unit_width);
            });
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

        super::paint::paint(out, paint(index), |out| {
            push_u64_padded(out, value, self.width);
        });
    }
}

/// Memory one thread needs to write rows: caches and scratch space. Contents carry no meaning
/// from one row to the next other than as caches.
#[derive(Default)]
pub struct Scratch {
    cursor: prefix::Cursor,

    #[cfg(unix)]
    long: long::State,
}

/// Everything shared by all rows of a run.
pub struct RowFmt<'a> {
    tree: &'a Tree,
    painters: Vec<Painter>,
    icons: bool,
    classify: bool,

    /// The width to cut rows at, if they are cut.
    truncate_to: Option<usize>,
    /// The size column; `None` if there is none.
    size: Option<Size>,
    link_arrow: Option<Esc>,

    #[cfg(unix)]
    long: Option<long::Long>,
}

impl<'a> RowFmt<'a> {
    /// The row format `ctx` selects for `tree`.
    pub fn new(tree: &'a Tree, ctx: &Context) -> Self {
        Self {
            tree,
            painters: tree.styles().iter().copied().map(Painter::new).collect(),
            icons: ctx.icons,
            classify: ctx.classify,
            // The width of the terminal on stdout; `None` if stdout is not one.
            truncate_to: ctx
                .truncate
                .then(terminal_size::terminal_size)
                .flatten()
                .map(|(width, _)| usize::from(width.0)),
            size: Size::new(ctx, tree.widths()),
            link_arrow: styles::colors().map(|colors| Esc::new(colors.link_arrow)),
            #[cfg(unix)]
            long: ctx.long.then(|| long::Long::new(ctx, tree.widths())),
        }
    }

    /// Appends the row of node `id` of a tree layout, which has the root on top if `inverted`.
    pub fn tree_row(
        &self,
        scratch: &mut Scratch,
        id: u32,
        glyphs: &prefix::Themes,
        inverted: bool,
        out: &mut Vec<u8>,
    ) {
        let start = out.len();
        let node = self.tree.node(id);

        if self.write_long(scratch, node, out) {
            out.push(b' ');
        }

        if let Some(size) = &self.size {
            size.write(node, out);
        }
        out.push(b' ');
        scratch
            .cursor
            .write(self.tree, id, node.depth(), glyphs, inverted, out);

        if self.icons {
            self.write_icon(node, out);
            out.push(b' ');
        }

        self.write_name(node, out);
        self.end_row(out, start);
    }

    /// Appends the row of node `id` of a flat layout.
    pub fn flat_row(&self, scratch: &mut Scratch, paths: &DirPaths, id: u32, out: &mut Vec<u8>) {
        let start = out.len();
        let node = self.tree.node(id);

        if self.write_long(scratch, node, out) {
            if let Some(size) = &self.size {
                out.extend_from_slice(b"   ");
                size.write(node, out);
            }
            out.push(b' ');
            return self.finish_flat(paths, id, node, out, start);
        }

        if let Some(size) = &self.size {
            size.write(node, out);
            out.extend_from_slice(b"   ");
        }

        self.finish_flat(paths, id, node, out, start);
    }

    /// Appends the long view's columns of `node`; false, appending nothing, outside the long
    /// view.
    #[cfg(unix)]
    fn write_long(&self, scratch: &mut Scratch, node: Node<'_>, out: &mut Vec<u8>) -> bool {
        let Some(long) = &self.long else {
            return false;
        };
        long.write(&mut scratch.long, node, out);
        true
    }

    /// The long view exists only on unix.
    #[cfg(not(unix))]
    #[expect(clippy::unused_self, reason = "same signature as the unix method")]
    const fn write_long(&self, _: &mut Scratch, _: Node<'_>, _: &mut Vec<u8>) -> bool {
        false
    }

    /// The path column of a flat row, and the end of the row.
    fn finish_flat(
        &self,
        paths: &DirPaths,
        id: u32,
        node: Node<'_>,
        out: &mut Vec<u8>,
        start: usize,
    ) {
        if self.icons {
            self.write_icon(node, out);
            out.push(b' ');
        }

        paint(out, self.painter(node).map(|p| &p.plain), |out| {
            self.write_relative_path(paths, id, out);
        });

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
            let cut = ansi::truncate(&row, width);
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
    fn write_icon(&self, node: Node<'_>, out: &mut Vec<u8>) {
        let (glyph, icon_paint) = icons::fs::lookup(
            node.file_type(),
            node.symlink_target_path(),
            node.file_name(),
        );

        let Some(painter) = self.painter(node) else {
            out.extend_from_slice(glyph.as_bytes());
            return;
        };

        match icon_paint {
            Paint::Fixed(code) => push_fixed(out, code, glyph),
            Paint::FileStyle => paint(out, Some(&painter.bold), |out| {
                out.extend_from_slice(glyph.as_bytes());
            }),
        }
    }

    /// The name of `node` in its style, followed by the indicator and, for a symlink, its target.
    fn write_name(&self, node: Node<'_>, out: &mut Vec<u8>) {
        let name = node.file_name();
        let painter = self.painter(node);
        let ind = if self.classify {
            node.indicator().unwrap_or_default().as_bytes()
        } else {
            b""
        };

        let Some(target) = node.symlink_target_path().map(std::path::Path::as_os_str) else {
            paint(out, painter.map(|p| &p.bold), |out| push_lossy(out, name));

            out.extend_from_slice(ind);
            return;
        };

        if let Some(painter) = painter {
            paint(out, Some(&painter.plain), |out| push_lossy(out, name));
            out.extend_from_slice(ind);
            out.push(b' ');
            paint(out, self.link_arrow.as_ref(), |out| {
                out.extend_from_slice("\u{2192} ".as_bytes());
                push_lossy(out, target);
            });
        } else {
            push_lossy(out, name);
            out.extend_from_slice(ind);
            out.extend_from_slice(" \u{2192} ".as_bytes());
            push_lossy(out, target);
        }
    }

    fn painter(&self, node: Node<'_>) -> Option<&Painter> {
        node.style().map(|index| &self.painters[index.index()])
    }
}

use crate::{Context, ansi::Escaped, tree::node::Node};
use cell::Cell;
use std::{
    fmt::{self, Display},
    marker::PhantomData,
};

#[cfg(unix)]
use super::long;

/// Concerned with rules to construct and a single cell in a given row.
pub mod cell;

pub struct Row<'a, T> {
    prefix: Option<&'a str>,
    ctx: &'a Context,
    node: &'a Node,
    layout: PhantomData<T>,
}

/// For both the [`super::Regular`] and [`super::Inverted`] layout variants.
pub struct Tree;

/// For the [`super::Flat`] variant.
pub struct Flat;

/// Joins the size and path columns of a flat row. The size column is omitted entirely, with no
/// leading or separating whitespace, when it renders empty (e.g. `--suppress-size`).
fn flat_row(size: &str, path: &str) -> String {
    if size.is_empty() {
        path.to_string()
    } else {
        format!("{size}   {path}")
    }
}

impl<'a, T> Row<'a, T> {
    pub const fn new(node: &'a Node, ctx: &'a Context, prefix: Option<&'a str>) -> Self {
        Self {
            prefix,
            node,
            ctx,
            layout: PhantomData,
        }
    }
}

#[cfg(unix)]
impl Display for Row<'_, Tree> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self.node;
        let ctx = self.ctx;

        let size = Cell::new(node, ctx, cell::Kind::FileSize);
        let name = Cell::new(
            node,
            ctx,
            cell::Kind::FileName {
                prefix: self.prefix,
            },
        );

        let row = if ctx.long {
            let optionals = long::Optionals::from(ctx);
            let long_display = long::Display::new(optionals, node, ctx);

            format!("{long_display} {size} {name}")
        } else {
            format!("{size} {name}")
        };

        if let (true, Some(window_width)) = (ctx.truncate, ctx.window_width) {
            let out = <str as Escaped>::truncate(&row, window_width);
            write!(f, "{out}")
        } else {
            write!(f, "{row}")
        }
    }
}

#[cfg(unix)]
impl Display for Row<'_, Flat> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self.node;
        let ctx = self.ctx;

        let size = Cell::new(node, ctx, cell::Kind::FileSize);
        let path = Cell::new(node, ctx, cell::Kind::FilePath);

        let row = if ctx.long {
            let optionals = long::Optionals::from(ctx);
            let long_display = long::Display::new(optionals, node, ctx);

            let size = size.to_string();

            if size.is_empty() {
                format!("{long_display}   {path}")
            } else {
                format!("{long_display}   {size} {path}")
            }
        } else {
            flat_row(&size.to_string(), &path.to_string())
        };

        if let (true, Some(window_width)) = (ctx.truncate, ctx.window_width) {
            let out = <str as Escaped>::truncate(&row, window_width);
            write!(f, "{out}")
        } else {
            write!(f, "{row}")
        }
    }
}

#[cfg(not(unix))]
impl Display for Row<'_, Tree> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self.node;
        let ctx = self.ctx;

        let size = Cell::new(node, ctx, cell::Kind::FileSize);
        let name = Cell::new(
            node,
            ctx,
            cell::Kind::FileName {
                prefix: self.prefix,
            },
        );

        let row = format!("{size} {name}");

        if let Some(window_width) = ctx.window_width.filter(|_| ctx.truncate) {
            let out = <str as Escaped>::truncate(&row, window_width);
            write!(f, "{out}")
        } else {
            write!(f, "{row}")
        }
    }
}

#[cfg(not(unix))]
impl Display for Row<'_, Flat> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self.node;
        let ctx = self.ctx;

        let size = Cell::new(node, ctx, cell::Kind::FileSize);
        let path = Cell::new(node, ctx, cell::Kind::FilePath);

        let row = flat_row(&size.to_string(), &path.to_string());

        if let Some(window_width) = ctx.window_width.filter(|_| ctx.truncate) {
            let out = <str as Escaped>::truncate(&row, window_width);
            write!(f, "{out}")
        } else {
            write!(f, "{row}")
        }
    }
}

#![warn(
    clippy::all,
    clippy::cargo,
    clippy::complexity,
    clippy::correctness,
    clippy::nursery,
    clippy::pedantic,
    clippy::perf,
    clippy::style,
    clippy::suspicious
)]
#![allow(
    clippy::cast_precision_loss,
    clippy::struct_excessive_bools,
    // Shared prefixes like `max_*` and `num_*` are deliberate field names.
    clippy::struct_field_names,
    clippy::wildcard_imports
)]

use clap::CommandFactory;
use context::{Context, layout};
use progress::{Indicator, IndicatorHandle};
use render::{Engine, Flat, FlatInverted, Inverted, Regular};
use std::{
    error::Error,
    io::{self, BufWriter, ErrorKind, IsTerminal, Write, stdout},
    process::ExitCode,
};
use tree::{Tree, count::FileCount};

/// Operations to wrangle ANSI escaped strings.
mod ansi;

/// CLI rules and definitions as well as context to be injected throughout the entire program.
mod context;

/// Operations relevant to the computation and presentation of disk usage.
mod disk_usage;

/// Filesystem operations.
mod fs;

/// All things related to icons on how to map certain files to the appropriate icons.
mod icons;

/// Concerned with displaying a progress indicator when stdout is a tty.
mod progress;

/// Concerned with taking an initialized [`tree::Tree`] and its [`tree::node::Node`]s and rendering the output.
mod render;

/// Global used throughout the program to paint the output.
mod styles;

/// Houses the primary data structures that are used to virtualize the filesystem, containing also
/// information on how the tree output should be ultimately rendered.
mod tree;

/// Utilities relating to interacting with tty properties.
mod tty;

/// Common utilities across all modules.
mod utils;

/// Parallel filesystem walk.
mod walk;

fn main() -> ExitCode {
    if let Err(e) = run() {
        // A closed stdout (e.g. `erd | head`) is not an error worth reporting.
        if let Some(io_err) = e.downcast_ref::<io::Error>()
            && io_err.kind() == ErrorKind::BrokenPipe
        {
            return ExitCode::SUCCESS;
        }

        // Usage errors keep clap's own formatting and exit status.
        if let Some(context::error::Error::ArgParse(clap_err)) = e.downcast_ref() {
            clap_err.exit();
        }

        eprintln!("erd: {e}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

fn run() -> Result<(), Box<dyn Error>> {
    // Color is only ever used on a terminal unless forced; parse the theme while clap works.
    if stdout().is_terminal() && std::env::var_os("LS_COLORS").is_some_and(|v| !v.is_empty()) {
        styles::prefetch_ls_colors();
    }

    let ctx = Context::try_init()?;

    if let Some(shell) = ctx.completions {
        clap_complete::generate(shell, &mut Context::command(), "erd", &mut stdout());
        return Ok(());
    }

    styles::init(ctx.no_color());

    let indicator = Indicator::maybe_init(&ctx);
    let no_report = ctx.no_report;
    // `--summarize` prints one line per root, like `du -s a b`.
    let separator = if ctx.summarize { "" } else { "\n" };
    // At level 0 (and so with `--summarize`) only the roots are requested, so a root without
    // visible entries is still worth printing.
    let roots_only = ctx.level() == 0;

    let mut trees = vec![];

    // On an error return, dropping `indicator` restores the terminal before the error is printed.
    for ctx in ctx.split_roots() {
        let tree = Tree::try_init(&ctx, indicator.as_ref().map(IndicatorHandle::indexed))?;
        trees.push((tree, ctx));
    }

    if !roots_only && trees.iter().all(|(tree, _)| tree.is_stump()) {
        return Err(Box::new(tree::error::Error::NoMatches));
    }

    let file_count = trees.iter().fold(FileCount::default(), |acc, (tree, _)| {
        acc + tree.file_count()
    });

    // Rows are streamed to stdout, so the indicator must be gone first.
    if let Some(progress) = indicator {
        progress.finish()?;
    }

    #[cfg(debug_assertions)]
    let should_write = std::env::var_os("ERDTREE_DEBUG").is_none();

    #[cfg(not(debug_assertions))]
    let should_write = true;

    let mut out: Box<dyn Write> = if should_write {
        Box::new(BufWriter::with_capacity(1 << 16, stdout().lock()))
    } else {
        Box::new(io::sink())
    };

    // Every rendered tree ends in a newline, so the "\n" separator leaves a blank line between
    // trees.
    let roots = trees.len();

    for (k, (tree, ctx)) in trees.into_iter().enumerate() {
        if k > 0 {
            out.write_all(separator.as_bytes())?;
        }
        render(tree, ctx, &mut out, k + 1 == roots)?;
    }

    let file_count = file_count.to_string();

    // The count is empty when no tree has any entries (only possible at level 0).
    if !no_report && !file_count.is_empty() {
        write!(out, "\n{file_count}\n")?;
    }

    out.flush()?;

    Ok(())
}

/// Writes the rows of `tree` in the layout selected by `ctx`. With `last`, the tree is not
/// freed: the process ends soon after, and returning its memory to the allocator page by page
/// is wasted time.
fn render(tree: Tree, ctx: Context, out: &mut impl Write, last: bool) -> io::Result<()> {
    fn write<T: render::Layout>(
        tree: Tree,
        ctx: Context,
        out: &mut impl Write,
        last: bool,
    ) -> io::Result<()> {
        let engine = Engine::<T>::new(tree, ctx);
        let written = engine.write(out);

        if last {
            std::mem::forget(engine);
        }

        written
    }

    match ctx.layout {
        layout::Type::Flat => write::<Flat>(tree, ctx, out, last),
        layout::Type::Iflat => write::<FlatInverted>(tree, ctx, out, last),
        layout::Type::Inverted => write::<Inverted>(tree, ctx, out, last),
        layout::Type::Regular => write::<Regular>(tree, ctx, out, last),
    }
}

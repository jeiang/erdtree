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
use context::Context;
use progress::{Indicator, IndicatorHandle};
use std::{
    error::Error,
    io::{self, BufWriter, ErrorKind, IsTerminal, Write, stdout},
    process::ExitCode,
};
use tree::{Tree, count::FileCount};

mod bytes;
mod context;
mod disk_usage;
mod fs;
mod icons;
mod par;
mod progress;
mod render;
mod styles;
mod sync;
mod tree;
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
    // Color is only ever used on a terminal unless forced; parse `LS_COLORS` while clap works.
    let prefetched = (stdout().is_terminal()
        && std::env::var_os("LS_COLORS").is_some_and(|v| !v.is_empty()))
    .then(styles::prefetch_ls_colors);

    let (ctx, roots) = Context::try_init()?;

    if let Some(shell) = ctx.completions {
        clap_complete::generate(shell, &mut Context::command(), "erd", &mut stdout());
        return Ok(());
    }

    let theme = styles::init(ctx.no_color(), prefetched);

    let indicator = Indicator::maybe_init(&ctx);
    let no_report = ctx.no_report;
    // `--summarize` prints one line per root, like `du -s a b`. Every rendered tree ends in a
    // newline, so "\n" leaves a blank line between trees.
    let separator = if ctx.summarize { "" } else { "\n" };
    // At level 0 (and so with `--summarize`) only the roots are requested, so a root without
    // visible entries is still worth printing.
    let roots_only = ctx.level() == 0;

    let mut trees = vec![];

    // On an error return, dropping `indicator` restores the terminal before the error is printed.
    for root in &roots {
        let progress = indicator.as_ref().map(IndicatorHandle::indexed);
        trees.push(Tree::try_init(&ctx, root, theme.colors.as_ref(), progress)?);
    }

    if !roots_only && trees.iter().all(Tree::is_stump) {
        return Err(Box::new(tree::error::Error::NoMatches));
    }

    let file_count = trees
        .iter()
        .fold(FileCount::default(), |acc, tree| acc + tree.file_count());

    // Rows are streamed to stdout, so the indicator must be gone first.
    if let Some(progress) = indicator {
        progress.finish()?;
    }

    let mut out = BufWriter::with_capacity(1 << 16, stdout().lock());

    let count = trees.len();

    for (k, tree) in trees.into_iter().enumerate() {
        if k > 0 {
            out.write_all(separator.as_bytes())?;
        }
        let written = render::write(&tree, &ctx, theme, &mut out);

        // The last tree is not freed: the process ends soon after, and returning its memory to
        // the allocator page by page is wasted time.
        if k + 1 == count {
            std::mem::forget(tree);
        }
        written?;
    }

    // The count is empty when no tree has any entries (only possible at level 0).
    if !no_report && !file_count.is_empty() {
        write!(out, "\n{file_count}\n")?;
    }

    out.flush()?;

    Ok(())
}

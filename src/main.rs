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
use progress::{Indicator, IndicatorHandle, Message};
use render::{Engine, Flat, FlatInverted, Inverted, Regular};
use std::{
    error::Error,
    io::{self, ErrorKind, Write, stdout},
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
    let ctx = Context::try_init()?;

    if let Some(shell) = ctx.completions {
        clap_complete::generate(shell, &mut Context::command(), "erd", &mut stdout());
        return Ok(());
    }

    styles::init(ctx.no_color());

    let indicator = Indicator::maybe_init(&ctx)?;
    let no_report = ctx.no_report;
    // `--summarize` prints one line per root, like `du -s a b`.
    let separator = if ctx.summarize { "" } else { "\n" };

    let mut trees = vec![];

    for ctx in ctx.split_roots() {
        match Tree::try_init(ctx, indicator.as_ref()) {
            Ok(res) => trees.push(res),
            Err(err) => {
                let _ = IndicatorHandle::try_terminate(indicator);
                return Err(Box::new(err));
            },
        }
    }

    if trees.iter().all(|(tree, _)| tree.is_stump()) {
        let _ = IndicatorHandle::try_terminate(indicator);
        return Err(Box::new(tree::error::Error::NoMatches));
    }

    if let Some(progress) = &indicator {
        progress
            .mailbox()
            .send(Message::DoneIndexing)
            .map_err(|_e| tree::error::Error::Terminated)?;
    }

    let file_count = trees.iter().fold(FileCount::default(), |acc, (tree, _)| {
        acc + tree.file_count()
    });

    // Every rendered tree ends in a newline, so the "\n" separator leaves a blank line between
    // trees.
    let mut output = trees
        .into_iter()
        .map(|(tree, ctx)| render(tree, ctx))
        .collect::<Vec<_>>()
        .join(separator);

    if !no_report {
        output = format!("{output}\n{file_count}\n");
    }

    if let Some(mut progress) = indicator {
        progress
            .mailbox()
            .send(Message::RenderReady)
            .map_err(|_e| tree::error::Error::Terminated)?;

        progress.join()?;
    }

    #[cfg(debug_assertions)]
    let should_write = std::env::var_os("ERDTREE_DEBUG").is_none();

    #[cfg(not(debug_assertions))]
    let should_write = true;

    if should_write {
        let mut out = stdout().lock();
        write!(out, "{output}")?;
        out.flush()?;
    }

    Ok(())
}

/// Renders the rows of `tree` in the layout selected by `ctx`.
fn render(tree: Tree, ctx: Context) -> String {
    match ctx.layout {
        layout::Type::Flat => Engine::<Flat>::new(tree, ctx).to_string(),
        layout::Type::Iflat => Engine::<FlatInverted>::new(tree, ctx).to_string(),
        layout::Type::Inverted => Engine::<Inverted>::new(tree, ctx).to_string(),
        layout::Type::Regular => Engine::<Regular>::new(tree, ctx).to_string(),
    }
}

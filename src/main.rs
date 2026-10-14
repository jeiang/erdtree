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
use tree::Tree;

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

    let (tree, ctx) = {
        match Tree::try_init(ctx, indicator.as_ref()) {
            Ok(res) => res,
            Err(err) => {
                let _ = IndicatorHandle::try_terminate(indicator);
                return Err(Box::new(err));
            },
        }
    };

    macro_rules! compute_output {
        ($t:ty) => {{
            let render = Engine::<$t>::new(tree, ctx);
            format!("{render}")
        }};
    }

    let output = match ctx.layout {
        layout::Type::Flat => compute_output!(Flat),
        layout::Type::Iflat => compute_output!(FlatInverted),
        layout::Type::Inverted => compute_output!(Inverted),
        layout::Type::Regular => compute_output!(Regular),
    };

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
        // Rows end in a newline; only the summary line (absent with `--no-report`) does not.
        if output.ends_with('\n') {
            write!(out, "{output}")?;
        } else {
            writeln!(out, "{output}")?;
        }
        out.flush()?;
    }

    Ok(())
}

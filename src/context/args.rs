use super::{Context, config, error::Error};
use clap::{
    ArgGroup, ArgMatches, Command, CommandFactory, FromArgMatches, builder::ArgAction,
    parser::ValueSource,
};
use std::{
    ffi::{OsStr, OsString},
    path::PathBuf,
};

/// Allows the implementor to compute [`ArgMatches`] that reconciles arguments from both the
/// command-line as well as the config file that gets loaded.
pub trait Reconciler: CommandFactory + FromArgMatches {
    /// Loads in arguments from both the command-line as well as the config file and reconciles
    /// identical arguments between the two using these rules:
    ///
    /// 1. If no config file is present, use arguments strictly from the command-line.
    /// 2. If an argument was provided via the CLI then override the argument from the config.
    /// 3. If an argument is sourced from its default value because a user didn't provide it via
    ///    the CLI, then select the argument from the config if it exists.
    fn compute_args() -> Result<ArgMatches, Error> {
        let cmd = Self::command().args_override_self(true);

        let user_args = Command::clone(&cmd).get_matches();

        if user_args.get_one::<bool>("no_config").is_some_and(|b| *b) {
            return validate(&cmd, std::env::args_os().skip(1));
        }

        let maybe_config_args = {
            let named_table = user_args.get_one::<String>("config");

            if let Some(rc) = load_rc_config_args() {
                if named_table.is_some() {
                    return Err(Error::Rc);
                }

                Some(rc?)
            } else {
                let toml = load_toml_config_args(named_table.map(String::as_str))?;

                if named_table.is_some() && toml.is_none() {
                    return Err(Error::NoToml);
                }

                toml
            }
        };

        let Some(config_args) = maybe_config_args else {
            return validate(&cmd, std::env::args_os().skip(1));
        };

        let mut final_args = init_empty_args();

        for arg in cmd.get_arguments() {
            let arg_id = arg.get_id();
            let id_str = arg_id.as_str();

            if id_str == "dir" {
                if let Some(dir) = user_args.try_get_one::<PathBuf>(id_str)? {
                    final_args.push(OsString::from(dir));
                }
                continue;
            }

            let argument_source = user_args
                .value_source(id_str)
                .map_or(&config_args, |source| {
                    if source == ValueSource::CommandLine {
                        &user_args
                    } else {
                        &config_args
                    }
                });

            let Some(key) = arg.get_long().map(|l| format!("--{l}")).map(OsString::from) else {
                continue;
            };

            match arg.get_action() {
                ArgAction::SetTrue => {
                    if argument_source
                        .try_get_one::<bool>(id_str)?
                        .is_some_and(|b| *b)
                    {
                        final_args.push(key);
                    }
                },
                ArgAction::SetFalse => {},
                ArgAction::Append => {
                    // Repeatable options need the key before every value.
                    let Ok(Some(raw)) = argument_source.try_get_raw(id_str) else {
                        continue;
                    };
                    for value in raw {
                        final_args.push(key.clone());
                        final_args.push(value.to_os_string());
                    }
                },
                _ => {
                    let Ok(Some(raw)) = argument_source.try_get_raw(id_str) else {
                        continue;
                    };
                    final_args.push(key);
                    final_args.extend(raw.map(OsStr::to_os_string));
                },
            }
        }

        validate(&cmd, final_args.into_iter().skip(1))
    }
}

impl Reconciler for Context {}

/// Adds the relations between arguments (`requires` and mutual exclusion) to `cmd`.
///
/// These are deliberately not declared on [`Context`] itself: the command-line and the config
/// file are each parsed on their own before being merged, and neither is complete without the
/// other (e.g. `glob = true` in the config with `--pattern` on the command-line). Relations are
/// therefore only enforced by [`validate`], once, on the merged arguments.
fn with_relations(cmd: Command) -> Command {
    // These arguments only exist on Unix; `mut_arg` panics on unknown ids.
    #[cfg(unix)]
    let cmd = cmd
        .mut_arg("octal", |a| a.requires("long"))
        .mut_arg("time", |a| a.requires("long"))
        .mut_arg("time_format", |a| a.requires("long"));

    cmd.mut_arg("glob", |a| a.requires("pattern"))
        .mut_arg("iglob", |a| a.requires("pattern"))
        .mut_arg("file_type", |a| a.requires("pattern"))
        .mut_arg("no_git", |a| a.requires("hidden"))
        .mut_arg("min_size", |a| a.conflicts_with("suppress_size"))
        .mut_arg("report_ignored", |a| a.conflicts_with("pattern"))
        .group(ArgGroup::new("searching").args(["glob", "iglob"]))
}

/// Parses `args` (without the binary name) against `cmd` with all argument relations enforced.
/// The real binary name is prepended so usage errors read `Usage: erd ...`.
fn validate(cmd: &Command, args: impl IntoIterator<Item = OsString>) -> Result<ArgMatches, Error> {
    let bin = std::env::args_os()
        .next()
        .unwrap_or_else(|| OsString::from("erd"));
    let full = std::iter::once(bin).chain(args);
    with_relations(cmd.clone())
        .try_get_matches_from(full)
        .map_err(Error::ArgParse)
}

/// Creates a properly formatted `Vec<OsString>` that [`clap::Command`] would understand.
#[inline]
fn init_empty_args() -> Vec<OsString> {
    vec![OsString::from("--")]
}

/// Loads an [`ArgMatches`] from `.erdtreerc`.
#[inline]
fn load_rc_config_args() -> Option<Result<ArgMatches, Error>> {
    config::rc::read_config_to_string().map(|rc_config| {
        let parsed_args = config::rc::parse(&rc_config);
        Context::command()
            .try_get_matches_from(parsed_args)
            .map_err(Error::Config)
    })
}

/// Loads an [`ArgMatches`] from `.erdtree.toml`, or `None` if there is no such file.
#[inline]
fn load_toml_config_args(named_table: Option<&str>) -> Result<Option<ArgMatches>, Error> {
    let Some(toml_config) = config::toml::load()? else {
        return Ok(None);
    };
    let parsed_args = config::toml::parse(toml_config, named_table)?;
    let config_args = Context::command()
        .try_get_matches_from(parsed_args)
        .map_err(Error::Config)?;

    Ok(Some(config_args))
}

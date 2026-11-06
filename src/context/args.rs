use super::{Context, config, error::Error};
use clap::{
    ArgGroup, ArgMatches, Command, CommandFactory, builder::ArgAction, parser::ValueSource,
};
use std::{
    ffi::{OsStr, OsString},
    path::PathBuf,
};

/// Loads in arguments from both the command-line as well as the config file and reconciles
/// identical arguments between the two using these rules:
///
/// 1. If no config file is present, use arguments strictly from the command-line.
/// 2. If an argument was provided via the CLI then override the argument from the config.
/// 3. If an argument is sourced from its default value because a user didn't provide it via
///    the CLI, then select the argument from the config if it exists.
pub fn compute_args() -> Result<ArgMatches, Error> {
    // Fast path: parse once with the relations enforced. Success means the plain parse of the
    // slow path would succeed too (relations only add constraints) with identical values. On
    // any error, help or version request, take the slow path so that output, error order and
    // exit status stay exactly as they were.
    let mut cmd = with_relations(Context::command().args_override_self(true));

    match cmd.try_get_matches_from_mut(std::env::args_os()) {
        Ok(user_args) => return reconcile(cmd, user_args, true),
        // Relations do not change help or version text.
        Err(err) if !err.use_stderr() => err.exit(),
        Err(_) => {},
    }

    // Exits for usage errors, like `Command::get_matches`.
    let user_args = Context::command()
        .args_override_self(true)
        .try_get_matches_from(std::env::args_os())
        .unwrap_or_else(|err| err.exit());

    // A command that has parsed once cannot take `mut_arg`; build another for the relations.
    let cmd = with_relations(Context::command().args_override_self(true));

    reconcile(cmd, user_args, false)
}

/// Merges the config file, if any, into `user_args`, the matches of the command-line. `cmd` must
/// carry the relations. `validated` tells that `user_args` was parsed with `cmd` already.
fn reconcile(
    mut cmd: Command,
    user_args: ArgMatches,
    validated: bool,
) -> Result<ArgMatches, Error> {
    if user_args.get_one::<bool>("no_config").is_some_and(|b| *b) {
        return cli_only(&mut cmd, user_args, validated);
    }

    let named_table = user_args.get_one::<String>("config").map(String::as_str);

    let Some(config_args) = load_toml_config_args(named_table)? else {
        if named_table.is_some() {
            return Err(Error::NoToml);
        }
        return cli_only(&mut cmd, user_args, validated);
    };

    let final_args = merge(&cmd, &user_args, &config_args)?;
    validate(&mut cmd, final_args)
}

/// The arguments that `validate` parses: per argument, the command-line value if one was given,
/// else the config value. A value that is a default on both sides is left out, so it stays a
/// default in the merged matches.
fn merge(
    cmd: &Command,
    user_args: &ArgMatches,
    config_args: &ArgMatches,
) -> Result<Vec<OsString>, Error> {
    let mut final_args = Vec::new();

    for arg in cmd.get_arguments() {
        let arg_id = arg.get_id();
        let id_str = arg_id.as_str();

        if id_str == "dir" {
            if let Some(dirs) = user_args.try_get_many::<PathBuf>(id_str)? {
                final_args.extend(dirs.map(OsString::from));
            }
            continue;
        }

        let argument_source = if user_args.value_source(id_str) == Some(ValueSource::CommandLine) {
            user_args
        } else {
            config_args
        };

        // A default stays a default, so that the merged matches can tell that it was not given
        // (`Context::threads_explicit`).
        if argument_source.value_source(id_str) == Some(ValueSource::DefaultValue) {
            continue;
        }

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

    Ok(final_args)
}

/// The matches when no config applies: `user_args` as is when `cmd` parsed it already, else the
/// command-line parsed again with the relations enforced.
fn cli_only(
    cmd: &mut Command,
    user_args: ArgMatches,
    validated: bool,
) -> Result<ArgMatches, Error> {
    if validated {
        Ok(user_args)
    } else {
        validate(cmd, std::env::args_os().skip(1))
    }
}

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

/// Parses `args` (without the binary name) against `cmd`, which must carry the argument
/// relations. The real binary name is prepended so usage errors read `Usage: erd ...`.
fn validate(
    cmd: &mut Command,
    args: impl IntoIterator<Item = OsString>,
) -> Result<ArgMatches, Error> {
    let bin = std::env::args_os()
        .next()
        .unwrap_or_else(|| OsString::from("erd"));
    let full = std::iter::once(bin).chain(args);
    cmd.try_get_matches_from_mut(full).map_err(Error::ArgParse)
}

/// Loads an [`ArgMatches`] from `.erdtree.toml`, or `None` if there is no such file.
#[inline]
fn load_toml_config_args(named_table: Option<&str>) -> Result<Option<ArgMatches>, Error> {
    let Some(toml_config) = config::load()? else {
        return Ok(None);
    };
    let parsed_args = config::parse(toml_config, named_table)?;
    let config_args = Context::command()
        .try_get_matches_from(parsed_args)
        .map_err(Error::Config)?;

    Ok(Some(config_args))
}

#[cfg(test)]
mod test {
    use super::{Command, CommandFactory, Context, merge, validate, with_relations};
    use crate::context::sort;

    fn command() -> Command {
        Context::command().args_override_self(true)
    }

    /// The [`Context`] that `cli` on the command-line and `config` from a config file give.
    fn context(cli: &[&str], config: &[&str]) -> Context {
        let parse = |args: &[&str]| {
            command()
                .try_get_matches_from(std::iter::once(&"erd").chain(args))
                .unwrap()
        };
        let mut cmd = with_relations(command());
        let args = merge(&cmd, &parse(cli), &parse(config)).unwrap();
        let merged = validate(&mut cmd, args).unwrap();
        Context::from_merged(merged, false).unwrap()
    }

    #[test]
    fn threads_given_anywhere_are_explicit() {
        let neither = context(&[], &["--sort", "name"]);
        assert!(!neither.threads_explicit);
        assert_eq!(neither.threads, Context::num_threads());
        assert_eq!(neither.sort, sort::Type::Name, "the config applies");

        let config = context(&[], &["--threads", "2"]);
        assert!(config.threads_explicit);
        assert_eq!(config.threads, 2);

        let cli = context(&["--threads", "3"], &["--threads", "2"]);
        assert!(cli.threads_explicit);
        assert_eq!(cli.threads, 3, "the command-line wins");
    }

    #[test]
    fn zero_threads_are_the_default_count() {
        let zero = context(&["--threads", "0"], &[]);
        assert!(zero.threads_explicit);
        assert_eq!(zero.threads, Context::num_threads());

        let config_zero = context(&[], &["--threads", "0"]);
        assert_eq!(config_zero.threads, Context::num_threads());
    }

    /// A non-empty `NO_COLOR` turns `--color auto` plain even on a terminal, and yields to
    /// `--color force`, from the command line or the config.
    #[test]
    fn no_color_env_yields_to_force() {
        for (cli, config, tty, no_color_env, plain) in [
            (&["--color", "auto"][..], &[][..], true, true, true),
            (&["--color", "auto"], &[], true, false, false),
            (&["--color", "auto"], &[], false, false, true),
            (&["--color", "none"], &[], true, false, true),
            (&["--color", "force"], &[], false, true, false),
            (&[], &["--color", "force"], false, true, false),
        ] {
            let mut ctx = context(cli, config);
            ctx.stdout_is_tty = tty;
            ctx.no_color_env = no_color_env;
            assert_eq!(
                ctx.no_color(),
                plain,
                "{cli:?} {config:?} tty={tty} NO_COLOR={no_color_env}"
            );
        }
    }
}

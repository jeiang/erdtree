use config::{Config, File, Value, ValueKind};
use error::Error;
use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
};

/// Errors associated with loading and parsing the toml config file.
pub mod error;

/// Testing related to `.erdtree.toml`.
pub mod test;

/// Represents an instruction on how to handle a single key-value pair, which makes up a single
/// command-line argument, when constructing the arguments vector.
enum ArgInstructions {
    /// Used for bool arguments such as `--icons`. When `icons = true` is set in `.erdtree.toml`,
    /// we only want `--icons` to be pushed into the ultimate arguments vector.
    PushKeyOnly,

    /// Used for arguments such as `--threads 10`.
    PushKeyValue { parsed_value: OsString },

    /// Used for repeatable arguments given as an array (e.g. `exclude = ["a", "b"]`), pushing the
    /// key once per element.
    PushKeyValues { parsed_values: Vec<OsString> },

    /// If a bool field is set to false in `.erdtree.toml` (e.g. `icons = false`) then we want to
    /// completely omit the key-value pair from the arguments that we ultimately use.
    Pass,
}

/// Takes in a `Config` that is generated from [`load`] returning a `Vec<OsString>` which
/// represents command-line arguments from `.erdtree.toml`. If a `named_table` is provided then
/// the top-level table in `.erdtree.toml` is ignored and the configurations specified in the
/// `named_table` will be used instead.
pub fn parse(config: Config, named_table: Option<&str>) -> Result<Vec<OsString>, Error> {
    let mut args_map = config.cache.into_table()?;

    if let Some(table) = named_table {
        let new_conf = args_map
            .get(table)
            .and_then(|conf| conf.clone().into_table().ok())
            .ok_or_else(|| Error::MissingAltConfig(table.to_owned()))?;

        args_map = new_conf;
    } else {
        args_map.retain(|_k, v| !matches!(v.kind, ValueKind::Table(_)));
    }

    let mut parsed_args = vec![OsString::from("--")];

    let process_key = |s| OsString::from(format!("--{s}").replace('_', "-"));

    for (k, v) in &args_map {
        match parse_argument(k, v)? {
            ArgInstructions::PushKeyValue { parsed_value } => {
                let fmt_key = process_key(k);
                parsed_args.push(fmt_key);
                parsed_args.push(parsed_value);
            },

            ArgInstructions::PushKeyValues { parsed_values } => {
                for parsed_value in parsed_values {
                    parsed_args.push(process_key(k));
                    parsed_args.push(parsed_value);
                }
            },

            ArgInstructions::PushKeyOnly => {
                let fmt_key = process_key(k);
                parsed_args.push(fmt_key);
            },

            ArgInstructions::Pass => {},
        }
    }

    Ok(parsed_args)
}

/// Reads in the first `.erdtree.toml` that exists, in [`candidates`] order. Returns `Ok(None)`
/// when there is no config file; a file that exists but fails to parse is an error.
pub fn load() -> Result<Option<Config>, Error> {
    let Some(path) = candidates().into_iter().find(|p| p.is_file()) else {
        return Ok(None);
    };

    Config::builder()
        .add_source(File::from(path))
        .build()
        .map(Some)
        .map_err(Error::from)
}

/// Locations of the TOML config in lookup order:
///
/// - `$ERDTREE_TOML_PATH`
/// - `$XDG_CONFIG_HOME/erdtree/{.erdtree.toml,erdtree.toml,config.toml}`
/// - `$XDG_CONFIG_HOME/.erdtree.toml`
/// - `$HOME/.config/erdtree/{.erdtree.toml,erdtree.toml,config.toml}`
/// - `$HOME/.erdtree.toml`
#[cfg(unix)]
fn candidates() -> Vec<PathBuf> {
    use super::{CONFIG_DIR, ERDTREE_CONFIG_TOML, ERDTREE_DIR, HOME, XDG_CONFIG_HOME};

    let mut paths: Vec<PathBuf> = env::var_os(super::ERDTREE_TOML_PATH)
        .map(PathBuf::from)
        .into_iter()
        .collect();

    if let Some(xdg) = env::var_os(XDG_CONFIG_HOME).map(PathBuf::from) {
        paths.extend(in_erdtree_dir(&xdg.join(ERDTREE_DIR)));
        paths.push(xdg.join(ERDTREE_CONFIG_TOML));
    }

    if let Some(home) = env::var_os(HOME).map(PathBuf::from) {
        paths.extend(in_erdtree_dir(&home.join(CONFIG_DIR).join(ERDTREE_DIR)));
        paths.push(home.join(ERDTREE_CONFIG_TOML));
    }

    paths
}

/// Locations of the TOML config in lookup order:
///
/// - `$ERDTREE_TOML_PATH`
/// - `%APPDATA%\erdtree\{.erdtree.toml,erdtree.toml,config.toml}`
#[cfg(windows)]
fn candidates() -> Vec<PathBuf> {
    use super::ERDTREE_DIR;

    env::var_os(super::ERDTREE_TOML_PATH)
        .map(PathBuf::from)
        .into_iter()
        .chain(
            dirs::config_dir()
                .into_iter()
                .flat_map(|d| in_erdtree_dir(&d.join(ERDTREE_DIR))),
        )
        .collect()
}

/// Config file names accepted inside an `erdtree` config directory, in lookup order.
fn in_erdtree_dir(dir: &Path) -> impl Iterator<Item = PathBuf> + use<> {
    [super::ERDTREE_CONFIG_TOML, "erdtree.toml", "config.toml"]
        .map(|name| dir.join(name))
        .into_iter()
}

/// Simple utility used to extract the underlying value from the [`Value`] enum that we get when
/// loading in the values from `.erdtree.toml`, returning instructions on how the argument should
/// be processed into the ultimate arguments vector.
fn parse_argument(keyword: &str, arg: &Value) -> Result<ArgInstructions, Error> {
    macro_rules! try_parse_num {
        ($n:expr_2021) => {
            usize::try_from($n)
                .map_err(|_e| Error::InvalidInteger(keyword.to_owned()))
                .map(|num| {
                    let parsed = OsString::from(format!("{num}"));
                    ArgInstructions::PushKeyValue {
                        parsed_value: parsed,
                    }
                })
        };
    }

    match &arg.kind {
        ValueKind::Boolean(val) => {
            if *val {
                Ok(ArgInstructions::PushKeyOnly)
            } else {
                Ok(ArgInstructions::Pass)
            }
        },
        ValueKind::String(val) => Ok(ArgInstructions::PushKeyValue {
            parsed_value: OsString::from(val),
        }),
        ValueKind::I64(val) => try_parse_num!(*val),
        ValueKind::I128(val) => try_parse_num!(*val),
        ValueKind::U64(val) => try_parse_num!(*val),
        ValueKind::U128(val) => try_parse_num!(*val),
        ValueKind::Array(items) => items
            .iter()
            .map(|item| match &item.kind {
                ValueKind::String(val) => Ok(OsString::from(val)),
                _ => Err(Error::InvalidArgument(keyword.to_owned())),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|parsed_values| ArgInstructions::PushKeyValues { parsed_values }),
        _ => Err(Error::InvalidArgument(keyword.to_owned())),
    }
}

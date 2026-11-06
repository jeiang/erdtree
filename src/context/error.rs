use super::config::error::Error as TomlError;
use clap::{Error as ClapError, parser::MatchesError};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    ArgParse(ClapError),

    #[error("A configuration file was found but failed to parse: {0}")]
    Config(ClapError),

    #[error("{0}")]
    Toml(#[from] TomlError),

    #[error("{0}")]
    Matches(#[from] MatchesError),

    #[error("'--config' was specified but a `.erdtree.toml` file could not be found")]
    NoToml,
}

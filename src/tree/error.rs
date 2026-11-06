use crate::walk::filter::Error as FilterError;

/// Errors that may occur while traversing or construction of [`Tree`].
///
/// [`Tree`]: super::Tree
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Filter(#[from] FilterError),

    #[error("{0}")]
    DirNotFound(String),

    #[error("Failed to compute root node.")]
    MissingRoot,

    #[error("No entries to show with given arguments.")]
    NoMatches,
}

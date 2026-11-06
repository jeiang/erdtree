use crate::context::error::Error as CtxError;

/// Errors that may occur while traversing or construction of [`Tree`].
///
/// [`Tree`]: super::Tree
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Context(#[from] CtxError),

    #[error("{0}")]
    DirNotFound(String),

    #[error("Failed to compute root node.")]
    MissingRoot,

    #[error("No entries to show with given arguments.")]
    NoMatches,
}

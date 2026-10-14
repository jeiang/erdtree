use clap::ValueEnum;

/// Enum to determine how directories should be ordered relative to regular files in output.
#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq, Default)]
pub enum Order {
    /// Directories are ordered as if they were regular nodes.
    #[default]
    None,

    /// Directories above files in the regular and flat layouts, below files in the inverted and
    /// iflat layouts
    First,

    /// Directories below files in the regular and flat layouts, above files in the inverted and
    /// iflat layouts
    Last,
}

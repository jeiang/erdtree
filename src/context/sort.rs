use clap::ValueEnum;

/// Order in which to print nodes.
#[derive(Copy, Clone, Debug, ValueEnum, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Type {
    /// Sort entries by file name in lexicographical order.
    Name,
    /// Sort entries by file name in reversed lexicographical order.
    Rname,

    /// Sort entries by file name in case-insensitive lexicographical order
    Nameci,

    /// Sort entries by file name in reversed case-insensitive lexicographical order
    Rnameci,

    /// Sort entries by size smallest to largest, top to bottom
    #[default]
    Size,

    /// Sort entries by size largest to smallest, bottom to top
    Rsize,

    /// Sort entries by newer to older Accessing Date
    #[value(alias("atime"))]
    Access,

    /// Sort entries by older to newer Accessing Date
    #[value(alias("ratime"))]
    Raccess,

    /// Sort entries by newer to older Creation Date
    #[value(alias("ctime"))]
    Create,

    /// Sort entries by older to newer Creation Date
    #[value(alias("rctime"))]
    Rcreate,

    /// Sort entries by newer to older Alteration Date
    #[value(alias("mtime"))]
    Mod,

    /// Sort entries by older to newer Alteration Date
    #[value(alias("rmtime"))]
    Rmod,
}

/// What a sort type orders nodes by.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum By {
    /// File name, byte by byte.
    Name,
    /// Case-folded file name, ties broken by [`By::Name`].
    NameCi,
    /// Size in the configured metric; a missing size counts as 0.
    Size,
    /// Timestamps; a missing one counts as the time of the run.
    Accessed,
    Created,
    Modified,
}

/// Direction of a sort, in the frame of the sorted child lists: the order in which the
/// inverted and iflat layouts print nodes top to bottom. The regular and flat layouts print
/// the same lists bottom to top.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Smaller names, sizes and older timestamps first.
    Ascending,
    /// Larger names, sizes and newer timestamps first.
    Descending,
}

impl Type {
    /// What this sort type orders by and in which direction.
    pub const fn spec(self) -> (By, Direction) {
        use Direction::{Ascending, Descending};

        match self {
            Self::Name => (By::Name, Ascending),
            Self::Rname => (By::Name, Descending),
            Self::Nameci => (By::NameCi, Ascending),
            Self::Rnameci => (By::NameCi, Descending),
            Self::Size => (By::Size, Descending),
            Self::Rsize => (By::Size, Ascending),
            Self::Access => (By::Accessed, Descending),
            Self::Raccess => (By::Accessed, Ascending),
            Self::Create => (By::Created, Descending),
            Self::Rcreate => (By::Created, Ascending),
            Self::Mod => (By::Modified, Descending),
            Self::Rmod => (By::Modified, Ascending),
        }
    }

    /// Whether this sorts by file name, in any case mode or direction.
    pub const fn is_name(self) -> bool {
        matches!(self.spec().0, By::Name | By::NameCi)
    }
}

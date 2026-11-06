use clap::ValueEnum;

/// Different types of timestamps available in long-view.
#[derive(Copy, Clone, Debug, ValueEnum, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Stamp {
    /// Time created (alias: ctime)
    #[value(alias("ctime"))]
    Create,

    /// Time last accessed (alias: atime)
    #[value(alias("atime"))]
    Access,

    /// Time last modified (alias: mtime)
    #[default]
    #[value(alias("mtime"))]
    Mod,
}

/// Different formatting options for timestamps
#[derive(Copy, Clone, Debug, ValueEnum, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Format {
    /// Timestamp formatted following the iso8601, with slight differences and the time-zone omitted
    Iso,

    /// Timestamp formatted following the exact iso8601 specifications
    IsoStrict,

    /// Timestamp only shows date without time in YYYY-MM-DD format
    Short,

    /// Timestamp is shown in DD MMM HH:MM format
    #[default]
    Default,
}

impl Format {
    /// The chrono pattern of the format.
    pub const fn pattern(self) -> &'static str {
        match self {
            Self::Iso => "%Y-%m-%d %H:%M:%S",
            Self::IsoStrict => "%Y-%m-%dT%H:%M:%S%Z",
            Self::Short => "%Y-%m-%d",
            Self::Default => "%d %h %H:%M %g",
        }
    }

    /// Width of the time column: the length of a formatted timestamp in this format with a
    /// four-digit year and a whole-minute offset; `short` keeps its earlier column of 12.
    pub const fn width(self) -> usize {
        match self {
            Self::Iso => 19,
            Self::IsoStrict => 25,
            Self::Short => 12,
            Self::Default => 15,
        }
    }
}

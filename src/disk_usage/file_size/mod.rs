use clap::ValueEnum;

/// Formats sizes in bytes, logical or physical.
pub mod byte;

pub mod line_count;

pub mod word_count;

/// Streaming UTF-8 validation shared by the line and word count metrics.
mod utf8_stream;

/// Determines between logical or physical size for display
#[derive(Copy, Clone, Debug, ValueEnum, Default)]
pub enum DiskUsage {
    /// How many bytes does a file contain
    Logical,

    /// How many actual bytes on disk, taking into account blocks, sparse files, and compression.
    #[default]
    Physical,

    /// How many total lines a file contains
    Line,

    /// How many total words a file contains
    Word,

    /// How many blocks are allocated to store the file
    #[cfg(unix)]
    Block,
}

impl DiskUsage {
    /// Whether sizes are in bytes, shown with a unit.
    pub const fn is_bytes(self) -> bool {
        matches!(self, Self::Logical | Self::Physical)
    }

    /// Whether sizes come from reading the contents of files.
    pub const fn reads_files(self) -> bool {
        matches!(self, Self::Line | Self::Word)
    }
}

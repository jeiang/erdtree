//! Listing with `std::fs::read_dir`, where directories are opened by path.

use super::Listing;
use crate::{
    fs::stat::{Fields, Stat},
    walk::{Walker, rules},
};
use std::{fs, path::Path};

/// How a job opens its directory: by path, so there is nothing to carry.
pub struct Access;

impl Access {
    pub const fn root() -> Self {
        Self
    }
}

/// An open directory, not listed yet.
pub struct Opened(fs::ReadDir);

impl Opened {
    /// Metadata of the directory, which only the descriptor-based walk reads.
    // The same interface as the descriptor-based walk.
    #[allow(clippy::unused_self)]
    pub const fn stat(&self, _fields: Fields) -> Option<Stat> {
        None
    }
}

/// Opens and lists directories for one worker.
pub struct Lister;

// The same interface as the descriptor-based listers, which hold buffers and a budget.
#[allow(clippy::unused_self, clippy::needless_pass_by_ref_mut)]
impl Lister {
    /// The lister of a walk by `walker`, which [`Lister::for_worker`] copies.
    pub const fn new(_walker: &Walker) -> Self {
        Self
    }

    pub const fn for_worker(&self) -> Self {
        Self
    }

    /// Opens the directory at `path`; `None` when it cannot be read.
    pub fn open(&self, path: &Path, _access: &Access) -> Option<Opened> {
        fs::read_dir(path).ok().map(Opened)
    }

    /// Reads every entry of `dir` into `listing` and notes ignore-relevant names in `found`.
    /// The metadata of the directory and whether its entries are only counted matter on macOS
    /// only.
    pub fn fill(
        &mut self,
        listing: &mut Listing,
        dir: Opened,
        _stat: Option<&Stat>,
        _count_only: bool,
        found: &mut rules::Found,
    ) {
        for entry in dir.0 {
            let Ok(entry) = entry else {
                listing.unusable += 1;
                continue;
            };

            let name = entry.file_name();
            found.note(&name);

            // Free on Unix (`d_type`) and Windows (find data).
            let Ok(file_type) = entry.file_type() else {
                listing.unusable += 1;
                continue;
            };

            if listing.push(name.as_encoded_bytes(), Some(file_type.into())) {
                listing.std.push(entry);
            }
        }
    }
}

impl Listing {
    /// Metadata of entry `i` without following a symlink.
    pub fn lstat(&self, i: usize, fields: Fields) -> Option<Stat> {
        Some(Stat::from_metadata(&self.std[i].metadata().ok()?, fields))
    }

    /// Metadata of entry `i`, following a symlink.
    // The descriptor-based walk looks the entry up relative to the open directory.
    #[allow(clippy::unused_self)]
    pub fn stat(&self, _i: usize, path: &Path, fields: Fields) -> Option<Stat> {
        Some(Stat::from_metadata(&fs::metadata(path).ok()?, fields))
    }

    /// Whether entry `i` is hidden: its name starts with `.` or, on Windows, it has the hidden
    /// attribute.
    pub fn is_hidden(&self, i: usize) -> bool {
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;

            const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;

            // Free on Windows: the attributes come with the directory listing.
            if self.std[i]
                .metadata()
                .is_ok_and(|md| md.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0)
            {
                return true;
            }
        }

        self.name(i).first() == Some(&b'.')
    }

    /// The open directory, to read ignore files relative to; directories are opened by path.
    // The same interface as the descriptor-based walk.
    #[allow(clippy::unused_self)]
    pub const fn fd(&self) -> rules::OpenDir {
        None
    }

    // The same interface as the descriptor-based walk.
    #[allow(clippy::unused_self)]
    pub const fn access(&self, _path: &Path, _follow: bool) -> Access {
        Access
    }
}

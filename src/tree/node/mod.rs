use super::{SizeFormat, Tree, topo::ix};
use crate::{
    disk_usage::file_size::{DiskUsage, FileSize, byte, line_count, word_count},
    fs::stat::Kind,
};
use nu_ansi_term::Style;
use std::{ffi::OsStr, path::Path, time::SystemTime};

#[cfg(unix)]
use crate::{
    disk_usage::file_size::block,
    fs::permissions::{FileMode, SymbolicNotation},
    tree::error::Error,
    tree::widths::lookup,
};

/// Ordering and sorting rules for [Node].
pub mod cmp;

/// A borrowed view of one entry of a [`Tree`]. All file-system data was read during the walk;
/// every method here is CPU-only.
#[derive(Clone, Copy)]
pub struct Node<'a> {
    tree: &'a Tree,
    i: u32,
}

impl<'a> Node<'a> {
    /// View of node `i` of `tree`.
    pub const fn new(tree: &'a Tree, i: u32) -> Self {
        Self { tree, i }
    }

    /// File name; for a symlink, the name of the link, not of the target.
    pub fn file_name(self) -> &'a OsStr {
        self.tree.cols.name(ix(self.i))
    }

    /// Depth below the root, which is depth 0.
    pub fn depth(self) -> usize {
        ix(self.tree.cols.depth[ix(self.i)])
    }

    /// Timestamp of the last modification, if collected (sort or long view).
    pub fn modified(self) -> Option<SystemTime> {
        self.tree.cols.modified.get(ix(self.i)).copied().flatten()
    }

    /// Timestamp of creation, if collected (sort or long view).
    pub fn created(self) -> Option<SystemTime> {
        self.tree.cols.created.get(ix(self.i)).copied().flatten()
    }

    /// Timestamp of the last access, if collected (sort or long view).
    pub fn accessed(self) -> Option<SystemTime> {
        self.tree.cols.accessed.get(ix(self.i)).copied().flatten()
    }

    /// Inode number (long view).
    #[cfg(unix)]
    pub fn ino(self) -> Option<u64> {
        self.inode().map(|inode| inode.ino)
    }

    /// Number of hard links to the inode (long view).
    #[cfg(unix)]
    pub fn nlink(self) -> Option<u64> {
        self.inode().map(|inode| inode.nlink)
    }

    #[cfg(unix)]
    fn inode(self) -> Option<crate::fs::inode::Inode> {
        self.tree.cols.inode.get(ix(self.i)).copied().flatten()
    }

    /// Whether the entry, or the target of a followed symlink, is a directory.
    pub fn is_dir(self) -> bool {
        self.file_type().is_dir()
    }

    /// Whether the entry is a symlink.
    pub fn is_symlink(self) -> bool {
        self.tree.is_link(self.i)
    }

    /// Target of the symlink.
    pub fn symlink_target_path(self) -> Option<&'a Path> {
        self.is_symlink()
            .then(|| self.tree.cols.link_target(self.i))
            .flatten()
    }

    /// Target of the symlink, as shown after the arrow.
    pub fn symlink_target_file_name(self) -> Option<&'a OsStr> {
        self.symlink_target_path().map(Path::as_os_str)
    }

    /// The `ls -F` style indicator for the kind of entry: `@` symlink, `/` directory and, on
    /// Unix, `|` FIFO, `=` socket, `*` executable regular file.
    pub fn indicator(self) -> Option<&'static str> {
        if self.is_symlink() {
            return Some("@");
        }

        let ft = self.file_type();

        if ft.is_dir() {
            return Some("/");
        }

        #[cfg(unix)]
        {
            if ft.is_fifo() {
                return Some("|");
            }

            if ft.is_socket() {
                return Some("=");
            }

            if ft.is_file()
                && self
                    .tree
                    .cols
                    .mode
                    .get(ix(self.i))
                    .is_some_and(|mode| mode & 0o111 != 0)
            {
                return Some("*");
            }
        }

        None
    }

    /// Type of the entry, or of its target when it is a followed symlink.
    pub fn file_type(self) -> Kind {
        self.tree.cols.file_type[ix(self.i)]
    }

    /// Raw size in the configured metric.
    pub fn size_value(self) -> Option<u64> {
        self.tree.cols.size[ix(self.i)]
    }

    /// Size in the configured metric and display format.
    pub fn file_size(self) -> Option<FileSize> {
        let value = self.size_value()?;
        let SizeFormat {
            disk_usage,
            human,
            unit,
        } = self.tree.size_format;

        Some(match disk_usage {
            DiskUsage::Logical => FileSize::Byte(byte::Metric::new(
                value,
                byte::MetricKind::Logical,
                unit,
                human,
            )),
            DiskUsage::Physical => FileSize::Byte(byte::Metric::new(
                value,
                byte::MetricKind::Physical,
                unit,
                human,
            )),
            DiskUsage::Line => FileSize::Line(line_count::Metric::from(value)),
            DiskUsage::Word => FileSize::Word(word_count::Metric::from(value)),
            #[cfg(unix)]
            DiskUsage::Block => FileSize::Block(block::Metric { value }),
        })
    }

    /// Permissions for the long view.
    #[cfg(unix)]
    pub fn mode(self) -> Result<FileMode, Error> {
        use std::os::unix::fs::PermissionsExt;

        let permissions = std::fs::Permissions::from_mode(self.tree.cols.mode[ix(self.i)]);
        Ok(permissions.try_mode_symbolic_notation()?)
    }

    /// Raw `st_mode` of the entry (long view).
    #[cfg(unix)]
    pub fn st_mode(self) -> u32 {
        self.tree.cols.mode[ix(self.i)]
    }

    /// Whether the entry has extended attributes (long view).
    #[cfg(unix)]
    pub fn has_xattrs(self) -> bool {
        self.tree
            .cols
            .long
            .get(ix(self.i))
            .is_some_and(|a| a.has_xattrs)
    }

    /// Owner and group names, if both could be resolved (long view, up to the display level).
    #[cfg(unix)]
    pub fn owner_and_group(self) -> Option<(&'a str, &'a str)> {
        let attrs = self.tree.cols.long.get(ix(self.i)).filter(|a| a.read)?;
        let owner = lookup(&self.tree.users, attrs.uid)?;
        let group = lookup(&self.tree.groups, attrs.gid)?;
        Some((owner, group))
    }

    /// `LS_COLORS` style, if colors are enabled.
    pub fn style(self) -> Option<Style> {
        self.tree.cols.style_at(ix(self.i))
    }
}

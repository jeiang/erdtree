//! Tree indices and their missing-parent sentinel.

/// Parent of the root.
pub const NONE: u32 = u32::MAX;

/// `usize` from a node index.
#[inline]
pub const fn ix(i: u32) -> usize {
    i as usize
}

/// Node index from a position; [`NONE`] when it does not fit.
#[inline]
pub fn id(i: usize) -> u32 {
    u32::try_from(i).unwrap_or(NONE)
}

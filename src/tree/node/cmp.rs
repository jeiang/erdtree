//! The order of `--sort` and `--dir-order`. Orders are in the frame of
//! [`Direction`](sort::Direction): the sorted lists hold nodes in the order the inverted and
//! iflat layouts print top to bottom.

use super::Node;
use crate::{
    context::{
        dir,
        sort::{self, By, Direction},
    },
    tree::{
        Tree,
        topo::{Key, ix},
    },
};
use std::{
    cmp::Ordering,
    sync::atomic::{AtomicBool, Ordering as AtomicOrdering},
};

/// Compares two nodes by the sort type and `--dir-order`, without a tie-break.
type NodeComparator = dyn for<'a, 'b> Fn(Node<'a>, Node<'b>) -> Ordering + Send + Sync;

/// How a [`Sorter`] orders nodes that the sort type and `--dir-order` leave equal, and what
/// the items it sorts are.
#[derive(Clone, Copy)]
pub enum Ties<'a> {
    /// Items are nodes, ordered by name on ties: the order of child lists.
    Name,
    /// Items are positions in this list of nodes, ordered by position on ties: a stable sort
    /// of the list, as the flat layouts need.
    Position(&'a [u32]),
}

/// Largest value of the 63 bits below the directory class.
const MAX63: u64 = u64::MAX >> 1;

/// The order of a sort type with `--dir-order` and [`Ties`]: packed integer keys that order
/// most items with cheap compares, and [`Self::cmp`] for items with equal keys.
///
/// A smaller key never belongs to an item that the order puts later, so only items with
/// equal keys need [`Self::cmp`]. The high word of a key holds the directory class at bit 63
/// and below it the size or timestamp, clamped to 63 bits (extremes share a key and fall back
/// to the comparator). The low word holds the first 8 bytes of the name (case-folded for
/// [`By::NameCi`]) for name sorts, the name prefix that breaks ties for other sorts with
/// [`Ties::Name`], and 0 otherwise. Descending sorts flip the value and the name bytes.
pub struct Sorter<'a> {
    tree: &'a Tree,
    by: By,
    direction: Direction,
    dir_order: dir::Order,
    ties: Ties<'a>,

    /// Compares by the sort type and `--dir-order`, built once.
    cmp: Box<NodeComparator>,

    /// Set when a key had to clamp its value, so that unequal nodes may share a key.
    clamped: AtomicBool,
}

impl<'a> Sorter<'a> {
    /// The order of `sort` and `dir_order` on the nodes of `tree`.
    pub fn new(tree: &'a Tree, sort: sort::Type, dir_order: dir::Order, ties: Ties<'a>) -> Self {
        let (by, direction) = sort.spec();

        Self {
            tree,
            by,
            direction,
            dir_order,
            ties,
            cmp: comparator(by, direction, dir_order),
            clamped: AtomicBool::new(false),
        }
    }

    /// Node of `item`.
    const fn node(&self, item: u32) -> Node<'a> {
        let i = match self.ties {
            Ties::Name => item,
            Ties::Position(list) => list[ix(item)],
        };
        Node::new(self.tree, i)
    }

    /// Key of `item`.
    pub fn key(&self, item: u32) -> Key {
        let node = self.node(item);
        let class = dir_class(self.dir_order, node) << 63;
        let ascending = self.direction == Direction::Ascending;
        let flip = |bytes: u64| if ascending { bytes } else { !bytes };

        let (value, exact) = match self.by {
            By::Name => return (class, flip(naming::prefix(node.file_name()))),
            By::NameCi => return (class, flip(naming::ci_prefix(node.file_name()))),
            By::Size => {
                let size = sizing::value(node);
                (size.min(MAX63), size < MAX63)
            },
            By::Accessed => time_stamping::key(time_stamping::accessed(node)),
            By::Created => time_stamping::key(time_stamping::created(node)),
            By::Modified => time_stamping::key(time_stamping::modified(node)),
        };

        if !exact {
            self.clamped.store(true, AtomicOrdering::Relaxed);
        }

        let primary = if ascending { value } else { MAX63 - value };

        // A clamped value ties with every value clamped to the same number, so its key leaves
        // the tie-break to the comparator.
        let lo = match self.ties {
            Ties::Name if exact => naming::prefix(node.file_name()),
            _ => 0,
        };

        (class | primary, lo)
    }

    /// Orders items `a` and `b`, whose keys must be equal.
    pub fn cmp(&self, a: u32, b: u32) -> Ordering {
        let (node_a, node_b) = (self.node(a), self.node(b));

        // Equal keys of a size or time sort hold the same directory class and value unless a
        // key was clamped, so the comparator would find them equal.
        let exact = !matches!(self.by, By::Name | By::NameCi)
            && !self.clamped.load(AtomicOrdering::Relaxed);
        let by_sort = if exact {
            Ordering::Equal
        } else {
            (self.cmp)(node_a, node_b)
        };

        match self.ties {
            Ties::Name => by_sort.then_with(|| node_a.file_name().cmp(node_b.file_name())),
            Ties::Position(_) => by_sort.then_with(|| a.cmp(&b)),
        }
    }
}

/// The first thing nodes are ordered by under `--dir-order`: with `first` directories come
/// after files in the sorted lists, with `last` before them.
fn dir_class(order: dir::Order, node: Node<'_>) -> u64 {
    match order {
        dir::Order::None => 0,
        dir::Order::First => u64::from(node.is_dir()),
        dir::Order::Last => u64::from(!node.is_dir()),
    }
}

/// Compares by what `by` orders, in `direction`, after the directory class of `dir_order`.
fn comparator(by: By, direction: Direction, dir_order: dir::Order) -> Box<NodeComparator> {
    match by {
        By::Name => ordered(naming::comparator, direction, dir_order),
        By::NameCi => ordered(naming::ci_comparator, direction, dir_order),
        By::Size => ordered(sizing::comparator, direction, dir_order),
        By::Accessed => ordered(time_stamping::by_accessed, direction, dir_order),
        By::Created => ordered(time_stamping::by_created, direction, dir_order),
        By::Modified => ordered(time_stamping::by_modified, direction, dir_order),
    }
}

/// `ascending`, the ascending comparator of a sort, turned to `direction` behind the
/// directory class of `dir_order`, as one closure.
fn ordered<F>(ascending: F, direction: Direction, dir_order: dir::Order) -> Box<NodeComparator>
where
    F: for<'a, 'b> Fn(Node<'a>, Node<'b>) -> Ordering + Copy + Send + Sync + 'static,
{
    let class =
        move |a: Node<'_>, b: Node<'_>| dir_class(dir_order, a).cmp(&dir_class(dir_order, b));

    match (dir_order, direction) {
        (dir::Order::None, Direction::Ascending) => Box::new(ascending),
        (dir::Order::None, Direction::Descending) => {
            Box::new(move |a: Node<'_>, b: Node<'_>| ascending(b, a))
        },
        (_, Direction::Ascending) => {
            Box::new(move |a: Node<'_>, b: Node<'_>| class(a, b).then_with(|| ascending(a, b)))
        },
        (_, Direction::Descending) => {
            Box::new(move |a: Node<'_>, b: Node<'_>| class(a, b).then_with(|| ascending(b, a)))
        },
    }
}

mod time_stamping {
    use crate::tree::node::Node;
    use core::cmp::Ordering;
    use std::{sync::LazyLock, time::SystemTime};

    /// Stands in for a missing timestamp. Read once, so every comparison sees the same value
    /// and the order stays total (sorting may panic on an inconsistent order).
    static NOW: LazyLock<SystemTime> = LazyLock::new(SystemTime::now);

    fn or_now(stamp: Option<SystemTime>) -> SystemTime {
        stamp.unwrap_or(*NOW)
    }

    /// The access time a sort compares.
    pub fn accessed(node: Node<'_>) -> SystemTime {
        or_now(node.accessed())
    }

    /// The creation time a sort compares.
    pub fn created(node: Node<'_>) -> SystemTime {
        or_now(node.created())
    }

    /// The modification time a sort compares.
    pub fn modified(node: Node<'_>) -> SystemTime {
        or_now(node.modified())
    }

    pub fn by_accessed(a: Node<'_>, b: Node<'_>) -> Ordering {
        accessed(a).cmp(&accessed(b))
    }

    pub fn by_created(a: Node<'_>, b: Node<'_>) -> Ordering {
        created(a).cmp(&created(b))
    }

    pub fn by_modified(a: Node<'_>, b: Node<'_>) -> Ordering {
        modified(a).cmp(&modified(b))
    }

    /// A 63-bit key that grows with the timestamp: nanoseconds around the epoch, clamped, so
    /// about the years 1824 to 2116 are distinct and timestamps outside tie. The flag tells
    /// whether the key is exact, as opposed to clamped.
    pub fn key(stamp: SystemTime) -> (u64, bool) {
        const ZERO: i128 = 1 << 62;
        let max63 = i128::from(u64::MAX >> 1);

        let nanos = |d: std::time::Duration| i128::try_from(d.as_nanos()).unwrap_or(i128::MAX);
        let since_epoch = match stamp.duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => nanos(d),
            Err(e) => -nanos(e.duration()),
        };

        let shifted = since_epoch + ZERO;
        let key = u64::try_from(shifted.clamp(0, max63)).unwrap_or(0);
        (key, 0 < shifted && shifted < max63)
    }
}

mod sizing {
    use crate::tree::node::Node;
    use core::cmp::Ordering;

    /// The size a sort compares; a missing size counts as 0.
    pub fn value(node: Node<'_>) -> u64 {
        node.size_value().unwrap_or(0)
    }

    pub fn comparator(a: Node<'_>, b: Node<'_>) -> Ordering {
        value(a).cmp(&value(b))
    }
}

mod naming {
    use crate::tree::node::Node;
    use core::cmp::Ordering;
    use std::ffi::OsStr;

    /// Orders file names lexicographically, as [`OsStr`] orders them.
    pub fn comparator(a: Node<'_>, b: Node<'_>) -> Ordering {
        a.file_name().cmp(b.file_name())
    }

    /// The first 8 bytes of `name`, as a number that orders like the bytes: the order of
    /// [`comparator`], as [`OsStr`] compares its encoded bytes on every platform.
    pub fn prefix(name: &OsStr) -> u64 {
        let bytes = name.as_encoded_bytes();
        let mut head = [0_u8; 8];
        let len = bytes.len().min(8);
        head[..len].copy_from_slice(&bytes[..len]);
        u64::from_be_bytes(head)
    }

    /// The first 8 bytes of the case-folded name that [`ci_comparator`] compares, as a number
    /// that orders like those bytes.
    pub fn ci_prefix(name: &OsStr) -> u64 {
        let bytes = name.as_encoded_bytes();
        let mut head = [0_u8; 8];
        let ascii = &bytes[..bytes.len().min(8)];

        if ascii.is_ascii() {
            for (slot, byte) in head.iter_mut().zip(ascii) {
                *slot = byte.to_ascii_lowercase();
            }
        } else {
            // UTF-8 orders like the code points, so the bytes of the folded text order like
            // its characters.
            let folded = name.to_string_lossy();
            let mut filled = 0;
            for c in folded.chars().flat_map(char::to_lowercase) {
                let mut utf8 = [0_u8; 4];
                for &byte in c.encode_utf8(&mut utf8).as_bytes() {
                    if filled < 8 {
                        head[filled] = byte;
                        filled += 1;
                    }
                }
                if filled == 8 {
                    break;
                }
            }
        }

        u64::from_be_bytes(head)
    }

    /// Orders file names case-insensitively. Names are lowercased with full Unicode case
    /// mapping (lossily, for non-UTF-8 names) and ties are broken by the case-sensitive
    /// [`comparator`] so the order is deterministic.
    pub fn ci_comparator(a: Node<'_>, b: Node<'_>) -> Ordering {
        // ASCII lowercases byte by byte, to the same order as the general path below.
        let (a_bytes, b_bytes) = (
            a.file_name().as_encoded_bytes(),
            b.file_name().as_encoded_bytes(),
        );
        if a_bytes.is_ascii() && b_bytes.is_ascii() {
            return a_bytes
                .iter()
                .map(u8::to_ascii_lowercase)
                .cmp(b_bytes.iter().map(u8::to_ascii_lowercase))
                .then_with(|| comparator(a, b));
        }

        let a_name = a.file_name().to_string_lossy();
        let b_name = b.file_name().to_string_lossy();
        let a_folded = a_name.chars().flat_map(char::to_lowercase);
        let b_folded = b_name.chars().flat_map(char::to_lowercase);

        a_folded.cmp(b_folded).then_with(|| comparator(a, b))
    }
}

#[cfg(test)]
mod test {
    use super::{MAX63, Sorter, Ties};
    use crate::{
        context::{dir, sort},
        fs::stat::Kind,
        tree::{Tree, columns::Columns, count::FileCount, node::Node, topo::ix, widths::Widths},
    };
    use clap::ValueEnum;
    use std::{
        ffi::OsStr,
        time::{Duration, UNIX_EPOCH},
    };

    /// Names with shared and case-folded 8-byte prefixes, non-ASCII case mappings that change
    /// lengths, and on Unix bytes that are not UTF-8.
    fn names() -> Vec<&'static OsStr> {
        let names = [
            "a",
            "A",
            "b",
            "B",
            "z",
            "abcdefgh",
            "abcdefghi",
            "abcdefghj",
            "ABCDEFGHI",
            "Abcdefghi",
            "abcdefgH",
            "é",
            "É",
            "éa",
            "Ea",
            "eb",
            "straße",
            "STRASSE",
            "strasse",
            "\u{212a}elvin",
            "kelvin",
            "KELVIN",
            "\u{130}x",
            "ix",
            "i\u{307}x",
            "~",
            "0",
        ]
        .map(OsStr::new)
        .into_iter();

        #[cfg(unix)]
        let names = {
            use std::os::unix::ffi::OsStrExt;
            names.chain([b"ab\xffcd".as_slice(), b"AB\xffcdefgh", b"\xfe"].map(OsStr::from_bytes))
        };

        names.collect()
    }

    /// A tree of nodes that combine the names with sizes, timestamps and kinds; with
    /// `extremes`, also sizes and times that clamp in the keys.
    fn tree(extremes: bool) -> Tree {
        let secs = Duration::from_secs;
        let mut sizes = vec![None, Some(0), Some(1), Some(2), Some(1 << 40)];
        let mut times = vec![
            None,
            Some(UNIX_EPOCH),
            UNIX_EPOCH.checked_add(Duration::from_nanos(1)),
            UNIX_EPOCH.checked_sub(Duration::from_nanos(1)),
            UNIX_EPOCH.checked_add(secs(4_000_000_000)),
            UNIX_EPOCH.checked_sub(secs(4_000_000_000)),
        ];
        if extremes {
            sizes.extend([MAX63 - 1, MAX63, MAX63 + 1, u64::MAX].map(Some));
            times.extend(
                [
                    secs(5_000_000_000),
                    secs(9_000_000_000),
                    secs(10_000_000_000),
                ]
                .into_iter()
                .flat_map(|d| [UNIX_EPOCH.checked_add(d), UNIX_EPOCH.checked_sub(d)]),
            );
        }

        let mut cols = Columns::default();
        let mut k = 0_usize;
        for name in names() {
            for variant in 0..4 {
                cols.push_name(name);
                cols.depth.push(1);
                cols.file_type.push(if (k + variant).is_multiple_of(3) {
                    Kind::Dir
                } else {
                    Kind::File
                });
                cols.size.push(sizes[(k * 7 + variant) % sizes.len()]);
                cols.accessed.push(times[(k * 5 + variant) % times.len()]);
                cols.created
                    .push(times[(k * 3 + variant * 2) % times.len()]);
                cols.modified.push(times[(k + variant * 3) % times.len()]);
                k += 1;
            }
        }

        Tree {
            link: vec![0; cols.len().div_ceil(64)],
            cols,
            root: 0,
            order: vec![],
            last: vec![],
            shown_depth: u32::MAX,
            stump: false,
            widths: Widths::default(),
            #[cfg(unix)]
            users: vec![],
            #[cfg(unix)]
            groups: vec![],
            count: FileCount::default(),
        }
    }

    /// For every sort type, `--dir-order` and kind of ties, keys followed by [`Sorter::cmp`]
    /// on equal keys order every pair of items as the comparator followed by the tie-break,
    /// with and without clamped keys.
    #[test]
    fn keys_agree_with_comparators() {
        for extremes in [false, true] {
            let tree = tree(extremes);
            let n = u32::try_from(tree.cols.len()).unwrap_or(0);
            let reversed: Vec<u32> = (0..n).rev().collect();

            for &sort in sort::Type::value_variants() {
                for &dir_order in dir::Order::value_variants() {
                    for ties in [Ties::Name, Ties::Position(&reversed)] {
                        let sorter = Sorter::new(&tree, sort, dir_order, ties);
                        let keys: Vec<_> = (0..n).map(|item| sorter.key(item)).collect();
                        let node = |item: u32| match ties {
                            Ties::Name => Node::new(&tree, item),
                            Ties::Position(list) => Node::new(&tree, list[ix(item)]),
                        };
                        let expected = |a: u32, b: u32| {
                            let (node_a, node_b) = (node(a), node(b));
                            (sorter.cmp)(node_a, node_b).then_with(|| match ties {
                                Ties::Name => node_a.file_name().cmp(node_b.file_name()),
                                Ties::Position(_) => a.cmp(&b),
                            })
                        };

                        for a in 0..n {
                            for b in 0..n {
                                let (ka, kb) = (keys[ix(a)], keys[ix(b)]);
                                let got = ka.cmp(&kb).then_with(|| sorter.cmp(a, b));
                                assert_eq!(
                                    got,
                                    expected(a, b),
                                    "{sort:?} {dir_order:?} extremes {extremes}: {:?} vs {:?}",
                                    node(a).file_name(),
                                    node(b).file_name(),
                                );
                            }
                        }

                        let clamped = !sort.is_name() && extremes;
                        assert_eq!(
                            sorter.clamped.load(std::sync::atomic::Ordering::Relaxed),
                            clamped
                        );
                    }
                }
            }
        }
    }
}

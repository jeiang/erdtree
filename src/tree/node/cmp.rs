use super::Node;
use crate::{
    context::{Context, dir, sort},
    tree::{Tree, topo::Key},
};
use std::{
    cmp::Ordering,
    sync::atomic::{AtomicBool, Ordering as AtomicOrdering},
};

/// Comparator type used to sort [Node]s.
pub type NodeComparator = dyn for<'a, 'b> Fn(Node<'a>, Node<'b>) -> Ordering + Send + Sync;

/// Yields function pointer to the appropriate `Node` comparator.
pub fn comparator(ctx: &Context) -> Box<NodeComparator> {
    let base = base_comparator(ctx.sort);

    match ctx.dir_order {
        dir::Order::First => {
            Box::new(move |a: Node<'_>, b: Node<'_>| dir_first_comparator(a, b, &*base))
        },
        dir::Order::Last => {
            Box::new(move |a: Node<'_>, b: Node<'_>| dir_last_comparator(a, b, &*base))
        },
        dir::Order::None => base,
    }
}

/// Packed sort keys for the order of [`comparator`], to sort by cheap integer compares first.
///
/// The key of a node is a pair of integers such that a smaller key never belongs to a node that
/// the comparator puts later: wherever keys differ they agree with the comparator, and only
/// nodes with equal keys need the comparator itself. The high word holds the directory class
/// (`--dir-order`) and the size or timestamp (clamped to 63 bits, so extremes tie and fall back
/// to the comparator); the low word holds the first 8 bytes of the name, case-folded and
/// reversed as the sort type requires.
pub struct SortKeys<'a> {
    tree: &'a Tree,
    sort: sort::Type,
    dir_order: dir::Order,

    /// Set when a key had to clamp its value, so that unequal nodes may share a key.
    clamped: AtomicBool,

    /// Whether ties of the sort type are broken by name, as in the order of child lists. Flat
    /// layouts compare by the sort type alone, where a name in the key would break ties wrongly.
    by_name: bool,
}

/// Largest value of the 63 bits below the directory class.
const MAX63: u64 = u64::MAX >> 1;

impl<'a> SortKeys<'a> {
    /// Keys for the sort of `ctx`. With `by_name`, nodes that tie are ordered by name.
    pub const fn new(tree: &'a Tree, ctx: &Context, by_name: bool) -> Self {
        Self {
            tree,
            sort: ctx.sort,
            dir_order: ctx.dir_order,
            clamped: AtomicBool::new(false),
            by_name,
        }
    }

    /// Whether nodes with the same key (of a sort type that does not look at names) tie in the
    /// comparator, for all keys computed so far.
    pub fn is_exact(&self) -> bool {
        !self.clamped.load(AtomicOrdering::Relaxed)
    }

    /// Key of node `i`.
    pub fn key(&self, i: u32) -> Key {
        let node = Node::new(self.tree, i);

        let class = match self.dir_order {
            dir::Order::First => u64::from(node.is_dir()),
            dir::Order::Last => u64::from(!node.is_dir()),
            dir::Order::None => 0,
        };

        let name = || node.file_name();
        let plain = || naming::prefix(name());

        let sized = |value: Option<u64>| {
            let value = value.unwrap_or(0);
            (value.min(MAX63), value < MAX63)
        };

        // `ascending` keys grow with the value, the others shrink.
        let (value, ascending) = match self.sort {
            sort::Type::Name => return (class << 63, plain()),
            sort::Type::Rname => return (class << 63, !plain()),
            sort::Type::Nameci => return (class << 63, naming::ci_prefix(name())),
            sort::Type::Rnameci => return (class << 63, !naming::ci_prefix(name())),
            sort::Type::Size => (sized(node.size_value()), false),
            sort::Type::Rsize => (sized(node.size_value()), true),
            sort::Type::Access => (time_stamping::key(node.accessed()), false),
            sort::Type::Raccess => (time_stamping::key(node.accessed()), true),
            sort::Type::Create => (time_stamping::key(node.created()), false),
            sort::Type::Rcreate => (time_stamping::key(node.created()), true),
            sort::Type::Mod => (time_stamping::key(node.modified()), false),
            sort::Type::Rmod => (time_stamping::key(node.modified()), true),
        };
        let (primary, lo, exact) = value_key(value, ascending, || self.tie(name));

        if !exact {
            self.clamped.store(true, AtomicOrdering::Relaxed);
        }

        (class << 63 | primary, lo)
    }

    /// Low word for sort types that break ties by name.
    fn tie<'n>(&self, name: impl FnOnce() -> &'n std::ffi::OsStr) -> u64 {
        if self.by_name {
            naming::prefix(name())
        } else {
            0
        }
    }
}

/// Key words for a value that the sort type orders by, ascending or descending. `value` is a
/// number of at most 63 bits with a flag that tells whether it is exact, as opposed to clamped.
/// `tie` gives the low word that breaks ties between exact values.
///
/// A clamped value ties with every other value clamped to the same number, so the tie-break
/// stays out of its key: only the comparator can order those nodes. The flag of the result
/// tells whether the key is exact.
fn value_key(
    (value, exact): (u64, bool),
    ascending: bool,
    tie: impl FnOnce() -> u64,
) -> (u64, u64, bool) {
    let primary = if ascending { value } else { MAX63 - value };
    (primary, if exact { tie() } else { 0 }, exact)
}

/// Orders directories first. Provides a fallback if inputs are not directories.
fn dir_first_comparator(
    a: Node<'_>,
    b: Node<'_>,
    fallback: impl for<'a, 'b> Fn(Node<'a>, Node<'b>) -> Ordering + Send + Sync,
) -> Ordering {
    match (a.is_dir(), b.is_dir()) {
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        _ => fallback(a, b),
    }
}

/// Orders directories last. Provides a fallback if inputs are not directories.
fn dir_last_comparator(
    a: Node<'_>,
    b: Node<'_>,
    fallback: impl for<'a, 'b> Fn(Node<'a>, Node<'b>) -> Ordering + Send + Sync,
) -> Ordering {
    match (a.is_dir(), b.is_dir()) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => fallback(a, b),
    }
}

/// Grabs the comparator for two non-dir type [Node]s.
fn base_comparator(sort_type: sort::Type) -> Box<NodeComparator> {
    Box::new(match sort_type {
        sort::Type::Name => naming::comparator,
        sort::Type::Rname => naming::rev_comparator,
        sort::Type::Nameci => naming::ci_comparator,
        sort::Type::Rnameci => naming::rev_ci_comparator,
        sort::Type::Size => sizing::comparator,
        sort::Type::Rsize => sizing::rev_comparator,
        sort::Type::Access => time_stamping::accessed::comparator,
        sort::Type::Raccess => time_stamping::accessed::rev_comparator,
        sort::Type::Create => time_stamping::created::comparator,
        sort::Type::Rcreate => time_stamping::created::rev_comparator,
        sort::Type::Mod => time_stamping::modified::comparator,
        sort::Type::Rmod => time_stamping::modified::rev_comparator,
    })
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

    /// A 63-bit key that grows with the timestamp (a missing one counts as now): nanoseconds
    /// around the epoch, clamped, so about the years 1824 to 2116 are distinct and timestamps
    /// outside tie. The flag tells whether the key is exact, as opposed to clamped.
    pub fn key(stamp: Option<SystemTime>) -> (u64, bool) {
        const ZERO: i128 = 1 << 62;
        let max63 = i128::from(u64::MAX >> 1);

        let nanos = |d: std::time::Duration| i128::try_from(d.as_nanos()).unwrap_or(i128::MAX);
        let since_epoch = match or_now(stamp).duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => nanos(d),
            Err(e) => -nanos(e.duration()),
        };

        let shifted = since_epoch + ZERO;
        let key = u64::try_from(shifted.clamp(0, max63)).unwrap_or(0);
        (key, 0 < shifted && shifted < max63)
    }

    pub mod accessed {
        use super::*;

        /// Comparator that sorts [Node]s by Last Access timestamp, newer to older.
        pub fn comparator(a: Node<'_>, b: Node<'_>) -> Ordering {
            let a_stamp = or_now(a.accessed());
            let b_stamp = or_now(b.accessed());
            b_stamp.cmp(&a_stamp)
        }

        /// Comparator that sorts [Node]s by Access timestamp, older to newer.
        pub fn rev_comparator(a: Node<'_>, b: Node<'_>) -> Ordering {
            comparator(b, a)
        }
    }

    pub mod created {
        use super::*;

        /// Comparator that sorts [Node]s by Creation timestamp, newer to older.
        pub fn comparator(a: Node<'_>, b: Node<'_>) -> Ordering {
            let a_stamp = or_now(a.created());
            let b_stamp = or_now(b.created());
            b_stamp.cmp(&a_stamp)
        }

        /// Comparator that sorts [Node]s by Creation timestamp, older to newer.
        pub fn rev_comparator(a: Node<'_>, b: Node<'_>) -> Ordering {
            comparator(b, a)
        }
    }

    pub mod modified {
        use super::*;

        /// Comparator that sorts [Node]s by Alteration timestamp, newer to older.
        pub fn comparator(a: Node<'_>, b: Node<'_>) -> Ordering {
            let a_stamp = or_now(a.modified());
            let b_stamp = or_now(b.modified());
            b_stamp.cmp(&a_stamp)
        }

        /// Comparator that sorts [Node]s by Alteration timestamp, older to newer.
        pub fn rev_comparator(a: Node<'_>, b: Node<'_>) -> Ordering {
            comparator(b, a)
        }
    }
}

mod sizing {
    use crate::tree::node::Node;
    use core::cmp::Ordering;

    /// Comparator that sorts [Node]s by size, largest to smallest.
    pub fn comparator(a: Node<'_>, b: Node<'_>) -> Ordering {
        let a_size = a.size_value().unwrap_or(0);
        let b_size = b.size_value().unwrap_or(0);
        b_size.cmp(&a_size)
    }
    /// Comparator that sorts [Node]s by size, smallest to largest.
    pub fn rev_comparator(a: Node<'_>, b: Node<'_>) -> Ordering {
        comparator(b, a)
    }
}

mod naming {
    use crate::tree::node::Node;
    use core::cmp::Ordering;
    use std::ffi::OsStr;

    /// Comparator based on [Node] file names in lexicographical order.
    pub fn comparator(a: Node<'_>, b: Node<'_>) -> Ordering {
        a.file_name().cmp(b.file_name())
    }

    /// Comparator based on [Node] file names in reversed lexicographical order.
    pub fn rev_comparator(a: Node<'_>, b: Node<'_>) -> Ordering {
        comparator(b, a)
    }

    /// The first 8 bytes of `name`, as a number that orders like the bytes: the order of names
    /// on Unix. Other platforms have no such prefix and give 0, which only costs speed.
    pub fn prefix(name: &OsStr) -> u64 {
        if cfg!(not(unix)) {
            return 0;
        }

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

    /// Comparator based on [Node] file names in case-insensitive lexicographical order. Names
    /// are lowercased with full Unicode case mapping (lossily, for non-UTF-8 names) and ties
    /// are broken by the case-sensitive [`comparator`] so the order is deterministic.
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

    /// Exact reverse of [`ci_comparator`].
    pub fn rev_ci_comparator(a: Node<'_>, b: Node<'_>) -> Ordering {
        ci_comparator(b, a)
    }
}

#[cfg(test)]
mod test {
    use super::{MAX63, time_stamping, value_key};
    use std::{
        cmp::Ordering,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    /// A value and the name that breaks its ties, as the comparator sees them.
    type Entry = (Option<SystemTime>, u64, u64);

    /// Whether `key` ever puts a before b although the comparator `cmp` puts a after b.
    fn contradicts(entries: &[Entry], ascending: bool, by_time: bool) -> bool {
        let key = |&(t, size, name): &Entry| {
            let value = if by_time {
                time_stamping::key(t)
            } else {
                (size.min(MAX63), size < MAX63)
            };
            let (hi, lo, _) = value_key(value, ascending, || name);
            (hi, lo)
        };
        let cmp = |a: &Entry, b: &Entry| {
            let by_value = if by_time {
                a.0.cmp(&b.0)
            } else {
                a.1.cmp(&b.1)
            };
            let by_value = if ascending {
                by_value
            } else {
                by_value.reverse()
            };
            by_value.then(a.2.cmp(&b.2))
        };

        entries.iter().any(|a| {
            entries
                .iter()
                .any(|b| key(a).cmp(&key(b)) == Ordering::Less && cmp(a, b) == Ordering::Greater)
        })
    }

    /// Keys of times and sizes, including extremes that clamp, never order nodes against the
    /// comparator, which breaks ties by name.
    #[test]
    fn keys_agree_with_comparator() {
        let mut seed = 0x9e37_79b9_u64;
        let mut next = || {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            seed >> 11
        };
        let times = [
            Duration::from_secs(0),
            Duration::from_secs(1),
            Duration::from_secs(4_000_000_000),
            Duration::from_secs(5_000_000_000),
            Duration::from_secs(9_000_000_000),
            Duration::from_secs(10_000_000_000),
            Duration::from_secs(11_000_000_000),
        ];
        let sizes = [0, 1, MAX63 - 1, MAX63, MAX63 + 1, u64::MAX];

        for round in 0..2000 {
            let entries: Vec<Entry> = (0..6)
                .map(|_| {
                    let base = times[usize::try_from(next()).unwrap_or(0) % times.len()];
                    let jitter = Duration::from_nanos(next() % 3);
                    let t = match next() % 3 {
                        0 => UNIX_EPOCH.checked_add(base + jitter),
                        1 => UNIX_EPOCH.checked_sub(base + jitter),
                        _ => UNIX_EPOCH.checked_add(Duration::from_nanos(next() % 5)),
                    };
                    let size = sizes[usize::try_from(next()).unwrap_or(0) % sizes.len()];
                    (t, size.saturating_sub(next() % 2), next() % 3)
                })
                .collect();

            for ascending in [false, true] {
                for by_time in [false, true] {
                    assert!(
                        !contradicts(&entries, ascending, by_time),
                        "round {round}: {entries:?}"
                    );
                }
            }
        }
    }
}

use super::Node;
use crate::context::{Context, dir, sort};
use std::cmp::Ordering;

/// Comparator type used to sort [Node]s.
pub type NodeComparator = dyn Fn(&Node, &Node) -> Ordering;

/// Yields function pointer to the appropriate `Node` comparator.
pub fn comparator(ctx: &Context) -> Box<NodeComparator> {
    let sort_type = ctx.sort;

    match ctx.dir_order {
        dir::Order::First => {
            Box::new(move |a, b| dir_first_comparator(a, b, base_comparator(sort_type)))
        },
        dir::Order::Last => {
            Box::new(move |a, b| dir_last_comparator(a, b, base_comparator(sort_type)))
        },
        dir::Order::None => base_comparator(sort_type),
    }
}

/// Orders directories first. Provides a fallback if inputs are not directories.
fn dir_first_comparator(
    a: &Node,
    b: &Node,
    fallback: impl Fn(&Node, &Node) -> Ordering,
) -> Ordering {
    match (a.is_dir(), b.is_dir()) {
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        _ => fallback(a, b),
    }
}

/// Orders directories last. Provides a fallback if inputs are not directories.
fn dir_last_comparator(
    a: &Node,
    b: &Node,
    fallback: impl Fn(&Node, &Node) -> Ordering,
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
    use std::time::SystemTime;

    pub mod accessed {
        use super::*;

        /// Comparator that sorts [Node]s by Last Access timestamp, newer to older.
        pub fn comparator(a: &Node, b: &Node) -> Ordering {
            let a_stamp = a.accessed().unwrap_or_else(SystemTime::now);
            let b_stamp = b.accessed().unwrap_or_else(SystemTime::now);
            b_stamp.cmp(&a_stamp)
        }

        /// Comparator that sorts [Node]s by Access timestamp, older to newer.
        pub fn rev_comparator(a: &Node, b: &Node) -> Ordering {
            comparator(b, a)
        }
    }

    pub mod created {
        use super::*;

        /// Comparator that sorts [Node]s by Creation timestamp, newer to older.
        pub fn comparator(a: &Node, b: &Node) -> Ordering {
            let a_stamp = a.created().unwrap_or_else(SystemTime::now);
            let b_stamp = b.created().unwrap_or_else(SystemTime::now);
            b_stamp.cmp(&a_stamp)
        }

        /// Comparator that sorts [Node]s by Creation timestamp, older to newer.
        pub fn rev_comparator(a: &Node, b: &Node) -> Ordering {
            comparator(b, a)
        }
    }

    pub mod modified {
        use super::*;

        /// Comparator that sorts [Node]s by Alteration timestamp, newer to older.
        pub fn comparator(a: &Node, b: &Node) -> Ordering {
            let a_stamp = a.modified().unwrap_or_else(SystemTime::now);
            let b_stamp = b.modified().unwrap_or_else(SystemTime::now);
            b_stamp.cmp(&a_stamp)
        }

        /// Comparator that sorts [Node]s by Alteration timestamp, older to newer.
        pub fn rev_comparator(a: &Node, b: &Node) -> Ordering {
            comparator(b, a)
        }
    }
}

mod sizing {
    use crate::disk_usage::file_size::FileSize;
    use crate::tree::node::Node;
    use core::cmp::Ordering;

    /// Comparator that sorts [Node]s by size, largest to smallest.
    pub fn comparator(a: &Node, b: &Node) -> Ordering {
        let a_size = a.file_size().map_or(0, FileSize::value);
        let b_size = b.file_size().map_or(0, FileSize::value);
        b_size.cmp(&a_size)
    }
    /// Comparator that sorts [Node]s by size, smallest to largest.
    pub fn rev_comparator(a: &Node, b: &Node) -> Ordering {
        comparator(b, a)
    }
}

mod naming {
    use crate::tree::node::Node;
    use core::cmp::Ordering;

    /// Comparator based on [Node] file names in lexicographical order.
    pub fn comparator(a: &Node, b: &Node) -> Ordering {
        a.file_name().cmp(b.file_name())
    }

    /// Comparator based on [Node] file names in reversed lexicographical order.
    pub fn rev_comparator(a: &Node, b: &Node) -> Ordering {
        comparator(b, a)
    }

    /// Comparator based on [Node] file names in case-insensitive lexicographical order. Names
    /// are lowercased with full Unicode case mapping (lossily, for non-UTF-8 names) and ties
    /// are broken by the case-sensitive [`comparator`] so the order is deterministic.
    pub fn ci_comparator(a: &Node, b: &Node) -> Ordering {
        let a_name = a.file_name().to_string_lossy();
        let b_name = b.file_name().to_string_lossy();
        let a_folded = a_name.chars().flat_map(char::to_lowercase);
        let b_folded = b_name.chars().flat_map(char::to_lowercase);

        a_folded.cmp(b_folded).then_with(|| comparator(a, b))
    }

    /// Exact reverse of [`ci_comparator`].
    pub fn rev_ci_comparator(a: &Node, b: &Node) -> Ordering {
        ci_comparator(b, a)
    }
}

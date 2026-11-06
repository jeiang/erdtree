//! Column widths, from maxima over the size and long-view columns instead of a formatted value
//! per node. Digit counts grow with the value, so the widest number comes from the largest one;
//! human-readable sizes switch unit as values grow, so each unit keeps its own largest value.

use super::Tree;
use crate::{
    bytes::digits,
    context::Context,
    disk_usage::{
        file_size::byte,
        units::{PREFIXES, PrefixKind},
    },
};
use std::ops::Range;

/// Widths of the size and long-view columns, in terminal columns, that fit the shown nodes.
#[derive(Default)]
pub struct Widths {
    /// Whether some shown node has a size; only then is there a size column.
    pub sized: bool,
    pub max_size_width: usize,

    /// Width of the unit of byte sizes: that of the longest unit name that sizes can show.
    pub max_size_unit_width: usize,

    #[cfg(unix)]
    pub max_nlink_width: usize,

    #[cfg(unix)]
    pub max_ino_width: usize,

    #[cfg(unix)]
    pub max_owner_width: usize,

    #[cfg(unix)]
    pub max_group_width: usize,
}

impl Widths {
    /// The widths of no nodes: no size column, and the unit width that sizes of `ctx` show.
    pub fn new(ctx: &Context) -> Self {
        let names = ctx.unit.names();
        let unit_width = match human(ctx) {
            Some(_) => names.iter().map(|name| name.len()).max().unwrap_or(0),
            None => names[0].len(),
        };

        Self {
            max_size_unit_width: unit_width,
            ..Self::default()
        }
    }
}

/// The unit of sizes shown human-readable, if they are.
const fn human(ctx: &Context) -> Option<PrefixKind> {
    if ctx.human && ctx.disk_usage.is_bytes() {
        Some(ctx.unit)
    } else {
        None
    }
}

/// Scans of fewer nodes than this run on one thread.
const PAR_MIN: usize = 1 << 18;

/// The nodes a width scan covers.
#[derive(Clone, Copy)]
pub enum Scope<'a> {
    All,

    /// The nodes `i` with `shown[i]`.
    Shown(&'a [bool]),
}

impl Scope<'_> {
    const fn len(self, all: usize) -> usize {
        match self {
            Self::All => all,
            Self::Shown(shown) => shown.len(),
        }
    }
}

/// Maximum sizes, tracked just finely enough to find the widest formatted number.
#[derive(Clone, Copy)]
pub struct SizeStats {
    max: Option<u64>,

    /// For human-readable sizes, their unit and the least value of buckets 1 and up (the
    /// prefix indices of [`PrefixKind::index_of`]).
    human: Option<(PrefixKind, [u64; PREFIXES - 1])>,

    /// For each bucket below the last, one more than the largest size under the next bucket's
    /// least value (0 for none).
    below: [u64; PREFIXES - 1],
}

impl SizeStats {
    /// Statistics of no sizes; `human` is the unit of human-readable sizes, if shown so.
    pub fn new(human: Option<PrefixKind>) -> Self {
        Self {
            max: None,
            human: human.map(|unit| (unit, std::array::from_fn(|k| unit.base(k + 1)))),
            below: [0; PREFIXES - 1],
        }
    }

    #[inline]
    pub fn add(&mut self, value: u64) {
        if self.max.is_none_or(|m| value > m) {
            self.max = Some(value);
        }

        if let Some((_, limits)) = &self.human {
            for (below, &limit) in self.below.iter_mut().zip(limits) {
                if value < limit {
                    *below = (*below).max(value + 1);
                }
            }
        }
    }

    /// Adds the sizes of `other`, which tracks the same units.
    pub fn merge(&mut self, other: &Self) {
        self.max = self.max.max(other.max);
        for (below, other) in self.below.iter_mut().zip(other.below) {
            *below = (*below).max(other);
        }
    }

    /// The width of the widest number as the size column shows it; `None` without sizes.
    pub fn width(&self) -> Option<usize> {
        let max = self.max?;
        let Some((unit, limits)) = self.human else {
            return Some(digits(max));
        };

        let format = byte::Metric::new(unit, true);
        let mut widest = 0;

        for b in 0..PREFIXES {
            // The largest size of bucket `b`, if there is one: the largest one below the next
            // bucket is at least as large as any in `b`.
            let candidate = if b < PREFIXES - 1 {
                self.below[b].checked_sub(1)
            } else {
                Some(max)
            };
            let least = if b == 0 { 0 } else { limits[b - 1] };

            if let Some(value) = candidate.filter(|&v| v >= least) {
                widest = widest.max(format.width(value));
            }
        }

        Some(widest)
    }
}

/// Maxima and the owners over some nodes.
pub struct Stats {
    size: SizeStats,
    #[cfg(unix)]
    ino: u64,
    #[cfg(unix)]
    nlink: u64,

    /// `(uid, gid)` of nodes whose owner and group were read, without consecutive repeats; not
    /// yet distinct.
    #[cfg(unix)]
    owners: Vec<(u32, u32)>,
}

impl Stats {
    fn merge(&mut self, other: &Self) {
        self.size.merge(&other.size);
        #[cfg(unix)]
        {
            self.ino = self.ino.max(other.ino);
            self.nlink = self.nlink.max(other.nlink);
            self.owners.extend_from_slice(&other.owners);
        }
    }
}

impl Tree {
    /// Scans `scope` for the facts that column widths depend on, with up to `threads` threads.
    pub fn scan(&self, ctx: &Context, scope: Scope<'_>, threads: usize) -> Stats {
        let len = scope.len(self.cols.len());

        if len < PAR_MIN || threads < 2 {
            return self.scan_range(ctx, scope, 0..len);
        }

        let parts = crate::par::run_parts(0..threads, |k| {
            self.scan_range(ctx, scope, crate::par::split(len, threads, k))
        });
        let mut parts = parts.into_iter();
        let mut total = parts.next().expect("parallel scan has parts");
        for part in parts {
            total.merge(&part);
        }
        total
    }

    /// [`Self::scan`] of the nodes at positions `range` of `scope`, on this thread.
    fn scan_range(&self, ctx: &Context, scope: Scope<'_>, range: Range<usize>) -> Stats {
        let cols = &self.cols;
        let mut stats = Stats {
            size: SizeStats::new(human(ctx)),
            #[cfg(unix)]
            ino: 0,
            #[cfg(unix)]
            nlink: 0,
            #[cfg(unix)]
            owners: vec![],
        };

        #[cfg(unix)]
        let mut last_owner = None;

        for i in range {
            if let Scope::Shown(shown) = scope
                && !shown[i]
            {
                continue;
            }

            if let Some(value) = cols.size[i] {
                stats.size.add(value);
            }

            #[cfg(unix)]
            if ctx.long {
                if let Some(Some(inode)) = cols.inode.get(i) {
                    stats.ino = stats.ino.max(inode.ino);
                    stats.nlink = stats.nlink.max(inode.nlink);
                }

                if let Some(attrs) = cols.long.get(i).filter(|a| a.read)
                    && last_owner != Some((attrs.uid, attrs.gid))
                {
                    last_owner = Some((attrs.uid, attrs.gid));
                    stats.owners.push((attrs.uid, attrs.gid));
                }
            }
        }

        stats
    }

    /// Widens the tree's column widths to fit the nodes `stats` was scanned from, and resolves
    /// their owner and group names.
    #[cfg_attr(
        not(unix),
        allow(unused_variables, reason = "only the Unix long view reads `ctx`")
    )]
    pub fn apply_widths(&mut self, ctx: &Context, stats: &Stats) {
        let props = &mut self.widths;
        if let Some(number) = stats.size.width() {
            props.sized = true;
            props.max_size_width = props.max_size_width.max(number);
        }

        #[cfg(unix)]
        if ctx.long {
            props.max_ino_width = props.max_ino_width.max(digits(stats.ino));
            props.max_nlink_width = props.max_nlink_width.max(digits(stats.nlink));

            let mut owners = stats.owners.clone();
            owners.sort_unstable();
            owners.dedup();
            self.resolve_owners(&owners);

            let props = &mut self.widths;

            for (uid, gid) in owners {
                if let (Some(owner), Some(group)) =
                    (lookup(&self.users, uid), lookup(&self.groups, gid))
                {
                    props.max_owner_width = props.max_owner_width.max(owner.len());
                    props.max_group_width = props.max_group_width.max(group.len());
                }
            }
        }
    }

    /// Resolves the names of the users and groups in `owners` (sorted, distinct), once each.
    #[cfg(unix)]
    fn resolve_owners(&mut self, owners: &[(u32, u32)]) {
        fn names(mut ids: Vec<u32>, get: fn(u32) -> Option<String>) -> Vec<(u32, Option<String>)> {
            ids.sort_unstable();
            ids.dedup();
            ids.into_iter().map(|id| (id, get(id))).collect()
        }

        self.users = names(
            owners.iter().map(|o| o.0).collect(),
            crate::fs::ug::user_name,
        );
        self.groups = names(
            owners.iter().map(|o| o.1).collect(),
            crate::fs::ug::group_name,
        );
    }
}

/// The resolved name of `id` in `table`, sorted by id.
#[cfg(unix)]
pub fn lookup(table: &[(u32, Option<String>)], id: u32) -> Option<&str> {
    let k = table.binary_search_by_key(&id, |(i, _)| *i).ok()?;
    table[k].1.as_deref()
}

#[cfg(test)]
mod test {
    use super::SizeStats;
    use crate::disk_usage::{file_size::byte, units::PrefixKind};

    /// The width a node-by-node pass gives.
    fn reference(values: &[u64], unit: PrefixKind) -> Option<usize> {
        let format = byte::Metric::new(unit, true);
        values.iter().map(|&v| format.width(v)).max()
    }

    /// Per-unit maxima give the width of the per-node pass, for values around every rounding
    /// and unit boundary and for many random ones.
    #[test]
    fn human_widths_match_node_by_node() {
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };

        for unit in [PrefixKind::Bin, PrefixKind::Si] {
            let mut edges = vec![0, 1, 9, 10, 99, 100, 999, 1000, 1023, 1024, u64::MAX];

            for k in 1..=4 {
                for base in [1000_u64.pow(k), 1024_u64.pow(k)] {
                    for scale in [1, 10, 100, 1000] {
                        let v = base.saturating_mul(scale);
                        for d in [0, 1, base / 20, base / 2000] {
                            edges.extend([v.saturating_sub(d), v.saturating_add(d)]);
                        }
                    }
                }
            }

            for round in 0..300 {
                // Small sets, so that the largest value of a bucket is often a rounding case.
                let count = 1 + usize::try_from(next() % 6).unwrap();
                let values: Vec<u64> = (0..count)
                    .map(|_| match next() % 3 {
                        0 => edges[usize::try_from(next()).unwrap() % edges.len()],
                        1 => next() >> (next() % 64),
                        _ => next() % 5000,
                    })
                    .collect();

                let mut stats = SizeStats::new(Some(unit));
                // Two halves, merged, as the threads of a parallel scan do.
                let (left, right) = values.split_at(values.len() / 2);
                let mut other = SizeStats::new(Some(unit));
                for &v in left {
                    stats.add(v);
                }
                for &v in right {
                    other.add(v);
                }
                stats.merge(&other);

                assert_eq!(
                    stats.width(),
                    reference(&values, unit),
                    "round {round}: {values:?}"
                );
            }
        }
    }

    /// Plain numbers: the width of the largest, which is 1 for 0.
    #[test]
    fn plain_widths_come_from_the_largest() {
        let mut stats = SizeStats::new(None);
        assert_eq!(stats.width(), None);
        stats.add(0);
        assert_eq!(stats.width(), Some(1));
        for v in [7, 12345, 99] {
            stats.add(v);
        }
        assert_eq!(stats.width(), Some(5));
    }
}

use clap::ValueEnum;

/// Determines whether to use SI prefixes or binary prefixes.
#[derive(Copy, Clone, Debug, ValueEnum, Default)]
pub enum PrefixKind {
    /// Displays disk usage using binary prefixes.
    #[default]
    Bin,

    /// Displays disk usage using SI prefixes.
    Si,
}

impl PrefixKind {
    /// Position of the closest human-readable prefix of `value`, from 0 (bytes) to 4: the number
    /// of times it reaches another power of 1024 (or 1000). Exact on integers, unlike a float
    /// logarithm.
    pub const fn index_of(self, value: u64) -> usize {
        match self {
            Self::Bin => match value.checked_ilog2() {
                Some(bits) if bits >= 40 => 4,
                Some(bits) => (bits / 10) as usize,
                None => 0,
            },
            Self::Si => match value.checked_ilog10() {
                Some(exp) if exp >= 12 => 4,
                Some(exp) => (exp / 3) as usize,
                None => 0,
            },
        }
    }

    /// The value of one unit of prefix `index` (at most 4), which is also the least value whose
    /// [`Self::index_of`] is `index`.
    pub const fn base(self, index: usize) -> u64 {
        match self {
            Self::Bin => [1, 1 << 10, 1 << 20, 1 << 30, 1 << 40][index],
            Self::Si => [1, 1_000, 1_000_000, 1_000_000_000, 1_000_000_000_000][index],
        }
    }

    /// Unit names from bytes up, in the order of [`Self::index_of`].
    pub const fn names(self) -> [&'static str; 5] {
        match self {
            Self::Bin => ["B", "KiB", "MiB", "GiB", "TiB"],
            Self::Si => ["B", "KB", "MB", "GB", "TB"],
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// The float thresholds the integer ones replace.
    fn float_bin(value: u64) -> usize {
        let log = (value as f64).log2();
        [10., 20., 30., 40.]
            .iter()
            .take_while(|&&limit| log >= limit)
            .count()
    }

    fn float_si(value: u64) -> usize {
        let log = (value as f64).log10();
        [3., 6., 9., 12.]
            .iter()
            .take_while(|&&limit| log >= limit)
            .count()
    }

    #[test]
    fn integer_prefixes_match_float_thresholds() {
        let mut values = vec![0, 1, 2, u64::MAX];
        let mut around = |p: u64| values.extend(p.saturating_sub(2)..=p.saturating_add(2));
        for k in 0..64 {
            around(1_u64 << k);
        }
        let mut p = 1_u64;
        while let Some(next) = p.checked_mul(10) {
            around(p);
            p = next;
        }
        around(p);

        for v in values {
            assert_eq!(PrefixKind::Bin.index_of(v), float_bin(v), "bin {v}");
            assert_eq!(PrefixKind::Si.index_of(v), float_si(v), "si {v}");
        }
    }

    #[test]
    fn bases_are_index_thresholds() {
        for unit in [PrefixKind::Bin, PrefixKind::Si] {
            assert_eq!(unit.base(0), 1);
            assert_eq!(unit.index_of(0), 0);

            for i in 1..=4 {
                assert_eq!(unit.index_of(unit.base(i)), i, "{unit:?} {i}");
                assert_eq!(unit.index_of(unit.base(i) - 1), i - 1, "{unit:?} {i}");
            }
        }
    }

    #[test]
    fn names_follow_indices() {
        let bin = PrefixKind::Bin;
        assert_eq!(bin.names()[bin.index_of(1 << 20)], "MiB");

        let si = PrefixKind::Si;
        assert_eq!(si.names()[si.index_of(5_000_000_000)], "GB");
    }
}

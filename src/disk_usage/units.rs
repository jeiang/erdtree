use clap::ValueEnum;

/// Number of prefixes sizes are shown with, from none (bytes) up to tera.
pub const PREFIXES: usize = 5;

/// Unit names from bytes up: the shown ones and peta, which only `--min-size` takes.
const BIN_UNITS: [&str; PREFIXES + 1] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
const SI_UNITS: [&str; PREFIXES + 1] = ["B", "KB", "MB", "GB", "TB", "PB"];

/// The values of one unit of each prefix in [`BIN_UNITS`] or [`SI_UNITS`].
const BIN_BASES: [u64; PREFIXES + 1] = powers(1024);
const SI_BASES: [u64; PREFIXES + 1] = powers(1000);

/// The powers of `step` from 1 up.
const fn powers(step: u64) -> [u64; PREFIXES + 1] {
    let mut table = [1; PREFIXES + 1];
    let mut k = 1;
    while k < table.len() {
        table[k] = table[k - 1] * step;
        k += 1;
    }
    table
}

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
    /// Position of the closest shown prefix of `value`, from 0 (bytes) to `PREFIXES - 1`: the
    /// number of times it reaches another power of 1024 (or 1000). Exact on integers, unlike a
    /// float logarithm.
    pub const fn index_of(self, value: u64) -> usize {
        let index = match self {
            Self::Bin => match value.checked_ilog2() {
                Some(bits) => (bits / 10) as usize,
                None => 0,
            },
            Self::Si => match value.checked_ilog10() {
                Some(exp) => (exp / 3) as usize,
                None => 0,
            },
        };
        if index < PREFIXES {
            index
        } else {
            PREFIXES - 1
        }
    }

    /// The value of one unit of prefix `index` (at most [`PREFIXES`]), which is also the least
    /// value whose [`Self::index_of`] is `index` for shown prefixes.
    pub const fn base(self, index: usize) -> u64 {
        match self {
            Self::Bin => BIN_BASES[index],
            Self::Si => SI_BASES[index],
        }
    }

    /// Unit names from bytes up, in the order of [`Self::index_of`], and one more (peta) for
    /// parsing.
    pub const fn units(self) -> &'static [&'static str; PREFIXES + 1] {
        match self {
            Self::Bin => &BIN_UNITS,
            Self::Si => &SI_UNITS,
        }
    }

    /// The shown unit names from bytes up, in the order of [`Self::index_of`].
    pub fn names(self) -> &'static [&'static str] {
        &self.units()[..PREFIXES]
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

            for i in 1..PREFIXES {
                assert_eq!(unit.index_of(unit.base(i)), i, "{unit:?} {i}");
                assert_eq!(unit.index_of(unit.base(i) - 1), i - 1, "{unit:?} {i}");
            }
            assert_eq!(unit.index_of(unit.base(PREFIXES)), PREFIXES - 1);
        }
    }
}

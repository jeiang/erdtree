use clap::ValueEnum;
use std::{
    convert::From,
    fmt::{self, Display},
};

/// Determines whether to use SI prefixes or binary prefixes.
#[derive(Copy, Clone, Debug, ValueEnum, Default)]
pub enum PrefixKind {
    /// Displays disk usage using binary prefixes.
    #[default]
    Bin,

    /// Displays disk usage using SI prefixes.
    Si,
}

/// Binary prefixes.
#[derive(Debug, PartialEq, Eq)]
pub enum BinPrefix {
    Base,
    Kibi,
    Mebi,
    Gibi,
    Tebi,
}

/// SI prefixes.
#[derive(Debug, PartialEq, Eq)]
pub enum SiPrefix {
    Base,
    Kilo,
    Mega,
    Giga,
    Tera,
}

impl SiPrefix {
    /// Returns the human readable representation of the SI prefix.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Base => "B",
            Self::Kilo => "KB",
            Self::Mega => "MB",
            Self::Giga => "GB",
            Self::Tera => "TB",
        }
    }
}

impl BinPrefix {
    /// Returns the human readable representation of the binary prefix.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Base => "B",
            Self::Kibi => "KiB",
            Self::Mebi => "MiB",
            Self::Gibi => "GiB",
            Self::Tebi => "TiB",
        }
    }
}

pub trait UnitPrefix {
    fn base_value(&self) -> u64;
}

impl UnitPrefix for SiPrefix {
    fn base_value(&self) -> u64 {
        match self {
            Self::Base => 1,
            Self::Kilo => 10_u64.pow(3),
            Self::Mega => 10_u64.pow(6),
            Self::Giga => 10_u64.pow(9),
            Self::Tera => 10_u64.pow(12),
        }
    }
}

impl UnitPrefix for BinPrefix {
    fn base_value(&self) -> u64 {
        match self {
            Self::Base => 1,
            Self::Kibi => 2_u64.pow(10),
            Self::Mebi => 2_u64.pow(20),
            Self::Gibi => 2_u64.pow(30),
            Self::Tebi => 2_u64.pow(40),
        }
    }
}

impl BinPrefix {
    /// The prefix of position `index` (0 for none, 4 for tebi).
    const fn from_index(index: usize) -> Self {
        match index {
            0 => Self::Base,
            1 => Self::Kibi,
            2 => Self::Mebi,
            3 => Self::Gibi,
            _ => Self::Tebi,
        }
    }

    /// Position of the closest human-readable prefix of `value`: the number of times it reaches
    /// another power of 1024, at most 4. Exact on integers, unlike a float `log2`.
    pub const fn index_of(value: u64) -> usize {
        match value.checked_ilog2() {
            Some(bits) if bits >= 40 => 4,
            Some(bits) => (bits / 10) as usize,
            None => 0,
        }
    }
}

impl SiPrefix {
    /// The prefix of position `index` (0 for none, 4 for tera).
    const fn from_index(index: usize) -> Self {
        match index {
            0 => Self::Base,
            1 => Self::Kilo,
            2 => Self::Mega,
            3 => Self::Giga,
            _ => Self::Tera,
        }
    }

    /// Position of the closest human-readable prefix of `value`: the number of times it reaches
    /// another power of 1000, at most 4. Exact on integers, unlike a float `log10`.
    pub const fn index_of(value: u64) -> usize {
        match value.checked_ilog10() {
            Some(exp) if exp >= 12 => 4,
            Some(exp) => (exp / 3) as usize,
            None => 0,
        }
    }
}

impl PrefixKind {
    /// Position of the closest human-readable prefix of `value`, from 0 (bytes) to 4.
    pub const fn index_of(self, value: u64) -> usize {
        match self {
            Self::Bin => BinPrefix::index_of(value),
            Self::Si => SiPrefix::index_of(value),
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

/// Get the closest human-readable unit prefix for value.
impl From<u64> for BinPrefix {
    fn from(value: u64) -> Self {
        Self::from_index(Self::index_of(value))
    }
}

/// Get the closest human-readable unit prefix for value.
impl From<u64> for SiPrefix {
    fn from(value: u64) -> Self {
        Self::from_index(Self::index_of(value))
    }
}

impl Display for BinPrefix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl Display for SiPrefix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
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
            assert_eq!(BinPrefix::index_of(v), float_bin(v), "bin {v}");
            assert_eq!(SiPrefix::index_of(v), float_si(v), "si {v}");
        }
    }

    #[test]
    fn names_follow_indices() {
        assert_eq!(
            BinPrefix::from(1 << 20).as_str(),
            PrefixKind::Bin.names()[2]
        );
        assert_eq!(
            SiPrefix::from(5_000_000_000).as_str(),
            PrefixKind::Si.names()[3]
        );
    }
}

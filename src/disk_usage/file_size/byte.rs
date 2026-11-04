use super::super::units::{BinPrefix, PrefixKind, SiPrefix, UnitPrefix};
use crate::render::bytes::u64_into;
use std::{
    fmt::{self, Display},
    io::{Cursor, Write},
};

/// Concerned with measuring file size in bytes, whether logical or physical determined by `kind`.
/// Binary or SI units used for reporting determined by `prefix_kind`.
pub struct Metric {
    pub value: u64,
    pub human_readable: bool,
    #[allow(dead_code)]
    kind: MetricKind,
    prefix_kind: PrefixKind,
}

/// Represents the appropriate method in which to compute bytes. `Logical` represent the total amount
/// of bytes in a file; `Physical` represents how many bytes are actually used to store the file on
/// disk.
pub enum MetricKind {
    Logical,
    Physical,
}

impl Metric {
    /// A [Metric] of `value` bytes.
    pub const fn new(
        value: u64,
        kind: MetricKind,
        prefix_kind: PrefixKind,
        human_readable: bool,
    ) -> Self {
        Self {
            value,
            human_readable,
            kind,
            prefix_kind,
        }
    }

    /// Initializes an empty [Metric] used to represent the total amount of bytes of a file.
    pub const fn init_empty_logical(human_readable: bool, prefix_kind: PrefixKind) -> Self {
        Self::new(0, MetricKind::Logical, prefix_kind, human_readable)
    }

    /// Initializes an empty [Metric] used to represent the total disk space of a file in bytes.
    pub const fn init_empty_physical(human_readable: bool, prefix_kind: PrefixKind) -> Self {
        Self::new(0, MetricKind::Physical, prefix_kind, human_readable)
    }

    /// The number and the unit as displayed, e.g. `("1.0", "KiB")`. The number is formatted into
    /// `buf`, so nothing is allocated.
    pub fn parts<'b>(&self, buf: &'b mut [u8; 32]) -> (&'b str, &'static str) {
        let value = self.value;
        let (unit, base) = match self.prefix_kind {
            PrefixKind::Si if self.human_readable => {
                let unit = SiPrefix::from(value);
                (
                    unit.as_str(),
                    (unit != SiPrefix::Base).then(|| unit.base_value()),
                )
            },
            PrefixKind::Bin if self.human_readable => {
                let unit = BinPrefix::from(value);
                (
                    unit.as_str(),
                    (unit != BinPrefix::Base).then(|| unit.base_value()),
                )
            },
            PrefixKind::Si => (SiPrefix::Base.as_str(), None),
            PrefixKind::Bin => (BinPrefix::Base.as_str(), None),
        };

        let len = if let Some(base) = base {
            tenths(buf, value, base)
        } else {
            let mut digits = [0_u8; 20];
            let start = u64_into(&mut digits, value);
            buf[..20 - start].copy_from_slice(&digits[start..]);
            20 - start
        };

        // Only ASCII digits and '.' were written.
        (std::str::from_utf8(&buf[..len]).unwrap_or_default(), unit)
    }
}

/// Values from which the integer rounding of [`tenths`] is no longer proven to agree with
/// `f64` division. At 4e14 the error of the division, at most half an ulp of `value / base`,
/// still stays below the gap between the true quotient and any rounding tie (`1 / (20 * base)`).
const TENTHS_LIMIT: u64 = 400_000_000_000_000;

/// Formats `value / base` with one decimal into `buf` as `{:.1}` of the `f64` quotient would,
/// and returns the length. Rounding happens on integers, which avoids the slow exact decimal
/// expansion of floats; exact ties and huge values take the float route, where the rounding
/// of the stored `f64` decides.
fn tenths(buf: &mut [u8; 32], value: u64, base: u64) -> usize {
    if value < TENTHS_LIMIT {
        let scaled = value * 10;
        let (mut n, rem) = (scaled / base, scaled % base);

        if 2 * rem != base {
            if 2 * rem > base {
                n += 1;
            }

            let mut digits = [0_u8; 20];
            let start = u64_into(&mut digits, n / 10);
            let int_len = 20 - start;
            buf[..int_len].copy_from_slice(&digits[start..]);
            buf[int_len] = b'.';
            buf[int_len + 1] = b'0' + (n % 10) as u8;
            return int_len + 2;
        }
    }

    let mut cursor = Cursor::new(&mut buf[..]);
    // 32 bytes hold any one-decimal float below 2^64.
    let _ = write!(cursor, "{:.1}", value as f64 / base as f64);
    usize::try_from(cursor.position()).unwrap_or(0)
}

impl Display for Metric {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut buf = [0; 32];
        let (number, unit) = self.parts(&mut buf);
        write!(f, "{number} {unit}")
    }
}

#[test]
fn test_metric() {
    let metric = Metric {
        value: 100,
        kind: MetricKind::Logical,
        human_readable: false,
        prefix_kind: PrefixKind::Bin,
    };
    assert_eq!(format!("{metric}"), "100 B");

    let metric = Metric {
        value: 1000,
        kind: MetricKind::Logical,
        human_readable: true,
        prefix_kind: PrefixKind::Si,
    };
    assert_eq!(format!("{metric}"), "1.0 KB");

    let metric = Metric {
        value: 1000,
        kind: MetricKind::Logical,
        human_readable: true,
        prefix_kind: PrefixKind::Bin,
    };
    assert_eq!(format!("{metric}"), "1000 B");

    let metric = Metric {
        value: 1024,
        kind: MetricKind::Logical,
        human_readable: true,
        prefix_kind: PrefixKind::Bin,
    };
    assert_eq!(format!("{metric}"), "1.0 KiB");

    let metric = Metric {
        value: 2_u64.pow(20),
        kind: MetricKind::Logical,
        human_readable: true,
        prefix_kind: PrefixKind::Bin,
    };
    assert_eq!(format!("{metric}"), "1.0 MiB");

    let metric = Metric {
        value: 123_454,
        kind: MetricKind::Logical,
        human_readable: false,
        prefix_kind: PrefixKind::Bin,
    };
    assert_eq!(format!("{metric}"), "123454 B");
}

#[test]
fn tenths_match_float_formatting() {
    let mut state = 0x2545_F491_4F6C_DD1D_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };

    let bases: Vec<u64> = (1..=4)
        .flat_map(|k| [1024_u64.pow(k), 1000_u64.pow(k)])
        .collect();
    let mut values = vec![
        0,
        1,
        TENTHS_LIMIT - 1,
        TENTHS_LIMIT,
        TENTHS_LIMIT + 1,
        u64::MAX,
    ];
    for &base in &bases {
        for m in [1, 2, 5, 9, 10, 11, 99, 100, 1023, 1024, 4096] {
            for d in [0_u64, 1, 2, base / 40, base / 20, base / 20 + 1, base / 2] {
                // Around the rounding boundaries `(k + 1/2) / 10 * base`.
                let at = (base / 20).wrapping_mul(2 * m + 1);
                values.extend([at.wrapping_add(d), at.wrapping_sub(d), base * m + d]);
            }
        }
    }
    for _ in 0..300_000 {
        values.push(next() >> (next() % 64));
    }

    for &base in &bases {
        for &value in values.iter().filter(|&&v| v >= base) {
            let mut buf = [0; 32];
            let len = tenths(&mut buf, value, base);
            let expected = format!("{:.1}", value as f64 / base as f64);
            assert_eq!(
                std::str::from_utf8(&buf[..len]).unwrap(),
                expected,
                "{value} / {base}"
            );
        }
    }
}

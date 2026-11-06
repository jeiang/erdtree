use super::super::units::PrefixKind;
use crate::render::bytes::u64_into;
use std::io::{Cursor, Write};

/// Formats sizes in bytes, in binary or SI units as `prefix_kind` says, human-readable or as
/// plain bytes.
#[derive(Clone, Copy)]
pub struct Metric {
    prefix_kind: PrefixKind,
    human_readable: bool,
}

impl Metric {
    pub const fn new(prefix_kind: PrefixKind, human_readable: bool) -> Self {
        Self {
            prefix_kind,
            human_readable,
        }
    }

    /// The number and the unit of `value` bytes as displayed, e.g. `("1.0", "KiB")`, and the
    /// prefix index of the value, which picks its color. Without human-readable units the unit
    /// is bytes, but the index is still that of the closest prefix. The number is formatted into
    /// `buf`, so nothing is allocated.
    pub fn parts(self, value: u64, buf: &mut [u8; 32]) -> (&str, &'static str, usize) {
        let index = self.prefix_kind.index_of(value);
        let shown = if self.human_readable { index } else { 0 };

        let len = if shown == 0 {
            let mut digits = [0_u8; 20];
            let start = u64_into(&mut digits, value);
            buf[..20 - start].copy_from_slice(&digits[start..]);
            20 - start
        } else {
            tenths(buf, value, self.prefix_kind.base(shown))
        };

        // Only ASCII digits and '.' were written.
        (
            std::str::from_utf8(&buf[..len]).unwrap_or_default(),
            self.prefix_kind.names()[shown],
            index,
        )
    }

    /// Widths of the number and the unit of `value` bytes as displayed.
    pub fn widths(self, value: u64) -> (usize, usize) {
        let mut buf = [0; 32];
        let (number, unit, _) = self.parts(value, &mut buf);
        (number.len(), unit.len())
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

#[test]
fn test_metric() {
    let shown = |value, prefix_kind, human_readable| {
        let mut buf = [0; 32];
        let (number, unit, _) = Metric::new(prefix_kind, human_readable).parts(value, &mut buf);
        format!("{number} {unit}")
    };

    assert_eq!(shown(100, PrefixKind::Bin, false), "100 B");
    assert_eq!(shown(1000, PrefixKind::Si, true), "1.0 KB");
    assert_eq!(shown(1000, PrefixKind::Bin, true), "1000 B");
    assert_eq!(shown(1024, PrefixKind::Bin, true), "1.0 KiB");
    assert_eq!(shown(2_u64.pow(20), PrefixKind::Bin, true), "1.0 MiB");
    assert_eq!(shown(123_454, PrefixKind::Bin, false), "123454 B");
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

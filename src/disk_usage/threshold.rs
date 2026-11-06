use super::units::PrefixKind;
use std::str::FromStr;

/// Minimum size an entry needs to have in order to be displayed, as given to `--min-size`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct MinSize {
    /// Threshold in the active disk-usage metric (bytes, lines, words or blocks).
    pub value: u64,

    /// Whether the user wrote a unit suffix (e.g. `KB`, `MiB`, or a plain `B`). Units only make
    /// sense for the byte metrics.
    pub has_unit: bool,
}

/// Parser for the `--min-size` argument: an integer with an optional, case-insensitive unit
/// suffix. SI suffixes (`KB`, `MB`, `GB`, `TB`, `PB`) are powers of 1000, binary suffixes (`KiB`,
/// `MiB`, `GiB`, `TiB`, `PiB`) are powers of 1024, and a bare `B` or no suffix means bytes.
///
/// # Errors
/// Returns a message describing the problem for a missing number, an unknown unit, or a value
/// that does not fit in 64 bits.
pub fn parse_min_size(input: &str) -> Result<MinSize, String> {
    let trimmed = input.trim();
    let digits_end = trimmed
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(trimmed.len());
    let (digits, suffix) = trimmed.split_at(digits_end);

    if digits.is_empty() {
        return Err(format!(
            "'{input}' is not a valid size; expected an integer with an optional unit such as 500, 10KB or 1GiB"
        ));
    }

    let number = u64::from_str(digits).map_err(|_| format!("'{digits}' is too large"))?;
    let suffix = suffix.trim();

    let multiplier = if suffix.is_empty() {
        Some(1)
    } else {
        [PrefixKind::Si, PrefixKind::Bin]
            .into_iter()
            .find_map(|unit| {
                let units = unit.units();
                let index = units.iter().position(|u| u.eq_ignore_ascii_case(suffix))?;
                Some(unit.base(index))
            })
    };
    let Some(multiplier) = multiplier else {
        let expected = PrefixKind::Si.units().iter();
        let expected: Vec<&str> = expected
            .chain(&PrefixKind::Bin.units()[1..])
            .copied()
            .collect();
        return Err(format!(
            "unknown unit '{suffix}' in '{input}'; expected one of {}",
            expected.join(", ")
        ));
    };

    let value = number
        .checked_mul(multiplier)
        .ok_or_else(|| format!("'{input}' is too large"))?;

    Ok(MinSize {
        value,
        has_unit: !suffix.is_empty(),
    })
}

#[cfg(test)]
mod test {
    use super::*;

    fn bytes(input: &str) -> u64 {
        parse_min_size(input).unwrap().value
    }

    #[test]
    fn bare_number_is_bytes() {
        assert_eq!(
            parse_min_size("500"),
            Ok(MinSize {
                value: 500,
                has_unit: false
            })
        );
        assert_eq!(
            parse_min_size("0"),
            Ok(MinSize {
                value: 0,
                has_unit: false
            })
        );
    }

    #[test]
    fn plain_b_is_bytes_with_unit() {
        assert_eq!(
            parse_min_size("500B"),
            Ok(MinSize {
                value: 500,
                has_unit: true
            })
        );
    }

    #[test]
    fn si_units_are_powers_of_1000() {
        assert_eq!(bytes("1KB"), 1000);
        assert_eq!(bytes("2MB"), 2_000_000);
        assert_eq!(bytes("3GB"), 3_000_000_000);
        assert_eq!(bytes("1TB"), 1_000_000_000_000);
        assert_eq!(bytes("1PB"), 1_000_000_000_000_000);
    }

    #[test]
    fn binary_units_are_powers_of_1024() {
        assert_eq!(bytes("1KiB"), 1024);
        assert_eq!(bytes("2MiB"), 2 * 1024 * 1024);
        assert_eq!(bytes("1GiB"), 1 << 30);
        assert_eq!(bytes("1TiB"), 1 << 40);
        assert_eq!(bytes("1PiB"), 1 << 50);
    }

    #[test]
    fn units_are_case_insensitive() {
        assert_eq!(bytes("1kb"), 1000);
        assert_eq!(bytes("1Kb"), 1000);
        assert_eq!(bytes("1kib"), 1024);
        assert_eq!(bytes("1GIB"), 1 << 30);
        assert_eq!(bytes("7b"), 7);
    }

    #[test]
    fn surrounding_whitespace_is_ignored() {
        assert_eq!(bytes(" 1 KB "), 1000);
    }

    #[test]
    fn rejects_garbage() {
        for bad in ["", "KB", "abc", "-5", "1.5KB", "1XB", "1K", "1 2", "+1"] {
            assert!(parse_min_size(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn rejects_overflow() {
        assert!(parse_min_size("99999999999999999999").is_err());
        assert!(parse_min_size("99999999PiB").is_err());
        assert!(parse_min_size("18446744073709551615").is_ok());
    }

    #[test]
    fn error_names_the_unit() {
        let err = parse_min_size("5XB").unwrap_err();
        assert!(err.contains("XB"), "{err}");
        assert!(
            err.ends_with("expected one of B, KB, MB, GB, TB, PB, KiB, MiB, GiB, TiB, PiB"),
            "{err}"
        );
    }
}

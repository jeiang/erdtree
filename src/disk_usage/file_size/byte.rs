use super::super::units::{BinPrefix, PrefixKind, SiPrefix, UnitPrefix};
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

        let mut cursor = Cursor::new(&mut buf[..]);
        // 32 bytes hold any u64 or one-decimal float below 2^64.
        let _ = match base {
            Some(base) => write!(cursor, "{:.1}", value as f64 / base as f64),
            None => write!(cursor, "{value}"),
        };
        let len = usize::try_from(cursor.position()).unwrap_or(0);

        // Only ASCII digits and '.' were written.
        (std::str::from_utf8(&buf[..len]).unwrap_or_default(), unit)
    }
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

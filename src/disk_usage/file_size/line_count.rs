use std::{
    convert::{AsRef, From},
    fmt::{self, Display},
    fs::File,
    io::Read,
    path::Path,
};

use super::utf8_stream::{BUF_SIZE, for_each_chunk};

/// Concerned with measuring file size using line count as a metric.
#[derive(Default)]
pub struct Metric {
    pub value: u64,
}

impl Metric {
    /// Streams the contents of a file given by `path` through a fixed-size buffer and computes
    /// the total number of lines in that file, as `str::lines` would. If a file is not UTF-8
    /// encoded as in the case of a binary jpeg file then `None` will be returned.
    pub fn init(path: impl AsRef<Path>) -> Option<Self> {
        Self::from_reader(File::open(path.as_ref()).ok()?, BUF_SIZE)
    }

    /// Counts lines in `reader` using a buffer of `buf_size` bytes.
    ///
    /// Equivalent to `str::lines().count()`: lines are terminated by `\n` (a preceding `\r` is
    /// part of the terminator and does not matter for counting), a final line without a
    /// terminator counts, and an empty input has zero lines.
    fn from_reader(reader: impl Read, buf_size: usize) -> Option<Self> {
        let mut newlines = 0_u64;
        let mut last_byte = None;

        for_each_chunk(reader, buf_size, |chunk| {
            newlines += chunk.bytes().filter(|&b| b == b'\n').count() as u64;
            last_byte = chunk.as_bytes().last().copied().or(last_byte);
        })?;

        let unterminated = last_byte.is_some_and(|b| b != b'\n');
        Some(Self {
            value: newlines + u64::from(unterminated),
        })
    }
}

impl From<u64> for Metric {
    fn from(value: u64) -> Self {
        Self { value }
    }
}

impl Display for Metric {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        <u64 as Display>::fmt(&self.value, f)
    }
}

#[test]
fn test_line_count() {
    let metric =
        Metric::init("tests/data/nemesis.txt").expect("Expected 'tests/data/nemesis.txt' to exist");

    assert_eq!(metric.value, 4);
}

#[cfg(test)]
mod tests {
    use super::super::utf8_stream::test_support::{BUF_SIZES, INVALID, generated_inputs};
    use super::*;

    fn oracle(data: &str) -> u64 {
        data.lines().count() as u64
    }

    fn streamed(data: &[u8], buf_size: usize) -> Option<u64> {
        Metric::from_reader(data, buf_size).map(|m| m.value)
    }

    #[test]
    fn edge_cases() {
        for (data, expected) in [
            ("", 0),
            ("\n", 1),
            ("\n\n", 2),
            ("a", 1),
            ("a\n", 1),
            ("a\nb", 2),
            ("a\r\nb\r\n", 2),
            ("a\rb", 1),
            ("a\r", 1),
            ("\r", 1),
            ("\r\n", 1),
            ("é€😀\n😀", 2),
        ] {
            assert_eq!(oracle(data), expected, "oracle {data:?}");
            for &size in BUF_SIZES {
                assert_eq!(
                    streamed(data.as_bytes(), size),
                    Some(expected),
                    "{data:?} buf {size}"
                );
            }
        }
    }

    #[test]
    fn matches_oracle_on_generated_inputs() {
        for data in generated_inputs(2000) {
            for &size in BUF_SIZES {
                assert_eq!(
                    streamed(data.as_bytes(), size),
                    Some(oracle(&data)),
                    "{data:?} buf {size}"
                );
            }
        }
    }

    #[test]
    fn input_larger_than_default_buffer() {
        let line = "é€😀 word\r\n";
        let data = line.repeat(BUF_SIZE / line.len() * 3 + 1);
        assert!(data.len() > BUF_SIZE * 2);
        assert_eq!(streamed(data.as_bytes(), BUF_SIZE), Some(oracle(&data)));
        let unterminated = format!("{data}tail");
        assert_eq!(
            streamed(unterminated.as_bytes(), BUF_SIZE),
            Some(oracle(&unterminated))
        );
    }

    #[test]
    fn invalid_utf8_is_none() {
        for &bad in INVALID {
            assert!(std::str::from_utf8(bad).is_err());
            for &size in BUF_SIZES {
                assert_eq!(streamed(bad, size), None, "{bad:?} buf {size}");
            }
        }
    }

    #[test]
    fn invalid_utf8_after_large_valid_prefix_is_none() {
        let mut data = "line\n".repeat(BUF_SIZE).into_bytes();
        data.push(0xff);
        assert_eq!(streamed(&data, BUF_SIZE), None);
    }
}

#[cfg(test)]
use std::path::Path;
use std::{
    convert::From,
    fmt::{self, Display},
    fs::File,
    io::Read,
};

#[cfg(test)]
use super::utf8_stream::for_each_chunk;
use super::utf8_stream::{BUF_SIZE, for_each_chunk_in};

/// Concerned with measuring file size using line count as a metric.
#[derive(Default)]
pub struct Metric {
    pub value: u64,
}

impl Metric {
    /// Streams the contents of a file given by `path` through a fixed-size buffer and computes
    /// the total number of lines in that file, as `str::lines` would. If a file is not UTF-8
    /// encoded as in the case of a binary jpeg file then `None` will be returned.
    #[cfg(test)]
    pub fn init(path: impl AsRef<Path>) -> Option<Self> {
        Self::from_reader(File::open(path.as_ref()).ok()?, BUF_SIZE)
    }

    /// Counts the lines of an open file, reading through `scratch`, which is grown to the read
    /// size when it is smaller. One buffer can serve many files.
    pub fn from_file(file: File, scratch: &mut Vec<u8>) -> Option<Self> {
        if scratch.len() < BUF_SIZE {
            scratch.resize(BUF_SIZE, 0);
        }

        Self::count(file, &mut scratch[..BUF_SIZE])
    }

    /// Counts lines in `reader` using a buffer of `buf_size` bytes.
    ///
    /// Equivalent to `str::lines().count()`: lines are terminated by `\n` (a preceding `\r` is
    /// part of the terminator and does not matter for counting), a final line without a
    /// terminator counts, and an empty input has zero lines.
    #[cfg(test)]
    fn from_reader(reader: impl Read, buf_size: usize) -> Option<Self> {
        Self::tally(|f| for_each_chunk(reader, buf_size, f))
    }

    /// Like [`Self::from_reader`], reading through `buf`.
    fn count(reader: impl Read, buf: &mut [u8]) -> Option<Self> {
        Self::tally(|f| for_each_chunk_in(reader, buf, f))
    }

    /// Counts the lines of the chunks `run` produces.
    fn tally(run: impl FnOnce(&mut dyn FnMut(&str)) -> Option<()>) -> Option<Self> {
        let mut newlines = 0_u64;
        let mut last_byte = None;

        run(&mut |chunk: &str| {
            newlines += count_newlines(chunk.as_bytes());
            last_byte = chunk.as_bytes().last().copied().or(last_byte);
        })?;

        let unterminated = last_byte.is_some_and(|b| b != b'\n');
        Some(Self {
            value: newlines + u64::from(unterminated),
        })
    }
}

/// Number of `\n` bytes in `bytes`. Counting in `u8` lanes of 255 lets the compiler use vector
/// compares and byte adds, an order of magnitude faster than a `u64` counter.
fn count_newlines(bytes: &[u8]) -> u64 {
    bytes
        .chunks(255)
        .map(|lane| {
            let mut n = 0_u8;
            for &b in lane {
                n += u8::from(b == b'\n');
            }
            u64::from(n)
        })
        .sum()
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
    fn newline_count_matches_a_plain_count() {
        for len in [0, 1, 254, 255, 256, 510, 511, 1000] {
            let data: Vec<u8> = (0..len)
                .map(|i| if i % 3 == 0 { b'\n' } else { b'a' })
                .collect();
            let plain = data.iter().fold(0_u64, |n, &b| n + u64::from(b == b'\n'));
            assert_eq!(count_newlines(&data), plain, "{len}");
        }
        assert_eq!(count_newlines(&[b'\n'; 1000]), 1000);
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

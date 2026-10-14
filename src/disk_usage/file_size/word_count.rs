use std::{
    convert::{AsRef, From},
    fmt::{self, Display},
    fs::File,
    io::Read,
    path::Path,
};

use super::utf8_stream::{BUF_SIZE, for_each_chunk};

/// Concerned with measuring file size using word count as a metric.
#[derive(Default)]
pub struct Metric {
    pub value: u64,
}

impl Metric {
    /// Streams the contents of a file given by `path` through a fixed-size buffer and computes
    /// the total number of words in that file. If a file is not UTF-8 encoded as in the case of
    /// a binary jpeg file then `None` will be returned.
    ///
    /// Words are UTF-8 encoded byte sequences delimited by Unicode Derived Core Property `White_Space`.
    pub fn init(path: impl AsRef<Path>) -> Option<Self> {
        Self::from_reader(File::open(path.as_ref()).ok()?, BUF_SIZE)
    }

    /// Counts words in `reader` using a buffer of `buf_size` bytes.
    ///
    /// Equivalent to `str::split_whitespace().count()`: a word starts at each non-whitespace
    /// character that follows whitespace (or the start of input). The "inside a word" state is
    /// carried across chunks, and chunks never split a character.
    fn from_reader(reader: impl Read, buf_size: usize) -> Option<Self> {
        let mut words = 0_u64;
        let mut in_word = false;

        for_each_chunk(reader, buf_size, |chunk| {
            for c in chunk.chars() {
                if c.is_whitespace() {
                    in_word = false;
                } else if !in_word {
                    in_word = true;
                    words += 1;
                }
            }
        })?;

        Some(Self { value: words })
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

    assert_eq!(metric.value, 27);
}

#[cfg(test)]
mod tests {
    use super::super::utf8_stream::test_support::{BUF_SIZES, INVALID, generated_inputs};
    use super::*;

    fn oracle(data: &str) -> u64 {
        data.split_whitespace().count() as u64
    }

    fn streamed(data: &[u8], buf_size: usize) -> Option<u64> {
        Metric::from_reader(data, buf_size).map(|m| m.value)
    }

    #[test]
    fn edge_cases() {
        for (data, expected) in [
            ("", 0),
            (" \n\t", 0),
            ("a", 1),
            ("a b", 2),
            ("  a   b  ", 2),
            ("a\r\nb", 2),
            ("a\u{2003}b", 2),
            ("a\u{85}b\u{a0}c\u{3000}d\u{2028}e", 5),
            ("a\u{200b}b", 1), // zero-width space is not White_Space
            ("é€😀 é€😀", 2),
            ("😀😀😀", 1),
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
    fn word_and_char_spanning_buffer_boundary() {
        // With a 4-byte buffer every 4-byte char fills it exactly and every multi-byte char in
        // the shifted variants straddles a read boundary.
        for prefix in ["", "x", "xx", "xxx"] {
            let data = format!("{prefix}😀€é\u{2003}😀 €");
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
        let unit = "é€😀 w\u{2003}";
        let data = unit.repeat(BUF_SIZE / unit.len() * 3 + 1);
        assert!(data.len() > BUF_SIZE * 2);
        assert_eq!(streamed(data.as_bytes(), BUF_SIZE), Some(oracle(&data)));
        // One enormous word spanning many buffers is still one word.
        let long = "😀".repeat(BUF_SIZE);
        assert_eq!(streamed(long.as_bytes(), BUF_SIZE), Some(1));
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
}

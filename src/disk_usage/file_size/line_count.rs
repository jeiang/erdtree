use std::{fs::File, io::Read};

use super::utf8_stream::{BUF_SIZE, for_each_chunk_in};

/// Counts the lines of an open file, reading through `scratch`, which is grown to the read
/// size when it is smaller. One buffer can serve many files. `size` is what the file system
/// reports for the file, or 0 when unknown.
pub fn from_file(file: File, size: u64, scratch: &mut Vec<u8>) -> Option<u64> {
    if scratch.len() < BUF_SIZE {
        scratch.resize(BUF_SIZE, 0);
    }

    count(file, size, &mut scratch[..BUF_SIZE])
}

/// Counts the lines of `reader`, reading through `buf`, from a file the file system reports as
/// `size` bytes long (0 if unknown). `None` if it is not UTF-8.
///
/// Equivalent to `str::lines().count()`: lines are terminated by `\n` (a preceding `\r` is
/// part of the terminator and does not matter for counting), a final line without a
/// terminator counts, and an empty input has zero lines.
fn count(reader: impl Read, size: u64, buf: &mut [u8]) -> Option<u64> {
    tally(|f| for_each_chunk_in(reader, buf, size, f))
}

/// Counts the lines of the chunks `run` produces.
fn tally(run: impl FnOnce(&mut dyn FnMut(&str)) -> Option<()>) -> Option<u64> {
    let mut newlines = 0_u64;
    let mut last_byte = None;

    run(&mut |chunk: &str| {
        newlines += count_newlines(chunk.as_bytes());
        last_byte = chunk.as_bytes().last().copied().or(last_byte);
    })?;

    let unterminated = last_byte.is_some_and(|b| b != b'\n');
    Some(newlines + u64::from(unterminated))
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

#[cfg(test)]
mod tests {
    use super::super::utf8_stream::test_support::{BUF_SIZES, INVALID, generated_inputs};
    use super::*;

    fn oracle(data: &str) -> u64 {
        data.lines().count() as u64
    }

    /// The count of `data` read through a buffer of `buf_size` bytes, which is the same whether
    /// the size of the input is known or not.
    fn streamed(data: &[u8], buf_size: usize) -> Option<u64> {
        let mut buf = vec![0; buf_size];
        let unknown = count(data, 0, &mut buf);
        let known = count(data, data.len() as u64, &mut buf);
        assert_eq!(unknown, known, "{data:?} buf {buf_size}");
        known
    }

    #[test]
    fn lines_of_a_file() {
        let path = "tests/data/nemesis.txt";
        let size = std::fs::metadata(path).unwrap().len();
        let count = from_file(File::open(path).unwrap(), size, &mut vec![]);
        assert_eq!(count, Some(4));
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

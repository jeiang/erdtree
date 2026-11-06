#[cfg(test)]
use std::path::Path;
use std::{fs::File, io::Read};

#[cfg(test)]
use super::utf8_stream::for_each_chunk;
use super::utf8_stream::{BUF_SIZE, for_each_chunk_in};

/// Streams the contents of a file given by `path` through a fixed-size buffer and computes
/// the total number of words in that file. If a file is not UTF-8 encoded as in the case of
/// a binary jpeg file then `None` will be returned.
///
/// Words are UTF-8 encoded byte sequences delimited by Unicode Derived Core Property `White_Space`.
#[cfg(test)]
fn init(path: impl AsRef<Path>) -> Option<u64> {
    from_reader(File::open(path.as_ref()).ok()?, BUF_SIZE)
}

/// Counts the words of an open file, reading through `scratch`, which is grown to the read
/// size when it is smaller. One buffer can serve many files. `size` is what the file system
/// reports for the file, or 0 when unknown.
pub fn from_file(file: File, size: u64, scratch: &mut Vec<u8>) -> Option<u64> {
    if scratch.len() < BUF_SIZE {
        scratch.resize(BUF_SIZE, 0);
    }

    count(file, size, &mut scratch[..BUF_SIZE])
}

/// Counts words in `reader` using a buffer of `buf_size` bytes.
///
/// Equivalent to `str::split_whitespace().count()`: a word starts at each non-whitespace
/// character that follows whitespace (or the start of input). The "inside a word" state is
/// carried across chunks, and chunks never split a character.
#[cfg(test)]
fn from_reader(reader: impl Read, buf_size: usize) -> Option<u64> {
    tally(|f| for_each_chunk(reader, buf_size, f))
}

/// Like [`from_reader`], reading through `buf`, from a file the file system reports as
/// `size` bytes long (0 if unknown).
fn count(reader: impl Read, size: u64, buf: &mut [u8]) -> Option<u64> {
    tally(|f| for_each_chunk_in(reader, buf, size, f))
}

/// Counts the words of the chunks `run` produces.
fn tally(run: impl FnOnce(&mut dyn FnMut(&str)) -> Option<()>) -> Option<u64> {
    let mut words = 0_u64;
    let mut in_word = false;

    run(&mut |chunk: &str| {
        if chunk.is_ascii() {
            words += ascii_words(chunk.as_bytes(), &mut in_word);
            return;
        }

        for c in chunk.chars() {
            if c.is_whitespace() {
                in_word = false;
            } else if !in_word {
                in_word = true;
                words += 1;
            }
        }
    })?;

    Some(words)
}

/// Counts the words that start in the ASCII `chunk`, given whether a word is open before it, and
/// updates that state. ASCII whitespace is exactly U+0009 to U+000D and U+0020, as
/// `char::is_whitespace` has it. Counting in `u8` lanes of 255 lets the compiler vectorize.
fn ascii_words(chunk: &[u8], in_word: &mut bool) -> u64 {
    let is_space = |b: u8| b == b' ' || b.wrapping_sub(9) < 5;

    let (Some(&first), Some(&last)) = (chunk.first(), chunk.last()) else {
        return 0;
    };

    let mut words = u64::from(!*in_word && !is_space(first));

    for (before, after) in chunk[..chunk.len() - 1]
        .chunks(255)
        .zip(chunk[1..].chunks(255))
    {
        let mut n = 0_u8;
        for (&a, &b) in before.iter().zip(after) {
            n += u8::from(is_space(a) & !is_space(b));
        }
        words += u64::from(n);
    }

    *in_word = !is_space(last);
    words
}

#[test]
fn test_line_count() {
    let count = init("tests/data/nemesis.txt").expect("Expected 'tests/data/nemesis.txt' to exist");

    assert_eq!(count, 27);
}

#[cfg(test)]
mod tests {
    use super::super::utf8_stream::test_support::{BUF_SIZES, INVALID, generated_inputs};
    use super::*;

    fn oracle(data: &str) -> u64 {
        data.split_whitespace().count() as u64
    }

    fn streamed(data: &[u8], buf_size: usize) -> Option<u64> {
        from_reader(data, buf_size)
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
    fn ascii_fast_path_matches_split_whitespace() {
        // Every ASCII byte class, in runs of every length around the lane size, and chunks
        // that start inside a word.
        let bytes = [
            b'a', b' ', b'\t', b'\n', 0x0b, 0x0c, b'\r', 0x1c, 0x1f, 0x7f, b'-', b'0',
        ];
        let mut state = 0x2545_F491_4F6C_DD1D_u64;

        for len in [0, 1, 2, 254, 255, 256, 257, 700] {
            for _ in 0..40 {
                let data: String = (0..len)
                    .map(|_| {
                        state ^= state << 13;
                        state ^= state >> 7;
                        state ^= state << 17;
                        char::from(bytes[usize::try_from(state % 12).unwrap()])
                    })
                    .collect();

                assert_eq!(
                    streamed(data.as_bytes(), BUF_SIZE),
                    Some(oracle(&data)),
                    "{data:?}"
                );
                assert_eq!(
                    streamed(data.as_bytes(), 4),
                    Some(oracle(&data)),
                    "{data:?}"
                );
            }
        }
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

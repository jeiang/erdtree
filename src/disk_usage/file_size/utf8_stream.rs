use std::io::{ErrorKind, Read};

/// Size of the read buffer used when streaming file contents.
pub const BUF_SIZE: usize = 64 * 1024;

/// Streams `reader` through a fixed-size buffer of `buf_size` bytes (at least 4), validating UTF-8
/// as it goes and handing each valid chunk to `f`. A multi-byte character split across two reads
/// is carried over and delivered whole in the next chunk, so `f` never sees a partial character.
///
/// Returns `None` if the stream is not valid UTF-8 or an I/O error occurs, mirroring
/// `fs::read_to_string`. `f` may already have been called by then; callers must discard any
/// partial result.
#[cfg(test)]
pub fn for_each_chunk(reader: impl Read, buf_size: usize, f: impl FnMut(&str)) -> Option<()> {
    for_each_chunk_in(reader, &mut vec![0u8; buf_size.max(4)], 0, f)
}

/// Like [`for_each_chunk`], reading through `buf`, which must hold at least 4 bytes. Reusing one
/// buffer for many files saves allocating and zeroing it per file.
///
/// `size` is the size the file system reports for a regular file, or 0 when unknown (a file that
/// reports 0 may still have contents, as in `/proc`). Once that many bytes have been read the
/// stream ends there, without the read that would only find the end of the file. A file that
/// grows while it is read is counted up to the size reported.
pub fn for_each_chunk_in(
    mut reader: impl Read,
    buf: &mut [u8],
    size: u64,
    mut f: impl FnMut(&str),
) -> Option<()> {
    debug_assert!(buf.len() >= 4);
    let mut filled = 0;
    let mut total = 0_u64;

    loop {
        let n = match reader.read(&mut buf[filled..]) {
            Ok(n) => n,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(_) => return None,
        };

        if n == 0 {
            // EOF: leftover bytes are a truncated character.
            return (filled == 0).then_some(());
        }
        filled += n;
        total += n as u64;

        match std::str::from_utf8(&buf[..filled]) {
            Ok(s) => {
                f(s);
                filled = 0;

                if total == size {
                    return Some(());
                }
            },
            Err(e) => {
                if e.error_len().is_some() {
                    return None;
                }
                let valid = e.valid_up_to();
                f(std::str::from_utf8(&buf[..valid]).ok()?);

                if total == size {
                    // A truncated character ends the file.
                    return None;
                }

                buf.copy_within(valid..filled, 0);
                filled -= valid;
            },
        }
    }
}

/// Shared test helpers: generated inputs plus the original whole-file implementations as oracles.
#[cfg(test)]
pub mod test_support {
    /// Pieces mixing ASCII, 2/3/4-byte characters, Unicode whitespace and line terminators.
    const PIECES: &[&str] = &[
        "a", "bc", " ", "\n", "\r", "\r\n", "\t", "é", "€", "😀", "\u{2003}", "\u{85}", "\u{a0}",
        "\u{200b}", "\u{2028}", "\u{3000}",
    ];

    /// Deterministic pseudo-random inputs (xorshift), no extra dev-dependencies.
    pub fn generated_inputs(count: usize) -> Vec<String> {
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        let mut next = move |modulus: usize| -> usize {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            usize::try_from(state % modulus as u64).expect("modulus fits in usize")
        };
        (0..count)
            .map(|_| {
                let len = next(40);
                (0..len).map(|_| PIECES[next(PIECES.len())]).collect()
            })
            .collect()
    }

    /// Buffer sizes small enough to split characters at every possible offset.
    pub const BUF_SIZES: &[usize] = &[4, 5, 6, 7, 8, 13, 64];

    /// Byte strings that `read_to_string` rejects.
    pub const INVALID: &[&[u8]] = &[
        b"\xff",
        b"ok\xff ok",
        b"\xc3",         // truncated 2-byte char at EOF
        b"abc\xe2\x82",  // truncated 3-byte char at EOF
        b"\xf0\x9f\x98", // truncated 4-byte char at EOF
        b"\xe2\x28\xa1", // bad continuation
        b"\xc0\x80",     // overlong
    ];
}

#[cfg(test)]
mod test {
    use super::*;

    /// Counts the reads made of the slice behind it.
    struct Counting<'a> {
        data: &'a [u8],
        reads: usize,
    }

    impl Read for Counting<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.reads += 1;
            self.data.read(buf)
        }
    }

    fn run(data: &[u8], size: u64) -> (Option<String>, usize) {
        let mut reader = Counting { data, reads: 0 };
        let mut text = String::new();
        let done = for_each_chunk_in(&mut reader, &mut [0; 8], size, |s| text.push_str(s));
        (done.map(|()| text), reader.reads)
    }

    #[test]
    fn a_known_size_saves_the_read_that_finds_the_end() {
        let data = "h\u{e9}llo w\u{f6}rld".as_bytes();
        let size = data.len() as u64;

        let (text, with_size) = run(data, size);
        assert_eq!(text.as_deref(), Some("h\u{e9}llo w\u{f6}rld"));

        let (text, without) = run(data, 0);
        assert_eq!(text.as_deref(), Some("h\u{e9}llo w\u{f6}rld"));
        assert_eq!(without, with_size + 1);
    }

    #[test]
    fn a_size_too_large_is_harmless() {
        // The end is found by reading, as for a file of unknown size.
        assert_eq!(run(b"some text", 100).0.as_deref(), Some("some text"));
    }

    #[test]
    fn a_character_cut_off_at_the_size_is_invalid() {
        let data = "a\u{e9}".as_bytes();
        assert_eq!(run(&data[..2], 2).0, None);
        assert_eq!(run(&data[..2], 0).0, None);
    }
}

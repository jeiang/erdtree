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
pub fn for_each_chunk(
    mut reader: impl Read,
    buf_size: usize,
    mut f: impl FnMut(&str),
) -> Option<()> {
    let mut buf = vec![0u8; buf_size.max(4)];
    let mut filled = 0;

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

        match std::str::from_utf8(&buf[..filled]) {
            Ok(s) => {
                f(s);
                filled = 0;
            },
            Err(e) => {
                if e.error_len().is_some() {
                    return None;
                }
                let valid = e.valid_up_to();
                f(std::str::from_utf8(&buf[..valid]).ok()?);
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

//! Byte-level building blocks of the output: decimal numbers, padding, lossy names and prefetch
//! hints. A leaf module, so the size formats and the tree's columns and widths use it without
//! depending on the renderer.

use std::ffi::OsStr;

/// Decimal digits of the numbers 0 to 99, two bytes each.
#[allow(clippy::cast_possible_truncation)] // `n` is below 100.
const PAIRS: [u8; 200] = {
    let mut table = [0_u8; 200];
    let mut n = 0;
    while n < 100 {
        table[2 * n] = b'0' + (n / 10) as u8;
        table[2 * n + 1] = b'0' + (n % 10) as u8;
        n += 1;
    }
    table
};

/// Number of decimal digits of `n`; 1 for 0.
pub const fn digits(n: u64) -> usize {
    if n == 0 { 1 } else { n.ilog10() as usize + 1 }
}

/// Writes `n` in decimal at the end of `buf` and returns the index of its first digit.
// The casts are of values below 100.
#[allow(clippy::cast_possible_truncation)]
pub fn u64_into(buf: &mut [u8; 20], mut n: u64) -> usize {
    let mut pos = buf.len();

    while n >= 100 {
        let pair = (n % 100) as usize * 2;
        n /= 100;
        pos -= 2;
        buf[pos..pos + 2].copy_from_slice(&PAIRS[pair..pair + 2]);
    }

    if n >= 10 {
        let pair = n as usize * 2;
        pos -= 2;
        buf[pos..pos + 2].copy_from_slice(&PAIRS[pair..pair + 2]);
    } else {
        pos -= 1;
        buf[pos] = b'0' + n as u8;
    }

    pos
}

pub fn push_u64(out: &mut Vec<u8>, n: u64) {
    let mut buf = [0_u8; 20];
    let pos = u64_into(&mut buf, n);
    out.extend_from_slice(&buf[pos..]);
}

/// Appends `n`, which must be below 100, as two digits.
#[cfg(unix)]
pub fn push_pair(out: &mut Vec<u8>, n: u32) {
    let pair = (n as usize % 100) * 2;
    out.extend_from_slice(&PAIRS[pair..pair + 2]);
}

#[inline]
pub fn push_spaces(out: &mut Vec<u8>, n: usize) {
    out.resize(out.len() + n, b' ');
}

/// Appends `n` in decimal, right-aligned to `width` like `{n:>width$}`.
pub fn push_u64_padded(out: &mut Vec<u8>, n: u64, width: usize) {
    push_spaces(out, width.saturating_sub(digits(n)));
    push_u64(out, n);
}

/// Appends `s` right-aligned to `width` characters like `{s:>width$}`, which counts characters
/// and not bytes.
pub fn push_str_padded(out: &mut Vec<u8>, s: &str, width: usize) {
    let chars = if s.is_ascii() {
        s.len()
    } else {
        s.chars().count()
    };

    push_spaces(out, width.saturating_sub(chars));
    out.extend_from_slice(s.as_bytes());
}

/// Hints the CPU to load the cache line of `item`, if there is one. Rows of the flat layouts are
/// in the order of a sort, so their data is scattered and each load waits for memory unless it is
/// requested early.
#[cfg(target_arch = "x86_64")]
#[inline]
pub fn prefetch<T>(item: Option<&T>) {
    if let Some(item) = item {
        // SAFETY: a prefetch never faults and has no effect on memory. The intrinsic is an
        // unsafe `#[target_feature]` function on 1.88 (the MSRV) and still on 1.99.
        unsafe {
            std::arch::x86_64::_mm_prefetch::<{ std::arch::x86_64::_MM_HINT_T0 }>(
                std::ptr::from_ref(item).cast(),
            );
        }
    }
}

/// Does nothing: only x86-64 has a stable prefetch.
#[cfg(not(target_arch = "x86_64"))]
#[inline]
pub const fn prefetch<T>(_item: Option<&T>) {}

/// Appends `name` as [`OsStr::to_string_lossy`] shows it.
#[inline]
pub fn push_lossy(out: &mut Vec<u8>, name: &OsStr) {
    match name.to_str() {
        Some(s) => out.extend_from_slice(s.as_bytes()),
        None => out.extend_from_slice(name.to_string_lossy().as_bytes()),
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn numbers_match_display() {
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        let mut values = vec![0, 1, 9, 10, 99, 100, 101, 999, 1000, u64::MAX, u64::MAX - 1];
        for k in 0..64 {
            values.extend([(1_u64 << k).wrapping_sub(1), 1 << k, (1 << k) + 1]);
        }
        for _ in 0..20_000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            values.push(state >> (state % 64));
        }

        for v in values {
            let mut out = vec![];
            push_u64(&mut out, v);
            assert_eq!(String::from_utf8(out).unwrap(), v.to_string());
            assert_eq!(digits(v), v.to_string().len());

            for width in [0, 3, 12, 25] {
                let mut out = vec![];
                push_u64_padded(&mut out, v, width);
                assert_eq!(String::from_utf8(out).unwrap(), format!("{v:>width$}"));
            }
        }
    }

    #[test]
    fn strings_pad_by_chars() {
        for s in ["", "a", "root", "h\u{e9}l\u{e8}ne", "\u{4e2d}\u{6587}"] {
            for width in [0, 1, 4, 9] {
                let mut out = vec![];
                push_str_padded(&mut out, s, width);
                assert_eq!(String::from_utf8(out).unwrap(), format!("{s:>width$}"));
            }
        }
    }
}

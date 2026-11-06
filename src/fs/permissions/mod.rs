#[cfg(test)]
mod test;

use super::stat::Kind;

/// Symbolic notation of `mode` as `ls -l` prints it, except that a regular file is `.` (as in
/// exa): the file type (`?` when unknown) and the `rwx` triads of user, group and other, with
/// setuid, setgid and the sticky bit shown as `s`/`S` and `t`/`T` in the execute slot.
pub const fn symbolic(mode: u32) -> [u8; 10] {
    let file_type = match Kind::from_mode(mode) {
        Kind::Dir => b'd',
        Kind::File => b'.',
        Kind::Symlink => b'l',
        Kind::Fifo => b'p',
        Kind::Socket => b's',
        Kind::CharDevice => b'c',
        Kind::BlockDevice => b'b',
        Kind::Other => b'?',
    };
    let [ur, uw, ux] = triad(mode >> 6, mode & 0o4000 != 0, b's');
    let [gr, gw, gx] = triad(mode >> 3, mode & 0o2000 != 0, b's');
    let [or, ow, ox] = triad(mode, mode & 0o1000 != 0, b't');

    [file_type, ur, uw, ux, gr, gw, gx, or, ow, ox]
}

/// The `rwx` triad in the low three bits of `bits`; `special` replaces the execute slot with
/// `mark` when executable and its uppercase otherwise.
const fn triad(bits: u32, special: bool, mark: u8) -> [u8; 3] {
    let exec = bits & 0o1 != 0;
    let x = match (exec, special) {
        (false, false) => b'-',
        (true, false) => b'x',
        (false, true) => mark.to_ascii_uppercase(),
        (true, true) => mark,
    };

    [
        if bits & 0o4 == 0 { b'-' } else { b'r' },
        if bits & 0o2 == 0 { b'-' } else { b'w' },
        x,
    ]
}

/// Octal notation of the permission bits of `mode`, zero-padded to four digits.
pub fn octal(mode: u32) -> String {
    format!("{:04o}", mode & 0o7777)
}

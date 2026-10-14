use crossterm::{cursor, ExecutableCommand};
use std::io;
use terminal_size::terminal_size;

/// Restore terminal settings.
pub fn restore() {
    io::stdout()
        .execute(cursor::Show)
        .expect("Failed to restore cursor");
}

/// Attempts to get the current size of the tty's window. Returns `None` if stdout isn't tty or if
/// failed to get width.
pub fn get_window_width() -> Option<usize> {
    Some(usize::from(terminal_size()?.0 .0))
}

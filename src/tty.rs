use terminal_size::terminal_size;

/// Attempts to get the current size of the tty's window. Returns `None` if stdout isn't tty or if
/// failed to get width.
pub fn get_window_width() -> Option<usize> {
    Some(usize::from(terminal_size()?.0.0))
}

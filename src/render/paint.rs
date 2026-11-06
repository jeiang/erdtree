use nu_ansi_term::Style;

/// The escape sequences that open and close `style`, built once so that painting a string is
/// two copies around it.
pub struct Esc {
    pub pre: Box<[u8]>,
    pub suf: Box<[u8]>,
}

impl Esc {
    /// The sequences of `style`; both are empty for a plain style.
    pub fn new(style: Style) -> Self {
        Self {
            pre: style.prefix().to_string().into_bytes().into(),
            suf: style.suffix().to_string().into_bytes().into(),
        }
    }
}

/// An [`Esc`] for each style of a file, as is and in bold.
pub struct Painter {
    pub plain: Esc,
    pub bold: Esc,
}

impl Painter {
    pub fn new(style: Style) -> Self {
        Self {
            plain: Esc::new(style),
            // Plain file styles must not acquire bold when rendered as names or icons.
            bold: Esc::new(if style.is_plain() {
                style
            } else {
                style.bold()
            }),
        }
    }
}

/// Writes a body between a style's precomputed escape sequences.
#[inline]
pub fn paint(out: &mut Vec<u8>, esc: Option<&Esc>, body: impl FnOnce(&mut Vec<u8>)) {
    if let Some(esc) = esc {
        out.extend_from_slice(&esc.pre);
        body(out);
        out.extend_from_slice(&esc.suf);
    } else {
        body(out);
    }
}

/// Writes the placeholder right-aligned to a column's display width.
pub fn placeholder(out: &mut Vec<u8>, width: usize) {
    crate::bytes::push_str_padded(out, crate::styles::PLACEHOLDER, width);
}

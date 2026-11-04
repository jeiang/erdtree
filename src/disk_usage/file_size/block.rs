use std::fmt::{self, Display};

#[derive(Default)]
pub struct Metric {
    pub value: u64,
}

impl Display for Metric {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        <u64 as Display>::fmt(&self.value, f)
    }
}

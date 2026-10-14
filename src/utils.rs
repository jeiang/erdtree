#[macro_export]
/// Ruby-like way to crate a hashmap.
macro_rules! hash {
    ( $( $( $k:literal)|* => $v:expr_2021 ),* ) => {
        {
            let mut hash = std::collections::HashMap::new();
            $(
                $( hash.insert($k, $v); )*
            )*
            hash
        }
    };
    ( $( $k:expr_2021 => $v:expr_2021 ),* ) => {
        {
            let mut hash = std::collections::HashMap::new();
            $( hash.insert($k, $v); )*
            hash
        }
    };
}

/// How many integral digits are there?
#[inline]
pub const fn num_integral(value: u64) -> usize {
    if value == 0 {
        return 0;
    }
    value.ilog10() as usize + 1
}

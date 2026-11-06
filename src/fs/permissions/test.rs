use super::{octal, symbolic};

fn sym(mode: u32) -> String {
    String::from_utf8(symbolic(mode).to_vec()).unwrap()
}

#[test]
fn file_types() {
    let cases = [
        (0o040_755, "drwxr-xr-x"),
        (0o100_644, ".rw-r--r--"),
        (0o120_777, "lrwxrwxrwx"),
        (0o010_600, "prw-------"),
        (0o140_755, "srwxr-xr-x"),
        (0o020_620, "crw--w----"),
        (0o060_660, "brw-rw----"),
    ];

    for (mode, expected) in cases {
        assert_eq!(sym(mode), expected, "{mode:o}");
    }
}

#[test]
fn unknown_file_type() {
    for kind in [0, 0o030_000, 0o050_000, 0o070_000, 0o160_000, 0o170_000] {
        assert_eq!(sym(kind | 0o644), "?rw-r--r--", "{kind:o}");
    }
}

#[test]
fn triads() {
    let cases = [
        (0o100_000, ".---------"),
        (0o100_421, ".r---w---x"),
        (0o100_356, ".-wxr-xrw-"),
        (0o100_777, ".rwxrwxrwx"),
    ];

    for (mode, expected) in cases {
        assert_eq!(sym(mode), expected, "{mode:o}");
    }
}

#[test]
fn special_bits() {
    let cases = [
        (0o101_644, ".rw-r--r-T", "1644"),
        (0o102_644, ".rw-r-Sr--", "2644"),
        (0o104_644, ".rwSr--r--", "4644"),
        (0o107_644, ".rwSr-Sr-T", "7644"),
        (0o107_777, ".rwsrwsrwt", "7777"),
        (0o041_777, "drwxrwxrwt", "1777"),
        (0o106_711, ".rws--s--x", "6711"),
    ];

    for (mode, expected_sym, expected_oct) in cases {
        assert_eq!(sym(mode), expected_sym, "{mode:o}");
        assert_eq!(octal(mode), expected_oct, "{mode:o}");
    }
}

#[test]
fn octal_pads_and_drops_file_type() {
    assert_eq!(octal(0o100_644), "0644");
    assert_eq!(octal(0o040_000), "0000");
    assert_eq!(octal(0o120_755), "0755");
}

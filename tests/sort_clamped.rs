#[cfg(unix)]
mod utils;

/// Timestamps beyond the year 2116 (and before 1824) still sort by time, not by name.
#[cfg(unix)]
#[test]
fn sort_mod_far_future() -> Result<(), Box<dyn std::error::Error>> {
    use std::time::{Duration, UNIX_EPOCH};

    let tmp = tempfile::TempDir::new()?;
    // Name order is a, b, c; newest first is a (2300), c (2250), b (2200).
    for (name, year) in [("a", 2300), ("b", 2200), ("c", 2250)] {
        let secs = (year - 1970) * 31_556_952;
        let file = std::fs::File::create(tmp.path().join(name))?;
        file.set_modified(UNIX_EPOCH + Duration::from_secs(secs))?;
    }

    let root = tmp.path().to_string_lossy().into_owned();
    for layout in ["regular", "inverted"] {
        for (sort, expected) in [("mod", "acb"), ("rmod", "bca")] {
            let out = utils::run_cmd(&["--sort", sort, "--layout", layout, "--no-report", &root]);
            let names: String = out
                .lines()
                .filter_map(|l| l.split_whitespace().last())
                // The root row ends in the random temp dir name, so match whole names only.
                .filter(|n| matches!(*n, "a" | "b" | "c"))
                .collect();
            // The regular layout lists the sorted order bottom up.
            let names = if layout == "regular" {
                names.chars().rev().collect()
            } else {
                names
            };
            assert_eq!(names, expected, "{sort} {layout}\n{out}");
        }
    }
    Ok(())
}

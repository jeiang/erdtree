#[cfg(unix)]
mod utils;

/// `--time-format iso-strict` prints the local offset as `+HH:MM`, across zones and DST changes.
#[cfg(unix)]
#[test]
fn iso_strict_offsets() -> Result<(), Box<dyn std::error::Error>> {
    use std::time::{Duration, UNIX_EPOCH};

    // (time zone, UTC seconds, expected stamp)
    let cases = [
        ("UTC", 1_719_835_200, "2024-07-01T12:00:00+00:00"),
        (
            "America/New_York",
            1_719_835_200,
            "2024-07-01T08:00:00-04:00",
        ),
        (
            "America/New_York",
            1_705_320_000,
            "2024-01-15T07:00:00-05:00",
        ),
        (
            "America/New_York",
            1_710_053_999,
            "2024-03-10T01:59:59-05:00",
        ),
        (
            "America/New_York",
            1_710_054_000,
            "2024-03-10T03:00:00-04:00",
        ),
        ("Asia/Kolkata", 1_719_835_200, "2024-07-01T17:30:00+05:30"),
        ("Asia/Kathmandu", 1_705_320_000, "2024-01-15T17:45:00+05:45"),
        (
            "Australia/Lord_Howe",
            1_705_320_000,
            "2024-01-15T23:00:00+11:00",
        ),
        (
            "Australia/Lord_Howe",
            1_719_835_200,
            "2024-07-01T22:30:00+10:30",
        ),
    ];

    let tmp = tempfile::TempDir::new()?;
    let root = tmp.path().to_string_lossy().into_owned();

    for (i, (_, secs, _)) in cases.iter().enumerate() {
        let file = std::fs::File::create(tmp.path().join(format!("f{i}")))?;
        file.set_modified(UNIX_EPOCH + Duration::from_secs(*secs))?;
    }

    for zone in [
        "UTC",
        "America/New_York",
        "Asia/Kolkata",
        "Asia/Kathmandu",
        "Australia/Lord_Howe",
    ] {
        // Sandboxed builds (e.g. `nix flake check`) may lack the tz database; chrono then
        // falls back to UTC, which says nothing about offsets.
        if zone != "UTC" && !has_zone(zone) {
            eprintln!("skipping {zone}: not in the tz database");
            continue;
        }

        let out = utils::Erd::new()
            .args([
                "-l",
                "--time-format",
                "iso-strict",
                "-C",
                "none",
                "--no-report",
                &root,
            ])
            .env("TZ", zone)
            .ok()
            .raw();

        for (i, (case_zone, _, expected)) in cases.iter().enumerate() {
            if *case_zone != zone {
                continue;
            }
            let row = out
                .lines()
                .find(|l| l.split_whitespace().last() == Some(format!("f{i}").as_str()))
                .unwrap_or_else(|| panic!("no row f{i}\n{out}"));
            assert!(row.contains(expected), "{zone}: {row:?} lacks {expected}");
        }
    }
    Ok(())
}

/// Whether the tz database has `zone`, in `$TZDIR` or the usual locations.
#[cfg(unix)]
fn has_zone(zone: &str) -> bool {
    std::env::var_os("TZDIR")
        .map(std::path::PathBuf::from)
        .into_iter()
        .chain(
            [
                "/usr/share/zoneinfo",
                "/usr/lib/zoneinfo",
                "/usr/share/lib/zoneinfo",
            ]
            .map(Into::into),
        )
        .any(|dir| dir.join(zone).is_file())
}

//! In-process local offsets without chrono's macOS CoreFoundation dependency.
//!
//! `tz-rs` shares the ancestry of chrono's Unix TZif/POSIX parser. Load the zone once,
//! then share its immutable transition table across render workers: unlike `localtime_r`,
//! lookups neither lock libc's global zone nor check macOS zone-change notifications.

use std::sync::LazyLock;
use tz::{TimeZone, TimeZoneSettings};

/// The UTC offset in seconds of the local time zone at `secs`.
pub fn local_offset(secs: i64) -> i32 {
    static ZONE: LazyLock<TimeZone> = LazyLock::new(current_zone);
    ZONE.find_local_time_type(secs).map_or_else(
        |_| {
            // chrono retains the last transition's offset when an old TZif has no footer;
            // tz-rs instead reports that the local time type is unspecified.
            let zone = ZONE.as_ref();
            zone.transitions().last().map_or(0, |last| {
                zone.local_time_types()[last.local_time_type_index()].ut_offset()
            })
        },
        tz::LocalTimeType::ut_offset,
    )
}

/// Matches chrono's `TZ` handling, including empty strings and invalid-value fallback.
fn current_zone() -> TimeZone {
    const SETTINGS: TimeZoneSettings<'_> = TimeZoneSettings::new(
        &[
            "/usr/share/zoneinfo",
            "/share/zoneinfo",
            "/etc/zoneinfo",
            "/usr/share/lib/zoneinfo",
        ],
        TimeZoneSettings::DEFAULT_READ_FILE_FN,
    );
    let value = std::env::var("TZ").ok();
    if value.as_deref() == Some("") {
        return TimeZone::utc();
    }
    value
        .as_deref()
        .map_or_else(|| SETTINGS.parse_local(), |tz| SETTINGS.parse_posix_tz(tz))
        .or_else(|_| SETTINGS.parse_local())
        .unwrap_or_else(|_| TimeZone::utc())
}

#[cfg(test)]
mod test {
    use super::local_offset;
    use chrono::{DateTime, Local, Offset, TimeZone};
    use std::process::Command;

    /// Name of the child test that compares with chrono under the `TZ` it is given.
    const CHILD: &str = "render::long::zone::test::child_matches_chrono";

    /// Where the system zones are.
    const TZDB_LOCATION: &str = "/usr/share/zoneinfo";

    /// Instants over the whole range of the long view, and one each day from 1970 to 2100 at an
    /// hour that moves through the day.
    fn instants() -> Vec<i64> {
        let mut out = vec![
            -(1 << 40),
            -2_208_988_800,
            -1,
            0,
            1,
            2_147_483_647,
            4_102_444_800,
            1 << 40,
        ];
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        for _ in 0..4000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            out.push(i64::try_from(state % 8_000_000_000).unwrap() - 3_000_000_000);
        }
        out.extend((0..47_482).map(|day| day * 86_400 + day * 7 % 24 * 3600 + day % 3600));
        out
    }

    /// Run by [`zones_match_chrono`] in a child process with `TZ` set: chrono's `Local` reads
    /// `TZ` from the environment, which a test must not change in its own process.
    #[test]
    #[ignore = "run by zones_match_chrono with TZ set"]
    fn child_matches_chrono() {
        let tz = std::env::var("TZ").ok();
        let mut instants = instants();
        // Include both sides of every recorded transition, not just sampled dates.
        let zone = super::current_zone();
        for transition in zone.as_ref().transitions() {
            let second = transition.unix_leap_time();
            instants.extend([second - 1, second, second + 1]);
        }
        for secs in instants {
            let utc = DateTime::from_timestamp(secs, 0).unwrap().naive_utc();
            let expected = Local.offset_from_utc_datetime(&utc).fix().local_minus_utc();
            assert_eq!(local_offset(secs), expected, "TZ={tz:?} at {secs}");
        }
    }

    /// Every zone of the system database, POSIX rules and invalid `TZ` strings give chrono's
    /// offsets, including fallback to the system zone.
    #[test]
    fn zones_match_chrono() {
        let mut values: Vec<String> = [
            "",
            "UTC",
            ":UTC",
            "/etc/localtime",
            ":America/Nuuk",
            "/usr/share/zoneinfo/America/Nuuk",
            "EST",
            "EST5EDT",
            "CET-1CEST,M3.5.0,M10.5.0/3",
            "<+0330>-3:30",
            "NZST-12NZDT,M9.5.0,M4.1.0/3",
            "this-zone-does-not-exist",
            ":",
            ":this-zone-does-not-exist",
            "UTC25",
            " ",
            " UTC0 ",
            "EST5EDT,M3.2.0/2,M11.1.0/2",
            "EST5EDT,J60/2,300/2",
            "EST5EDT,60/2,300/2",
            "<-02>2<-01>,M3.5.0/-1,M10.5.0/0",
        ]
        .map(String::from)
        .into();

        let mut stack = vec![std::path::PathBuf::from(TZDB_LOCATION)];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if std::fs::read(&path).unwrap().starts_with(b"TZif")
                    && let Ok(name) = path.strip_prefix(TZDB_LOCATION)
                {
                    values.push(name.to_string_lossy().into_owned());
                }
            }
        }

        let exe = std::env::current_exe().unwrap();
        let mut unset = Command::new(&exe);
        unset
            .args([CHILD, "--exact", "--ignored", "--quiet"])
            .env_remove("TZ");
        let mut runs = vec![unset];
        for value in &values {
            let mut cmd = Command::new(&exe);
            cmd.args([CHILD, "--exact", "--ignored", "--quiet"])
                .env("TZ", value);
            runs.push(cmd);
        }

        let failed: Vec<_> = std::thread::scope(|s| {
            let handles: Vec<_> = runs
                .chunks_mut(64)
                .map(|chunk| {
                    s.spawn(move || {
                        chunk
                            .iter_mut()
                            .filter_map(|cmd| {
                                let out = cmd.output().unwrap();
                                (!out.status.success())
                                    .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().unwrap())
                .collect()
        });

        assert!(failed.is_empty(), "{failed:#?}");
    }
}

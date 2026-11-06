//! Timestamps of the long view without chrono's per-row time zone conversion.

use super::super::bytes::push_pair;
use crate::context::time::Format;
use chrono::{DateTime, Datelike, NaiveDateTime, Timelike};
#[cfg(not(target_os = "macos"))]
use chrono::{Local, Offset, TimeZone};
use std::time::{SystemTime, UNIX_EPOCH};

/// Timestamps from 2100 on, and before 1970, take chrono's route: the fields below assume
/// four-digit years.
const FAST_LIMIT: i64 = 4_102_444_800;

/// Length of the spans over which one UTC offset is reused.
const WINDOW: i64 = 3600;

/// Spans remembered; file times cluster in time, so a small direct-mapped table suffices.
const SLOTS: usize = 256;

/// Selects the slot of a window.
const SLOT_MASK: i64 = 255;

/// One remembered span.
#[derive(Clone, Copy)]
struct Slot {
    window: i64,

    /// The offset in seconds, `None` if it changes within the span.
    offset: Option<i32>,
    used: bool,
}

/// The UTC offset of the local time zone at `secs`, as chrono finds it.
#[cfg(not(target_os = "macos"))]
pub fn local_offset(secs: i64) -> i32 {
    DateTime::from_timestamp(secs, 0).map_or(0, |utc| {
        Local
            .offset_from_utc_datetime(&utc.naive_utc())
            .fix()
            .local_minus_utc()
    })
}

/// The UTC offset of the local time zone at `secs`, as chrono's `Local` finds it, without
/// linking CoreFoundation.
#[cfg(target_os = "macos")]
pub fn local_offset(secs: i64) -> i32 {
    if DateTime::from_timestamp(secs, 0).is_none() {
        return 0;
    }
    super::zone::local_offset(secs)
}

/// Remembers local UTC offsets by hour. An hour whose first and last second have the same
/// offset has that offset throughout, since no zone changes its offset twice within an hour;
/// an hour with a transition is looked up every time.
pub struct Clock {
    slots: Vec<Slot>,

    /// The second and format of the last text, which is `text`: files written together share a
    /// second, and neighbors in a listing often are.
    last: Option<(i64, Format)>,
    text: Vec<u8>,
}

impl Default for Clock {
    fn default() -> Self {
        Self {
            slots: vec![
                Slot {
                    window: 0,
                    offset: None,
                    used: false
                };
                SLOTS
            ],
            last: None,
            text: Vec::new(),
        }
    }
}

impl Clock {
    /// The offset at `secs`, using `lookup` to find it.
    fn offset(&mut self, secs: i64, lookup: impl Fn(i64) -> i32) -> i32 {
        let window = secs.div_euclid(WINDOW);
        let slot = &mut self.slots[usize::try_from(window & SLOT_MASK).unwrap_or_default()];

        if !slot.used || slot.window != window {
            let start = window * WINDOW;
            let (first, last) = (lookup(start), lookup(start + WINDOW - 1));
            *slot = Slot {
                window,
                offset: (first == last).then_some(first),
                used: true,
            };
        }

        slot.offset.unwrap_or_else(|| lookup(secs))
    }

    /// Appends `time` in `format` as chrono shows it in the local time zone, unless `format` or
    /// `time` is beyond the fast route, in which case it appends nothing and returns `false`.
    pub fn write(&mut self, out: &mut Vec<u8>, time: SystemTime, format: Format) -> bool {
        self.write_with(out, time, format, local_offset)
    }

    /// [`Self::write`] with the offsets found by `lookup`.
    fn write_with(
        &mut self,
        out: &mut Vec<u8>,
        time: SystemTime,
        format: Format,
        lookup: impl Fn(i64) -> i32,
    ) -> bool {
        let Ok(since_epoch) = time.duration_since(UNIX_EPOCH) else {
            return false;
        };
        let Ok(secs) = i64::try_from(since_epoch.as_secs()) else {
            return false;
        };
        if secs >= FAST_LIMIT {
            return false;
        }

        if self.last == Some((secs, format)) {
            out.extend_from_slice(&self.text);
            return true;
        }

        let offset = self.offset(secs, lookup);
        let local = DateTime::from_timestamp(secs + i64::from(offset), 0).map(|t| t.naive_utc());
        self.text.clear();
        let done = local.is_some_and(|local| write_fields(&mut self.text, &local, offset, format));
        self.last = done.then_some((secs, format));

        if done {
            out.extend_from_slice(&self.text);
        }
        done
    }
}

/// Appends the fields of `local`, which is `offset` seconds east of UTC, as
/// [`Format::pattern`] gives them (`%Z` of `iso-strict` shows as `+HH:MM`); returns `false`,
/// writing nothing, for an offset with seconds.
fn write_fields(out: &mut Vec<u8>, local: &NaiveDateTime, offset: i32, format: Format) -> bool {
    const MONTHS: [&[u8; 3]; 12] = [
        b"Jan", b"Feb", b"Mar", b"Apr", b"May", b"Jun", b"Jul", b"Aug", b"Sep", b"Oct", b"Nov",
        b"Dec",
    ];

    // chrono prints an offset with seconds as `+HH:MM:SS`.
    if matches!(format, Format::IsoStrict) && offset % 60 != 0 {
        return false;
    }

    let date = |out: &mut Vec<u8>| {
        let year = local.year().unsigned_abs();
        push_pair(out, year / 100);
        push_pair(out, year % 100);
        out.push(b'-');
        push_pair(out, local.month());
        out.push(b'-');
        push_pair(out, local.day());
    };

    match format {
        Format::Default => {
            push_pair(out, local.day());
            out.push(b' ');
            out.extend_from_slice(MONTHS[local.month0() as usize]);
            out.push(b' ');
            push_pair(out, local.hour());
            out.push(b':');
            push_pair(out, local.minute());
            out.push(b' ');
            push_pair(out, local.iso_week().year().unsigned_abs() % 100);
        },
        Format::Iso => {
            date(out);
            out.push(b' ');
            push_pair(out, local.hour());
            out.push(b':');
            push_pair(out, local.minute());
            out.push(b':');
            push_pair(out, local.second());
        },
        Format::Short => date(out),
        Format::IsoStrict => {
            date(out);
            out.push(b'T');
            push_pair(out, local.hour());
            out.push(b':');
            push_pair(out, local.minute());
            out.push(b':');
            push_pair(out, local.second());
            out.push(if offset < 0 { b'-' } else { b'+' });
            let minutes = offset.unsigned_abs() / 60;
            push_pair(out, minutes / 60);
            out.push(b':');
            push_pair(out, minutes % 60);
        },
    }

    true
}

#[cfg(test)]
mod test {
    use super::*;
    use chrono::{FixedOffset, NaiveDate, NaiveTime};
    use std::time::Duration;

    const FORMATS: [Format; 4] = [
        Format::Default,
        Format::Iso,
        Format::Short,
        Format::IsoStrict,
    ];

    /// A zone with two changes a year, at half past midnight UTC, so that an hour contains each.
    fn zone(secs: i64) -> i32 {
        let day = (secs - 1800).div_euclid(86_400) % 365;
        if (80..300).contains(&day) {
            7200
        } else {
            -18_000
        }
    }

    #[test]
    fn fields_match_chrono() {
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut clock = Clock::default();

        // Also the edges of years, where the ISO week year differs from the year.
        let mut secs: Vec<i64> = [0, 1, 86_399, 86_400, 1_735_689_600, 4_102_444_799]
            .into_iter()
            .collect();
        for year in 1971..2100 {
            let jan1 = NaiveDate::from_ymd_opt(year, 1, 1)
                .unwrap()
                .and_time(NaiveTime::MIN)
                .and_utc()
                .timestamp();
            secs.extend([jan1 - 90_000, jan1 - 1, jan1, jan1 + 90_000]);
        }
        secs.extend((0..100_000).map(|_| i64::try_from(next() % 4_102_444_800).unwrap()));

        for s in secs {
            let time = UNIX_EPOCH + Duration::from_secs(s.unsigned_abs());
            let offset = zone(s);
            let expected = DateTime::from_timestamp(s, 0)
                .unwrap()
                .with_timezone(&FixedOffset::east_opt(offset).unwrap());

            for format in FORMATS {
                let mut out = vec![];
                assert!(clock.write_with(&mut out, time, format, zone));
                assert_eq!(
                    String::from_utf8(out).unwrap(),
                    expected.format(format.pattern()).to_string(),
                    "{s} {format:?}"
                );
            }
        }
    }

    /// Offsets of any whole number of minutes match chrono; an offset with seconds (a zone's
    /// local mean time) is left to chrono.
    #[test]
    fn iso_strict_offsets_match_chrono() {
        let mut clock = Clock::default();

        for (n, offset) in [
            0, 3600, -3600, 19_800, -12_600, 20_700, -43_200, 50_400, 1, -17_762,
        ]
        .into_iter()
        .enumerate()
        {
            let secs = 1_700_000_000 + i64::try_from(n).unwrap() * 86_400;
            let time = UNIX_EPOCH + Duration::from_secs(secs.unsigned_abs());
            let expected = DateTime::from_timestamp(secs, 0)
                .unwrap()
                .with_timezone(&FixedOffset::east_opt(offset).unwrap())
                .format(Format::IsoStrict.pattern())
                .to_string();
            let mut out = vec![];
            let done = clock.write_with(&mut out, time, Format::IsoStrict, |_| offset);

            if offset % 60 == 0 {
                assert!(done);
                assert_eq!(String::from_utf8(out).unwrap(), expected);
            } else {
                assert!(!done);
                assert_eq!(out, Vec::<u8>::new());
            }
        }
    }

    #[test]
    fn out_of_range_takes_the_slow_route() {
        let mut clock = Clock::default();
        let mut out = vec![];

        assert!(!clock.write_with(
            &mut out,
            UNIX_EPOCH - Duration::from_secs(5),
            Format::Default,
            zone
        ));
        let far = UNIX_EPOCH + Duration::from_secs(FAST_LIMIT.unsigned_abs());
        assert!(!clock.write_with(&mut out, far, Format::Iso, zone));
        assert_eq!(out, Vec::<u8>::new());
    }
}

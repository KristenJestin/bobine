//! What a user watched, as the stats page counts it: plays of movies and series, each with when
//! it happened on the local clock, when the server knows.

use super::{ImageRef, ItemId};

/// Where a watch history was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistorySource {
    /// Each play the server's Playback Reporting plugin recorded: when, and for how long.
    PlaybackReporting,
    /// The titles marked played, with their play count and their last play only: how long is
    /// estimated from their length.
    PlayedItems,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WatchHistory {
    pub source: HistorySource,
    pub plays: Vec<Play>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WatchedKind {
    Movie,
    Series,
}

/// A movie, or the series an episode belongs to.
#[derive(Debug, Clone, PartialEq)]
pub struct Watched {
    /// None for a title the library no longer holds: it is known by its name only.
    pub id: Option<ItemId>,
    pub name: String,
    pub kind: WatchedKind,
    pub poster: Option<ImageRef>,
    pub genres: Vec<String>,
}

/// `count` plays of a title, `seconds` long in all.
#[derive(Debug, Clone, PartialEq)]
pub struct Play {
    pub title: Watched,
    /// When, on the local clock; None when the source does not say.
    pub at: Option<LocalTime>,
    pub seconds: u64,
    pub count: u32,
}

/// A moment on the local clock, to the minute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LocalTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
}

impl LocalTime {
    pub fn now() -> Self {
        // SAFETY: `time` with a null pointer only returns the time.
        let now = unsafe { libc::time(std::ptr::null_mut()) };
        Self::from_unix(now)
    }

    /// The local time of a moment counted in seconds from 1970 UTC.
    pub fn from_unix(seconds: i64) -> Self {
        let time = seconds as libc::time_t;
        // SAFETY: `localtime_r` writes into the `tm` it is given and nothing else.
        let tm = unsafe {
            let mut tm: libc::tm = std::mem::zeroed();
            libc::localtime_r(&time, &mut tm);
            tm
        };
        Self {
            year: tm.tm_year + 1900,
            month: tm.tm_mon as u32 + 1,
            day: tm.tm_mday as u32,
            hour: tm.tm_hour as u32,
            minute: tm.tm_min as u32,
        }
    }

    /// `2026-10-12 20:31…` or `2026-10-12T20:31…`, read as it is written, whatever follows the
    /// minutes.
    pub fn parse(text: &str) -> Option<Self> {
        let number = |range: std::ops::Range<usize>| text.get(range)?.parse::<u32>().ok();
        let separators = text.as_bytes();
        if separators.len() < 16
            || separators[4] != b'-'
            || separators[7] != b'-'
            || !matches!(separators[10], b' ' | b'T')
            || separators[13] != b':'
        {
            return None;
        }
        let time = Self {
            year: number(0..4)? as i32,
            month: number(5..7)?,
            day: number(8..10)?,
            hour: number(11..13)?,
            minute: number(14..16)?,
        };
        let valid = (1..=12).contains(&time.month)
            && (1..=31).contains(&time.day)
            && time.hour < 24
            && time.minute < 60;
        valid.then_some(time)
    }

    /// A UTC time as the server writes it (`2026-10-12T20:31:00.0000000Z`), on the local clock.
    pub fn from_utc(text: &str) -> Option<Self> {
        Self::from_clock(text, 0)
    }

    /// A time read on a clock `offset` seconds ahead of UTC, on the local clock.
    pub fn from_clock(text: &str, offset: i64) -> Option<Self> {
        let read = Self::parse(text)?;
        let seconds = read.days() * 86_400 + i64::from(read.hour * 3600 + read.minute * 60);
        Some(Self::from_unix(seconds - offset))
    }

    /// Days from 1970-01-01 to this date (Howard Hinnant's `days_from_civil`).
    pub fn days(self) -> i64 {
        let year = i64::from(self.year) - i64::from(self.month <= 2);
        let era = year.div_euclid(400);
        let of_era = year - era * 400;
        let month = i64::from(self.month);
        let of_year =
            (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(self.day) - 1;
        let of_era_days = of_era * 365 + of_era / 4 - of_era / 100 + of_year;
        era * 146_097 + of_era_days - 719_468
    }

    /// The day of the week, Monday 0 to Sunday 6.
    pub fn weekday(self) -> usize {
        // 1970-01-01 was a Thursday.
        (self.days() + 3).rem_euclid(7) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(year: i32, month: u32, day: u32) -> LocalTime {
        LocalTime {
            year,
            month,
            day,
            hour: 0,
            minute: 0,
        }
    }

    #[test]
    fn a_date_counts_its_days_from_1970() {
        assert_eq!(at(1970, 1, 1).days(), 0);
        assert_eq!(at(2000, 3, 1).days(), 11_017);
        assert_eq!(at(1969, 12, 31).days(), -1);
    }

    #[test]
    fn the_week_starts_on_monday() {
        assert_eq!(at(2026, 10, 12).weekday(), 0, "a Monday");
        assert_eq!(at(2026, 10, 17).weekday(), 5, "a Saturday");
        assert_eq!(at(2026, 10, 18).weekday(), 6, "a Sunday");
    }

    #[test]
    fn a_time_is_read_with_a_space_or_a_t() {
        let expected = LocalTime {
            year: 2026,
            month: 10,
            day: 12,
            hour: 20,
            minute: 31,
        };
        assert_eq!(
            LocalTime::parse("2026-10-12 20:31:07.1234567"),
            Some(expected)
        );
        assert_eq!(LocalTime::parse("2026-10-12T20:31:00Z"), Some(expected));
        assert_eq!(LocalTime::parse("2026-10-12"), None);
        assert_eq!(LocalTime::parse("2026-13-12 20:31"), None);
    }
}

//! The times under the seek bar, in French, after the validated design: `1 h 12 min`,
//! `− 59 min`, `fin à 23 h 41`.

/// A length of time: hours and minutes past the hour, minutes alone under an hour, seconds
/// under a minute.
pub fn duration(seconds: f64) -> String {
    // `max` also turns NaN into 0.
    let seconds = seconds.max(0.) as u64;
    match (seconds / 3600, seconds % 3600 / 60) {
        (0, 0) => format!("{seconds} s"),
        (0, minutes) => format!("{minutes} min"),
        (hours, minutes) => format!("{hours} h {minutes:02} min"),
    }
}

/// What is left, after a minus sign.
pub fn remaining(seconds: f64) -> String {
    format!("− {}", duration(seconds))
}

/// The clock time the film ends, from the local time now (hours, minutes) and what is left.
pub fn end_clock(now: (u32, u32), remaining_seconds: f64) -> String {
    let minutes = now.0 * 60 + now.1 + (remaining_seconds.max(0.) / 60.) as u32;
    let minutes = minutes % (24 * 60);
    format!("fin à {} h {:02}", minutes / 60, minutes % 60)
}

/// The local wall clock, hours and minutes.
pub fn local_now() -> (u32, u32) {
    // SAFETY: `time` with a null pointer only returns the time; `localtime_r` writes into the
    // `tm` it is given and nothing else.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        (tm.tm_hour as u32, tm.tm_min as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_time_reads_in_hours_and_minutes() {
        assert_eq!(duration(4320.), "1 h 12 min");
        assert_eq!(duration(7200.), "2 h 00 min");
    }

    #[test]
    fn under_an_hour_reads_in_minutes() {
        assert_eq!(duration(3599.), "59 min");
        assert_eq!(duration(60.), "1 min");
    }

    #[test]
    fn under_a_minute_reads_in_seconds() {
        assert_eq!(duration(42.7), "42 s");
        assert_eq!(duration(0.), "0 s");
    }

    #[test]
    fn a_negative_or_unknown_time_reads_as_zero() {
        assert_eq!(duration(-3.), "0 s");
        assert_eq!(duration(f64::NAN), "0 s");
    }

    #[test]
    fn the_remaining_time_has_a_minus_sign() {
        assert_eq!(remaining(3540.), "− 59 min");
    }

    #[test]
    fn the_end_clock_adds_what_is_left_to_now() {
        assert_eq!(end_clock((22, 42), 3540.), "fin à 23 h 41");
    }

    #[test]
    fn the_end_clock_goes_past_midnight() {
        assert_eq!(end_clock((23, 30), 3600.), "fin à 0 h 30");
    }
}

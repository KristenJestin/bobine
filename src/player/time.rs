//! The times under the seek bar: the elapsed time and what is left as clock times (`0:01:05`,
//! `−1:40:12`), then the clock time the film ends (`fin à 23 h 41`).

/// A length of time as a clock reads it, `h:mm:ss`, the hours always shown: the elapsed time
/// and the time left keep one shape from the first second to the last.
pub fn clock(seconds: f64) -> String {
    // `max` also turns NaN into 0.
    let seconds = seconds.max(0.) as u64;
    format!(
        "{}:{:02}:{:02}",
        seconds / 3600,
        seconds % 3600 / 60,
        seconds % 60
    )
}

/// What is left, after a minus sign.
pub fn remaining(seconds: f64) -> String {
    format!("−{}", clock(seconds))
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
    fn the_elapsed_time_is_a_clock_time() {
        assert_eq!(clock(65.), "0:01:05");
        assert_eq!(clock(4324.), "1:12:04");
        assert_eq!(clock(36000.), "10:00:00");
    }

    #[test]
    fn under_a_minute_the_clock_still_shows_hours_and_minutes() {
        assert_eq!(clock(42.7), "0:00:42");
        assert_eq!(clock(0.), "0:00:00");
    }

    #[test]
    fn a_negative_or_unknown_time_reads_as_zero() {
        assert_eq!(clock(-3.), "0:00:00");
        assert_eq!(clock(f64::NAN), "0:00:00");
    }

    #[test]
    fn the_remaining_time_is_a_clock_time_after_a_minus_sign() {
        assert_eq!(remaining(6012.), "−1:40:12");
        assert_eq!(remaining(59.), "−0:00:59");
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

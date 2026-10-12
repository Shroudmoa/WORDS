//! Wall-clock time: the machine's UTC offset, civil date arithmetic, and the
//! strings a clock shows.
//!
//! Kept apart from the terminal work on purpose: the calendar maths is a pure
//! function of a `i64`, so it can be tested against known dates without ever
//! reading a real clock.

use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

/// A local wall-clock reading, to the second.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Wall {
    pub year: i32,
    /// 1..=12
    pub month: u32,
    /// 1..=31
    pub day: u32,
    pub hour: u32,
    pub min: u32,
    pub sec: u32,
    /// 0 = Monday .. 6 = Sunday.
    pub weekday: usize,
}

impl Wall {
    /// Break local seconds-since-epoch into calendar fields.
    pub fn from_local(secs: i64) -> Wall {
        let days = secs.div_euclid(86_400);
        let rem = secs.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        Wall {
            year,
            month,
            day,
            hour: (rem / 3600) as u32,
            min: ((rem / 60) % 60) as u32,
            sec: (rem % 60) as u32,
            // 1970-01-01 was a Thursday, which is index 3 with Monday at zero.
            weekday: (days + 3).rem_euclid(7) as usize,
        }
    }

    /// `HH:MM:SS`, the digits the big font draws.
    pub fn hms(&self) -> String {
        format!("{:02}:{:02}:{:02}", self.hour, self.min, self.sec)
    }

    /// `MON 2026-10-12`, the caption under the digits.
    pub fn date(&self) -> String {
        format!(
            "{} {:04}-{:02}-{:02}",
            WEEKDAYS[self.weekday], self.year, self.month, self.day
        )
    }
}

const WEEKDAYS: [&str; 7] = ["MON", "TUE", "WED", "THU", "FRI", "SAT", "SUN"];

/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 to a calendar
/// date. Correct for negative days too, so the maths does not fall over if the
/// clock is ever asked for a pre-epoch instant.
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    ((y + i64::from(m <= 2)) as i32, m as u32, d as u32)
}

/// Seconds since the Unix epoch, UTC.
pub fn utc_now() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    }
}

/// The machine's current UTC offset in seconds, read from `date +%z`.
///
/// A subprocess, but one that runs on the way into the clock and then only
/// rarely: the offset does not move except at a DST boundary. If `date` is
/// missing we assume UTC, which is wrong twice a year and never otherwise.
pub fn local_offset() -> i64 {
    let out = Command::new("date")
        .arg("+%z")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    match out {
        Ok(o) => parse_offset(String::from_utf8_lossy(&o.stdout).trim()).unwrap_or(0),
        Err(_) => 0,
    }
}

/// `+0530` / `-0800` -> seconds east of UTC.
fn parse_offset(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 5 || !matches!(b[0], b'+' | b'-') {
        return None;
    }
    let h: i64 = std::str::from_utf8(&b[1..3]).ok()?.parse().ok()?;
    let m: i64 = std::str::from_utf8(&b[3..5]).ok()?.parse().ok()?;
    let secs = h * 3600 + m * 60;
    Some(if b[0] == b'-' { -secs } else { secs })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_epoch_is_a_thursday() {
        let w = Wall::from_local(0);
        assert_eq!((w.year, w.month, w.day), (1970, 1, 1));
        assert_eq!((w.hour, w.min, w.sec), (0, 0, 0));
        assert_eq!(w.weekday, 3, "1970-01-01 was a Thursday");
    }

    #[test]
    fn known_instants_land_on_the_right_day() {
        // 2001-09-09T01:46:40Z, a Sunday.
        let w = Wall::from_local(1_000_000_000);
        assert_eq!((w.year, w.month, w.day), (2001, 9, 9));
        assert_eq!((w.hour, w.min, w.sec), (1, 46, 40));
        assert_eq!(w.weekday, 6);

        // 2023-11-14T22:13:20Z, a Tuesday.
        let w = Wall::from_local(1_700_000_000);
        assert_eq!((w.year, w.month, w.day), (2023, 11, 14));
        assert_eq!((w.hour, w.min, w.sec), (22, 13, 20));
        assert_eq!(w.weekday, 1, "Monday is zero, so Tuesday is one");
    }

    #[test]
    fn a_leap_day_survives() {
        // 2024-02-29T12:00:00Z.
        let w = Wall::from_local(1_709_208_000);
        assert_eq!((w.year, w.month, w.day), (2024, 2, 29));
        assert_eq!(w.hour, 12);
    }

    #[test]
    fn the_display_strings_read_as_expected() {
        let w = Wall::from_local(0);
        assert_eq!(w.hms(), "00:00:00");
        assert_eq!(w.date(), "THU 1970-01-01");
        let w = Wall::from_local(1_700_000_000);
        assert_eq!(w.hms(), "22:13:20");
        assert_eq!(w.date(), "TUE 2023-11-14");
    }

    #[test]
    fn offsets_parse_both_ways() {
        assert_eq!(parse_offset("+0000"), Some(0));
        assert_eq!(parse_offset("+0200"), Some(7200));
        assert_eq!(parse_offset("-0800"), Some(-8 * 3600));
        assert_eq!(parse_offset("+0530"), Some(5 * 3600 + 1800));
        assert_eq!(parse_offset("UTC"), None);
        assert_eq!(parse_offset("+02"), None);
    }

    #[test]
    fn a_negative_time_still_makes_a_date() {
        // A second before the epoch: 1969-12-31, a Wednesday.
        let w = Wall::from_local(-1);
        assert_eq!((w.year, w.month, w.day), (1969, 12, 31));
        assert_eq!((w.hour, w.min, w.sec), (23, 59, 59));
        assert_eq!(w.weekday, 2);
    }
}

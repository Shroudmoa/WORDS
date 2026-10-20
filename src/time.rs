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

/// Days since 1970-01-01 for a civil date -- the inverse of
/// `civil_from_days`, and Howard Hinnant's algorithm again for the same
/// reasons (correct either side of the epoch, no drifting tables).
pub fn days_from_civil(y: i32, month: u32, day: u32) -> i64 {
    let y = y as i64 - i64::from(month <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = (month as i64 + 9).rem_euclid(12); // [0, 11], March at zero
    let doy = (153 * mp + 2) / 5 + day as i64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

pub fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Days in a month, or 0 for a month that does not exist.
pub fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(y) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Parse the stamp a deadline is written as: `2026-12-31 23:59`. The time may
/// be left off (midnight), and the leading zeroes may too. A date that does
/// not exist -- the thirtieth of February, a month thirteen -- is refused
/// here rather than quietly rolling over into the next one.
pub fn parse_when(s: &str) -> Option<(i32, u32, u32, u32, u32)> {
    let mut parts = s.split_whitespace();
    let date = parts.next()?;
    let time = parts.next().unwrap_or("00:00");
    if parts.next().is_some() {
        return None;
    }

    let mut d = date.split('-');
    let y: i32 = d.next()?.trim().parse().ok()?;
    let m: u32 = d.next()?.trim().parse().ok()?;
    let day: u32 = d.next()?.trim().parse().ok()?;
    if d.next().is_some() || !(1..=9999).contains(&y) || day < 1 || day > days_in_month(y, m) {
        return None;
    }

    let mut t = time.split(':');
    let h: u32 = t.next()?.trim().parse().ok()?;
    let min: u32 = t.next()?.trim().parse().ok()?;
    if t.next().is_some() || h > 23 || min > 59 {
        return None;
    }
    Some((y, m, day, h, min))
}

/// How much of the calendar year has run, and how long the rest of it is:
/// `(percent gone, seconds to the turn of the year)`. `local` is seconds
/// since the epoch in the machine's own time.
pub fn year_stats(local: i64) -> (f64, i64) {
    let w = Wall::from_local(local);
    let start = days_from_civil(w.year, 1, 1) * 86_400;
    let end = days_from_civil(w.year + 1, 1, 1) * 86_400;
    let pct = (local - start) as f64 * 100.0 / (end - start) as f64;
    (pct, end - local)
}

/// Tomorrow's date, as `(year, month, day)` -- what the add-a-deadline form
/// starts pre-filled with.
pub fn tomorrow(w: &Wall) -> (i32, u32, u32) {
    civil_from_days(days_from_civil(w.year, w.month, w.day) + 1)
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

    #[test]
    fn days_from_civil_is_the_inverse_of_civil_from_days() {
        for days in [-100_000, -1, 0, 1, 719_468, 20_000, 30_000] {
            let c = civil_from_days(days);
            assert_eq!(days_from_civil(c.0, c.1, c.2), days, "{c:?}");
        }
        // The known dates, the other way around.
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2001, 9, 9), 1_000_000_000 / 86_400);
        assert_eq!(days_from_civil(2024, 2, 29), 1_709_208_000 / 86_400);
    }

    #[test]
    fn february_knows_its_leap_years() {
        assert!(is_leap(2024) && is_leap(2000));
        assert!(!is_leap(2023) && !is_leap(1900));
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2023, 2), 28);
        assert_eq!(days_in_month(2026, 10), 31);
        assert_eq!(days_in_month(2026, 4), 30);
        assert_eq!(days_in_month(2026, 13), 0, "no such month");
    }

    #[test]
    fn when_stamps_parse_and_refuse_the_impossible() {
        assert_eq!(parse_when("2026-12-31 23:59"), Some((2026, 12, 31, 23, 59)));
        assert_eq!(parse_when("2027-1-1 0:05"), Some((2027, 1, 1, 0, 5)));
        assert_eq!(parse_when("2028-02-29"), Some((2028, 2, 29, 0, 0)));
        for bad in [
            "2026-02-30",  // February has no thirtieth
            "2026-13-01",  // no month thirteen
            "2026-00-10",  // nor month zero
            "2027-02-29",  // and 2027 is not a leap year
            "2026-12-31 24:00",
            "2026-12-31 12:60",
            "2026-12",     // a year and a month is not a date
            "tomorrow",
            "",
        ] {
            assert_eq!(parse_when(bad), None, "{bad} should not parse");
        }
    }

    #[test]
    fn the_year_counts_up_and_the_countdown_down_together() {
        // The epoch: one second into 1970, which was not a leap year.
        let (pct, left) = year_stats(1);
        assert!(pct < 0.001, "{pct}");
        assert_eq!(left, 365 * 86_400 - 1);

        // Midday on the fourth of July: 184.5 of 365 days in.
        let mid = days_from_civil(1970, 7, 4) * 86_400 + 12 * 3600;
        let (pct, left) = year_stats(mid);
        assert!((pct - 184.5 * 100.0 / 365.0).abs() < 1e-9, "{pct}");
        assert_eq!(left, 365 * 86_400 - mid);

        // A leap year is a day longer, and the countdown says so exactly.
        let nye = days_from_civil(2024, 12, 31) * 86_400 + 86_399;
        let (pct, left) = year_stats(nye);
        assert!(pct > 99.9, "{pct}");
        assert_eq!(left, 1, "one second of 2024 left");
        let y2024 = year_stats(days_from_civil(2024, 1, 1) * 86_400 + 60).0;
        assert!(y2024 >= 0.0 && y2024 < 0.01);
    }

    #[test]
    fn tomorrow_is_the_day_after_this_one() {
        assert_eq!(tomorrow(&Wall::from_local(days_from_civil(2026, 10, 20) * 86_400)), (2026, 10, 21));
        assert_eq!(tomorrow(&Wall::from_local(days_from_civil(2026, 12, 31) * 86_400)), (2027, 1, 1));
        assert_eq!(tomorrow(&Wall::from_local(days_from_civil(2024, 2, 28) * 86_400)), (2024, 2, 29));
    }
}

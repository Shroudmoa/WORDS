//! Saved deadlines: a name and a moment, remembered between runs.
//!
//! The file sits beside the phrase files in the config directory but is not a
//! `.txt`, so it never turns up on the phrase menu. One deadline per line,
//! written the way it was typed:
//!
//! ```text
//! NEW YEAR | 2027-01-01 00:00
//! RELEASE DAY | 2026-11-04 09:30
//! ```
//!
//! The stamp is *local* time, which is what a person means when they name a
//! day -- at the cost that the moment is pinned against the offset current
//! when the file was read, so a deadline landing on a DST change can end up
//! an hour out. An hour out is a better failure mode than a date the user did
//! not type.
//!
//! Any line that does not read as one deadline is skipped rather than being a
//! reason to refuse the rest, so a half-written save or a stray note in the
//! file costs at most the line it was written on.

use std::io;
use std::path::Path;

use crate::time;

/// One deadline: what it is called, the moment as typed, and the moment as a
/// clock counts it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Deadline {
    pub name: String,
    pub y: i32,
    pub m: u32,
    pub d: u32,
    pub h: u32,
    pub min: u32,
    /// The instant itself, UTC epoch seconds. The civil fields above are what
    /// the user typed; this is what the digits count against.
    pub at: i64,
}

impl Deadline {
    /// A deadline for a moment already checked by [`time::parse_when`].
    /// `offset` is the machine's seconds east of UTC at this reading.
    pub fn at(
        name: impl Into<String>,
        y: i32,
        m: u32,
        d: u32,
        h: u32,
        min: u32,
        offset: i64,
    ) -> Deadline {
        let local = time::days_from_civil(y, m, d) * 86_400 + (h * 3600 + min * 60) as i64;
        Deadline {
            name: name.into(),
            y,
            m,
            d,
            h,
            min,
            at: local - offset,
        }
    }

    /// `2027-01-01 00:00` -- the moment, written back the way it reads.
    pub fn stamp(&self) -> String {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            self.y, self.m, self.d, self.h, self.min
        )
    }

    /// One line of the file.
    pub fn line(&self) -> String {
        format!("{} | {}", self.name, self.stamp())
    }
}

/// Parse one line of the file. A deadline already in the past still parses:
/// an overdue deadline is information, not a syntax error.
pub fn parse_line(line: &str, offset: i64) -> Option<Deadline> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (name, stamp) = line.split_once('|')?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let (y, m, d, h, min) = time::parse_when(stamp)?;
    Some(Deadline::at(name, y, m, d, h, min, offset))
}

/// Every deadline in the file, soonest first. A file that is not there is an
/// empty list rather than an error: the first run has to start somewhere.
pub fn load(path: &Path, offset: i64) -> Vec<Deadline> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut out: Vec<Deadline> = text.lines().filter_map(|l| parse_line(l, offset)).collect();
    out.sort_by_key(|d| d.at);
    out
}

/// Write the list out. A `|` in a name would break the format, so it is
/// folded into a slash on the way out rather than being quietly dropped.
pub fn save(path: &Path, ds: &[Deadline]) -> io::Result<()> {
    let mut text = String::from("# name | local date and time\n");
    for d in ds {
        let mut safe = d.clone();
        safe.name = safe.name.replace('|', "/");
        text.push_str(&safe.line());
        text.push('\n');
    }
    std::fs::write(path, text)
}

/// A remaining time the way the menu says it: `72d 11:22` once there is a
/// day in it, `11:22:33` while there is not.
pub fn short(secs: i64) -> String {
    let s = secs.max(0);
    let (d, h, m) = (s / 86_400, (s / 3600) % 24, (s / 60) % 60);
    if d > 0 {
        format!("{d}d {h:02}:{m:02}")
    } else {
        format!("{h:02}:{m:02}:{:02}", s % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A scratch file that cleans up after itself.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let p = std::env::temp_dir().join(format!("words-deadline-test-{name}"));
            let _ = std::fs::remove_file(&p);
            Scratch(p)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn a_deadline_round_trips_through_its_line() {
        let d = Deadline::at("NEW YEAR", 2027, 1, 1, 0, 0, 0);
        assert_eq!(d.line(), "NEW YEAR | 2027-01-01 00:00");
        assert_eq!(parse_line(&d.line(), 0), Some(d));
    }

    #[test]
    fn the_offset_moves_the_instant_not_the_stamp() {
        // The same wall-clock moment, read two hours east of UTC: the instant
        // is two hours earlier in UTC terms.
        let utc = Deadline::at("X", 2026, 6, 1, 12, 0, 0);
        let east = Deadline::at("X", 2026, 6, 1, 12, 0, 7200);
        assert_eq!(east.at, utc.at - 7200);
        assert_eq!(east.stamp(), utc.stamp(), "the stamp is what was typed");
    }

    #[test]
    fn lines_that_are_not_deadlines_are_skipped() {
        for bad in [
            "",
            "# a note to self",
            "NO STAMP",
            "| 2027-01-01 00:00",
            "NAME | feb thirty",
            "NAME | 2026-02-30 10:00",
        ] {
            assert_eq!(parse_line(bad, 0), None, "{bad:?} should not parse");
        }
    }

    #[test]
    fn a_saved_list_comes_back_soonest_first() {
        let s = Scratch::new("roundtrip");
        let ds = [
            Deadline::at("LATER", 2028, 3, 1, 9, 0, 0),
            Deadline::at("SOONER", 2027, 1, 1, 0, 0, 0),
        ];
        save(&s.0, &ds).expect("writes");
        let back = load(&s.0, 0);
        let names: Vec<&str> = back.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["SOONER", "LATER"], "sorted by soonest first");
    }

    #[test]
    fn a_file_with_a_bad_line_keeps_the_good_ones() {
        let s = Scratch::new("halfbad");
        std::fs::write(
            &s.0,
            "GOOD ONE | 2027-05-05 08:00\nGARBAGE LINE\nALSO GOOD | 2027-06-06 09:00\n",
        )
        .unwrap();
        let back = load(&s.0, 0);
        let names: Vec<&str> = back.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["GOOD ONE", "ALSO GOOD"]);
    }

    #[test]
    fn a_file_that_is_not_there_is_an_empty_list() {
        assert!(load(Path::new("/nonexistent/deadlines"), 0).is_empty());
    }

    #[test]
    fn a_pipe_in_a_name_cannot_break_the_format() {
        let s = Scratch::new("pipe");
        let d = Deadline::at("A|B", 2027, 1, 1, 0, 0, 0);
        save(&s.0, std::slice::from_ref(&d)).expect("writes");
        let back = load(&s.0, 0);
        assert_eq!(back.len(), 1, "the line still parses: {back:?}");
        assert_eq!(back[0].name, "A/B", "the pipe was folded, not dropped");
    }

    #[test]
    fn remaining_times_read_the_way_the_menu_says_them() {
        assert_eq!(short(72 * 3600 + 11 * 60 + 22), "3d 00:11");
        assert_eq!(short(2 * 86_400 + 3 * 3600 + 4 * 60), "2d 03:04");
        assert_eq!(short(11 * 3600 + 22 * 60 + 33), "11:22:33");
        assert_eq!(short(59), "00:00:59");
        assert_eq!(short(-5), "00:00:00", "a deadline gone by reads as none left");
    }
}

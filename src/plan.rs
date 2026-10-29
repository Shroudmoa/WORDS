//! A little day planner: named blocks of time, remembered between runs.
//!
//! The file sits beside the phrase files in the config directory but is not a
//! `.txt`, so it never turns up on the phrase menu. One event per line, with a
//! start and an end:
//!
//! ```text
//! DESIGN REVIEW | 2026-10-29 09:00 10:30
//! LUNCH         | 2026-10-29 12:30 13:15
//! ```
//!
//! Events are single-day blocks: an end at or before the start is refused
//! rather than being read as an overnight shift. Any line that does not read as
//! one event is skipped rather than being a reason to refuse the rest.

use std::io;
use std::path::Path;

use crate::time;

/// One block of time on one day.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub name: String,
    pub y: i32,
    pub m: u32,
    pub d: u32,
    /// Minutes since midnight. The planner draws from these, so a block is
    /// just two numbers on a line from 00:00 to 24:00.
    pub start: u32,
    pub end: u32,
}

impl Event {
    pub fn new(
        name: impl Into<String>,
        y: i32,
        m: u32,
        d: u32,
        start: u32,
        end: u32,
    ) -> Event {
        Event {
            name: name.into(),
            y,
            m,
            d,
            start,
            end,
        }
    }

    /// Days since the epoch, which is what the planner compares days by.
    pub fn day(&self) -> i64 {
        time::days_from_civil(self.y, self.m, self.d)
    }

    /// `YYYY-MM-DD`, the way the file writes the day.
    pub fn date(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.y, self.m, self.d)
    }

    /// `HH:MM`, from minutes since midnight.
    pub fn hhmm(min: u32) -> String {
        format!("{:02}:{:02}", (min / 60).min(23), min % 60)
    }

    /// `09:00-10:30`, the block as one string.
    pub fn span(&self) -> String {
        format!("{}-{}", Self::hhmm(self.start), Self::hhmm(self.end))
    }

    /// One line of the file.
    pub fn line(&self) -> String {
        format!(
            "{} | {} {} {}",
            self.name,
            self.date(),
            Self::hhmm(self.start),
            Self::hhmm(self.end)
        )
    }
}

/// Parse one line of the file. The name may contain a `|`; only the last one
/// separates it from the stamp.
pub fn parse_line(line: &str) -> Option<Event> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (name, rest) = line.rsplit_once('|')?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }

    let mut parts = rest.split_whitespace();
    let date = parts.next()?;
    let start = parts.next()?;
    let end = parts.next()?;
    if parts.next().is_some() {
        return None;
    }

    let (y, m, d, sh, sm) = time::parse_when(&format!("{date} {start}"))?;
    let (eh, em) = time::parse_clock(end)?;
    let start = sh * 60 + sm;
    let end = eh * 60 + em;
    if end <= start {
        return None;
    }
    Some(Event::new(name, y, m, d, start, end))
}

/// Every event in the file, soonest first, and within a day by start time. A
/// file that is not there is an empty list rather than an error.
pub fn load(path: &Path) -> Vec<Event> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut out: Vec<Event> = text.lines().filter_map(parse_line).collect();
    sort(&mut out);
    out
}

/// Keep the list in the order the planner draws it: day, then start, then name
/// so two blocks at the same minute have a stable order.
pub fn sort(events: &mut [Event]) {
    events.sort_by(|a, b| {
        (a.day(), a.start, &a.name).cmp(&(b.day(), b.start, &b.name))
    });
}

/// Write the list out. A newline in a name would break the line, so it becomes
/// a space. A `|` is safe: the parser separates on the last one.
pub fn save(path: &Path, events: &[Event]) -> io::Result<()> {
    let mut text = String::from("# name | local date start end\n");
    for e in events {
        let mut safe = e.clone();
        safe.name = safe.name.replace(['\n', '\r'], " ");
        text.push_str(&safe.line());
        text.push('\n');
    }
    std::fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let p = std::env::temp_dir().join(format!("words-plan-test-{name}"));
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
    fn an_event_round_trips_through_its_line() {
        let e = Event::new("DESIGN REVIEW", 2026, 10, 29, 9 * 60, 10 * 60 + 30);
        assert_eq!(e.line(), "DESIGN REVIEW | 2026-10-29 09:00 10:30");
        assert_eq!(parse_line(&e.line()), Some(e));
    }

    #[test]
    fn a_name_may_contain_the_separator() {
        let e = Event::new("A|B", 2026, 10, 29, 60, 120);
        assert_eq!(parse_line(&e.line()).unwrap().name, "A|B");
    }

    #[test]
    fn lines_that_are_not_events_are_skipped() {
        for bad in [
            "",
            "# a note",
            "NO STAMP",
            "| 2026-10-29 09:00 10:00",
            "NAME | 2026-02-30 09:00 10:00",
            "NAME | 2026-10-29 10:00 09:00", // end before start
            "NAME | 2026-10-29 09:00 09:00", // no time at all
            "NAME | 2026-10-29 09:00",
            "NAME | 2026-10-29 09:00 10:00 extra",
        ] {
            assert_eq!(parse_line(bad), None, "{bad:?} should not parse");
        }
    }

    #[test]
    fn a_saved_list_comes_back_in_planner_order() {
        let s = Scratch::new("roundtrip");
        let events = [
            Event::new("LATER", 2026, 10, 30, 9 * 60, 10 * 60),
            Event::new("AFTERNOON", 2026, 10, 29, 14 * 60, 15 * 60),
            Event::new("MORNING", 2026, 10, 29, 9 * 60, 10 * 60),
        ];
        save(&s.0, &events).expect("writes");
        let back = load(&s.0);
        let names: Vec<&str> = back.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["MORNING", "AFTERNOON", "LATER"]);
    }

    #[test]
    fn a_file_with_a_bad_line_keeps_the_good_ones() {
        let s = Scratch::new("halfbad");
        std::fs::write(
            &s.0,
            "GOOD | 2026-10-29 09:00 10:00\nGARBAGE\nALSO GOOD | 2026-10-29 11:00 12:00\n",
        )
        .unwrap();
        let back = load(&s.0);
        assert_eq!(back.len(), 2, "{back:?}");
    }

    #[test]
    fn a_file_that_is_not_there_is_an_empty_list() {
        assert!(load(Path::new("/nonexistent/plan")).is_empty());
    }

    #[test]
    fn a_pipe_or_newline_cannot_break_the_format() {
        let s = Scratch::new("pipe");
        let e = Event::new("A|B\nC", 2026, 10, 29, 60, 120);
        save(&s.0, std::slice::from_ref(&e)).expect("writes");
        let back = load(&s.0);
        assert_eq!(back.len(), 1, "the line still parses: {back:?}");
        assert_eq!(back[0].name, "A|B C", "the pipe survives, the newline does not");
    }

    #[test]
    fn spans_read_as_start_and_end_times() {
        let e = Event::new("X", 2026, 1, 1, 9 * 60 + 5, 13 * 60);
        assert_eq!(e.span(), "09:05-13:00");
    }
}

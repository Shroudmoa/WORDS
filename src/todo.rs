//! A little todo list, remembered between runs.
//!
//! The file sits beside the phrase files in the config directory but is not a
//! `.txt`, so it never turns up on the phrase menu. One task per line, written
//! the way it reads:
//!
//! ```text
//! [ ] BUY COFFEE
//! [x] SHIP IT
//! ```
//!
//! Plain checkboxes keep it hand-editable: a text editor is a perfectly good
//! second interface to a list this small. Any line that is not a task is
//! skipped rather than being a reason to refuse the rest, so a stray note or a
//! half-written save costs at most the line it was written on.

use std::io;
use std::path::Path;

/// One task: what it says, and whether it has been done.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Todo {
    pub text: String,
    pub done: bool,
}

impl Todo {
    pub fn new(text: impl Into<String>) -> Todo {
        Todo {
            text: text.into(),
            done: false,
        }
    }

    /// One line of the file: `[ ]` open, `[x]` done.
    pub fn line(&self) -> String {
        format!("[{}] {}", if self.done { 'x' } else { ' ' }, self.text)
    }
}

/// Parse one line of the file. `[X]` is accepted as well as `[x]`, and a plain
/// `- ` bullet is read as an open task so a hand-written list still loads.
pub fn parse_line(line: &str) -> Option<Todo> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (done, rest) = if let Some(r) = line
        .strip_prefix("[x]")
        .or_else(|| line.strip_prefix("[X]"))
    {
        (true, r)
    } else if let Some(r) = line.strip_prefix("[ ]") {
        (false, r)
    } else if let Some(r) = line.strip_prefix('-') {
        (false, r)
    } else {
        return None;
    };
    let text = rest.trim();
    if text.is_empty() {
        return None;
    }
    Some(Todo {
        text: text.to_string(),
        done,
    })
}

/// Every task in the file, in the order it was written. A file that is not
/// there is an empty list rather than an error: the first run has to start
/// somewhere.
pub fn load(path: &Path) -> Vec<Todo> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines().filter_map(parse_line).collect()
}

/// Write the list out. A newline in a task would break the format, so it is
/// folded into a space on the way out rather than being quietly dropped.
pub fn save(path: &Path, todos: &[Todo]) -> io::Result<()> {
    let mut text = String::from("# [ ] open   [x] done\n");
    for t in todos {
        let mut safe = t.clone();
        safe.text = safe.text.replace(['\n', '\r'], " ");
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
            let p = std::env::temp_dir().join(format!("words-todo-test-{name}"));
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
    fn a_task_round_trips_through_its_line() {
        let open = Todo::new("BUY COFFEE");
        assert_eq!(open.line(), "[ ] BUY COFFEE");
        assert_eq!(parse_line(&open.line()), Some(open));

        let done = Todo {
            text: "SHIP IT".into(),
            done: true,
        };
        assert_eq!(done.line(), "[x] SHIP IT");
        assert_eq!(parse_line(&done.line()), Some(done));
    }

    #[test]
    fn the_checkbox_is_read_the_way_people_write_it() {
        assert_eq!(parse_line("[X] UPPER").unwrap().done, true);
        assert_eq!(parse_line("[ ] LOWER").unwrap().done, false);
        assert_eq!(parse_line("- BULLET").unwrap().text, "BULLET");
        assert_eq!(parse_line("- BULLET").unwrap().done, false);
    }

    #[test]
    fn lines_that_are_not_tasks_are_skipped() {
        for bad in ["", "# a note", "JUST A LINE", "[] EMPTY BOX", "[ ]", "[x]"] {
            assert_eq!(parse_line(bad), None, "{bad:?} should not parse");
        }
    }

    #[test]
    fn a_saved_list_comes_back_in_the_order_it_was_written() {
        let s = Scratch::new("roundtrip");
        let todos = [
            Todo::new("FIRST"),
            Todo {
                text: "SECOND".into(),
                done: true,
            },
            Todo::new("THIRD"),
        ];
        save(&s.0, &todos).expect("writes");
        assert_eq!(load(&s.0), todos);
    }

    #[test]
    fn a_file_with_a_bad_line_keeps_the_good_ones() {
        let s = Scratch::new("halfbad");
        std::fs::write(&s.0, "[ ] GOOD\nGARBAGE\n[x] ALSO GOOD\n").unwrap();
        let back = load(&s.0);
        let text: Vec<&str> = back.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(text, ["GOOD", "ALSO GOOD"]);
    }

    #[test]
    fn a_file_that_is_not_there_is_an_empty_list() {
        assert!(load(Path::new("/nonexistent/todos")).is_empty());
    }

    #[test]
    fn a_newline_in_a_task_cannot_break_the_format() {
        let s = Scratch::new("newline");
        let t = Todo::new("A\nB");
        save(&s.0, std::slice::from_ref(&t)).expect("writes");
        let back = load(&s.0);
        assert_eq!(back.len(), 1, "the line still parses: {back:?}");
        assert_eq!(back[0].text, "A B", "the newline was folded, not dropped");
    }
}

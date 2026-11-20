//! Reader Mode tests: layout, Vim-style motions, search, selection and the
//! position file. Everything runs on in-memory documents, so no terminal is
//! needed.

use super::document::{Document, Kind};
use super::history;
use super::{Outcome, Pos, Reader, Ui};
use std::path::PathBuf;

fn reader(text: &str) -> Reader {
    reader_width(text, 60, 24)
}

fn reader_width(text: &str, cols: usize, rows: usize) -> Reader {
    let mut r = Reader::new(cols, rows);
    let doc = Document::from_text(PathBuf::from("test.txt"), Kind::Txt, text);
    r.chars = doc.lines.iter().map(|l| l.text.chars().collect()).collect();
    r.doc = Some(doc);
    r.layout = None;
    r.rebuild_layout();
    r
}

fn tap(r: &mut Reader, keys: &str) -> Outcome {
    let mut last = Outcome::None;
    for b in keys.bytes() {
        last = r.key(b);
    }
    last
}

fn row_text(r: &Reader, row: usize) -> String {
    let l = r.layout.as_ref().unwrap();
    let d = r.document().unwrap();
    let rr = l.rows[row];
    d.lines[rr.line].text[rr.start..rr.end].to_string()
}

#[test]
fn navigation_starts_in_normal_mode_at_the_top() {
    let r = reader("first\nsecond\nthird");
    assert_eq!(r.ui, Ui::Normal);
    assert_eq!(r.cursor, Pos { line: 0, col: 0 });
    assert_eq!(r.top, 0);
}

#[test]
fn basic_motions_move_in_both_directions() {
    let mut r = reader("hello world\nsecond line");
    tap(&mut r, "l");
    assert_eq!(r.cursor.col, 1);
    tap(&mut r, "j");
    assert_eq!(r.cursor.line, 1);
    tap(&mut r, "k");
    assert_eq!(r.cursor.line, 0);
    tap(&mut r, "h");
    assert_eq!(r.cursor.col, 0);
    tap(&mut r, "$");
    assert_eq!(r.cursor.col, 11);
    tap(&mut r, "0");
    assert_eq!(r.cursor.col, 0);
}

#[test]
fn numeric_prefixes_apply_to_the_next_motion() {
    let mut r = reader("0123456789\nabcdefghij\nabcdefghij");
    tap(&mut r, "10l");
    assert_eq!(r.cursor.col, 10);
    tap(&mut r, "2j");
    assert_eq!(r.cursor.line, 2);
    tap(&mut r, "3h");
    assert_eq!(r.cursor.col, 7);
    // A prefix that names a line is consumed by G.
    tap(&mut r, "2G");
    assert_eq!(r.cursor.line, 1);
}

#[test]
fn word_motions_skip_spaces_and_cross_lines() {
    let mut r = reader("one two\nthree  four");
    tap(&mut r, "w");
    assert_eq!(r.cursor, Pos { line: 0, col: 4 });
    tap(&mut r, "w");
    assert_eq!(r.cursor, Pos { line: 1, col: 0 });
    tap(&mut r, "e");
    assert_eq!(r.cursor, Pos { line: 1, col: 4 });
    tap(&mut r, "w");
    assert_eq!(r.cursor, Pos { line: 1, col: 7 });
    tap(&mut r, "b");
    assert_eq!(r.cursor, Pos { line: 1, col: 0 });
    tap(&mut r, "b");
    assert_eq!(r.cursor, Pos { line: 0, col: 4 });
    tap(&mut r, "b");
    assert_eq!(r.cursor, Pos { line: 0, col: 0 });
}

#[test]
fn gg_and_G_reach_the_ends_of_the_document() {
    let mut r = reader("a\nb\nc\nd");
    tap(&mut r, "G");
    assert_eq!(r.cursor, Pos { line: 3, col: 0 });
    tap(&mut r, "gg");
    assert_eq!(r.cursor, Pos { line: 0, col: 0 });
    // An unrelated key after a single g does not strand the sequence.
    tap(&mut r, "gj");
    assert_eq!(r.cursor.line, 1);
}

#[test]
fn a_colon_number_jumps_to_a_line_and_bad_commands_do_nothing() {
    let mut r = reader("a\nb\nc\nd\ne");
    tap(&mut r, ":3\n");
    assert_eq!(r.cursor.line, 2);
    tap(&mut r, ":999\n");
    assert_eq!(r.cursor.line, 4, "out-of-range jumps clamp");
    tap(&mut r, ":nope\n");
    assert_eq!(r.cursor.line, 4);
    assert_eq!(r.ui, Ui::Normal);
    tap(&mut r, ":2\x1b");
    assert_eq!(r.cursor.line, 4, "escaped command changes nothing");
}

#[test]
fn screen_motions_and_centering_follow_the_viewport() {
    let body = (0..60).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
    let mut r = reader(&body);
    assert_eq!(r.page, 21);
    tap(&mut r, "\x04"); // ctrl-d
    assert_eq!(r.cursor.line, 10);
    assert_eq!(r.top, 0, "the cursor is still on screen");
    tap(&mut r, "\x15"); // ctrl-u
    assert_eq!(r.cursor.line, 0);
    tap(&mut r, "\x06"); // ctrl-f
    assert_eq!(r.cursor.line, 21);
    tap(&mut r, "\x02"); // ctrl-b
    assert_eq!(r.cursor.line, 0);
    tap(&mut r, "H");
    assert_eq!(r.cursor.line, r.top);
    tap(&mut r, "L");
    assert_eq!(r.cursor.line, r.top + r.page - 1);
    tap(&mut r, "M");
    assert_eq!(r.cursor.line, r.top + r.page / 2);
    tap(&mut r, "zz");
    assert_eq!(r.cursor.line, r.top + r.page / 2);
}

#[test]
fn wrapped_rows_move_down_one_row_at_a_time() {
    let mut r = reader_width("one two three four five six seven", 12, 12);
    assert!(r.layout.as_ref().unwrap().rows.len() > 1);
    let first = row_text(&r, 0);
    assert!(first.len() < 32);
    tap(&mut r, "j");
    assert_eq!(r.cursor.line, 0, "j moves within one wrapped logical line");
    assert_eq!(r.cursor.col, r.layout.as_ref().unwrap().rows[1].start);
}

#[test]
fn search_finds_matches_and_n_N_steps_through_them() {
    let mut r = reader("alpha beta\nalpha gamma\nbeta alpha");
    tap(&mut r, "/alpha\n");
    assert_eq!(r.cursor.line, 0);
    assert_eq!(r.highlights(0), vec![(0, 5)]);
    tap(&mut r, "n");
    assert_eq!(r.cursor.line, 1);
    tap(&mut r, "n");
    assert_eq!(r.cursor.line, 2);
    tap(&mut r, "n");
    assert_eq!(r.cursor.line, 0, "search wraps");
    tap(&mut r, "N");
    assert_eq!(r.cursor.line, 2);
    tap(&mut r, "?beta\n");
    assert_eq!(r.cursor, Pos { line: 2, col: 0 });
    tap(&mut r, "N");
    assert_eq!(r.cursor, Pos { line: 0, col: 6 });
}

#[test]
fn search_is_case_insensitive_for_unicode_and_can_be_cancelled() {
    let mut r = reader("Straße\nSTRASSE\nÄhre\nähre");
    tap(&mut r, "/ähre\n");
    assert_eq!(r.cursor.line, 2);
    tap(&mut r, "/straße\n");
    assert_eq!(r.cursor.line, 0);
    tap(&mut r, "/xyz");
    tap(&mut r, "\x1b");
    assert_eq!(r.ui, Ui::Normal);
    assert_eq!(r.cursor.line, 0);
}

#[test]
fn visual_selection_copies_the_chosen_range() {
    let mut r = reader("hello world\nsecond");
    tap(&mut r, "v");
    assert_eq!(r.ui, Ui::Visual);
    tap(&mut r, "llll");
    tap(&mut r, "j");
    // While still in visual mode the selection is the text between anchor and
    // cursor; check it before copying it away.
    assert_eq!(r.selected_text(), "hello world\nseco");
    tap(&mut r, "y");
    assert_eq!(r.ui, Ui::Normal);
    assert_eq!(r.selected_text(), "");
}

#[test]
fn yy_copies_the_current_line_without_moving() {
    let mut r = reader("first\nsecond\nthird");
    tap(&mut r, "j");
    let before = r.cursor;
    tap(&mut r, "yy");
    assert_eq!(r.cursor, before);
    tap(&mut r, "v");
    tap(&mut r, "$");
    assert_eq!(r.selected_text(), "second");
}

#[test]
fn an_empty_document_is_safe_to_navigate() {
    let mut r = reader("");
    assert!(r.document().unwrap().is_empty());
    for key in "jklwbe0$GgGnzy/?:HML".bytes() {
        let _ = r.key(key);
    }
    assert_eq!(r.cursor, Pos { line: 0, col: 0 });
    assert_eq!(r.top, 0);
}

#[test]
fn resizing_recomputes_wrapping_and_keeps_the_cursor_visible() {
    let body = (0..20).map(|i| format!("paragraph number {i} with a tail")).collect::<Vec<_>>().join("\n");
    let mut r = reader(&body);
    let wide_rows = r.layout.as_ref().unwrap().rows.len();
    r.set_size(20, 10);
    assert!(r.layout.as_ref().unwrap().rows.len() > wide_rows);
    tap(&mut r, "G");
    assert!(r.top + r.page > r.row());
    r.set_size(120, 40);
    assert_eq!(r.layout.as_ref().unwrap().width, 100, "comfortable max width");
    assert!(r.top + r.page > r.row());
}

#[test]
fn leaving_persists_the_position_and_reopening_restores_it() {
    let file = std::env::temp_dir().join(format!("words-reader-pos-{}.txt", std::process::id()));
    let path = PathBuf::from("/somewhere/book.txt");
    let _ = std::fs::remove_file(&file);
    assert_eq!(history::load_at(file.clone(), &path), None);

    history::save_at(file.clone(), &path, Pos { line: 12, col: 4 });
    history::save_at(
        file.clone(),
        &PathBuf::from("/other.txt"),
        Pos { line: 2, col: 1 },
    );
    assert_eq!(
        history::load_at(file.clone(), &path),
        Some(Pos { line: 12, col: 4 })
    );
    history::save_at(file.clone(), &path, Pos { line: 7, col: 0 });
    assert_eq!(
        history::load_at(file.clone(), &path),
        Some(Pos { line: 7, col: 0 })
    );
    let text = std::fs::read_to_string(&file).unwrap();
    assert_eq!(text.lines().count(), 2, "one entry per document");
    let _ = std::fs::remove_file(file);
}

#[test]
fn quitting_from_a_prompt_is_not_confused_with_search_text() {
    let mut r = reader("a\nb");
    tap(&mut r, "/q");
    let out = tap(&mut r, "\x03");
    assert_eq!(out, Outcome::None, "ctrl-c in a prompt only cancels typing");
    tap(&mut r, "\x1b");
    assert_eq!(r.ui, Ui::Normal);
    let out = r.key(b'q');
    assert_eq!(out, Outcome::Leave);
}

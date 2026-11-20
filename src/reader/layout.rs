//! Text layout: wrapped display rows over a document's logical lines.
//!
//! Wrapping is computed once per width and kept in `rows`; drawing only ever
//! walks the visible slice. A long paragraph therefore costs memory linear in
//! its text, but no more terminal work than a short one.

use super::document::Document;

/// One visual row of text. `start`/`end` are char offsets into the logical
/// line's rendered text, so cursor, search and selection all share one
/// coordinate system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayRow {
    pub line: usize,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug)]
pub struct Layout {
    pub width: usize,
    pub rows: Vec<DisplayRow>,
    /// First display row of each logical line; `rows.len()` for the last one.
    pub line_start: Vec<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Word,
    Space,
}

struct Token {
    start: usize,
    end: usize,
    width: usize,
    kind: Kind,
}

impl Layout {
    pub fn new(doc: &Document, width: usize) -> Layout {
        let width = width.max(1);
        let mut rows = Vec::new();
        let mut line_start = Vec::with_capacity(doc.lines.len());

        for (li, line) in doc.lines.iter().enumerate() {
            line_start.push(rows.len());
            let chars: Vec<char> = line.text.chars().collect();
            if chars.is_empty() {
                rows.push(DisplayRow {
                    line: li,
                    start: 0,
                    end: 0,
                });
                continue;
            }
            let tokens = tokenize(&chars);
            let mut from = 0usize;
            let mut used = 0usize;
            let mut ti = 0usize;
            let mut emitted_end = None;

            while ti < tokens.len() {
                let t = &tokens[ti];
                // A word wider than the column is cut at character boundaries;
                // long URLs and unbroken CJK runs are not allowed to overflow.
                if t.width > width {
                    if used > 0 {
                        let end = trim_end(&chars, from, t.start);
                        rows.push(DisplayRow {
                            line: li,
                            start: from,
                            end,
                        });
                        emitted_end = Some(end);
                    }
                    let mut pos = t.start;
                    while pos < t.end {
                        let mut end = pos;
                        let mut w = 0usize;
                        while end < t.end {
                            let cw = char_width(chars[end]);
                            if w + cw > width && end > pos {
                                break;
                            }
                            w += cw;
                            end += 1;
                        }
                        rows.push(DisplayRow {
                            line: li,
                            start: pos,
                            end,
                        });
                        emitted_end = Some(end);
                        pos = end;
                    }
                    from = t.end;
                    used = 0;
                    ti += 1;
                    continue;
                }

                if used > 0 && used + t.width > width {
                    let end = trim_end(&chars, from, t.start);
                    rows.push(DisplayRow {
                        line: li,
                        start: from,
                        end,
                    });
                    emitted_end = Some(end);
                    // The space that broke the row is leading whitespace now;
                    // it would only waste a column on the continuation.
                    if t.kind == Kind::Space {
                        from = t.end;
                        used = 0;
                        ti += 1;
                        continue;
                    }
                    from = t.start;
                    used = 0;
                }
                used += t.width;
                ti += 1;
            }

            let last_end = trim_end(&chars, from, chars.len());
            if emitted_end != Some(last_end) {
                rows.push(DisplayRow {
                    line: li,
                    start: from,
                    end: last_end,
                });
            }
        }
        Layout {
            width,
            rows,
            line_start,
        }
    }

    /// The display row containing a char offset on a logical line.
    pub fn row_for(&self, line: usize, col: usize) -> usize {
        let a = self.line_start.get(line).copied().unwrap_or(self.rows.len());
        let b = self
            .line_start
            .get(line + 1)
            .copied()
            .unwrap_or(self.rows.len());
        for i in a..b {
            if col < self.rows[i].end {
                return i;
            }
        }
        b.saturating_sub(1).max(a)
    }

    /// The visual cell offset of a char position within its display row.
    pub fn goal_for(&self, doc: &Document, line: usize, col: usize) -> usize {
        if self.rows.is_empty() {
            return 0;
        }
        let row = self.row_for(line, col);
        let r = self.rows[row];
        let text: Vec<char> = doc.lines[line].text.chars().collect();
        text[r.start..col.min(r.end).max(r.start)]
            .iter()
            .map(|c| char_width(*c))
            .sum()
    }

    /// Inverse of `goal_for`: pick the char position on `row` nearest `goal`.
    pub fn col_for_goal(&self, doc: &Document, row: usize, goal: usize) -> usize {
        let Some(r) = self.rows.get(row) else {
            return 0;
        };
        let text: Vec<char> = doc.lines[r.line].text.chars().collect();
        let mut chosen = r.start;
        let mut w = 0usize;
        for (i, c) in text.iter().enumerate().take(r.end).skip(r.start) {
            if w + char_width(*c) > goal {
                break;
            }
            w += char_width(*c);
            chosen = i + 1;
        }
        chosen.min(r.end).max(r.start)
    }

    pub fn row_end(&self, row: usize) -> usize {
        self.rows.get(row).map_or(0, |r| r.end)
    }

    pub fn row_start(&self, row: usize) -> usize {
        self.rows.get(row).map_or(0, |r| r.start)
    }
}

fn trim_end(chars: &[char], from: usize, end: usize) -> usize {
    let mut e = end;
    while e > from && chars[e - 1].is_whitespace() {
        e -= 1;
    }
    e
}

fn tokenize(chars: &[char]) -> Vec<Token> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let space = chars[i].is_whitespace();
        let start = i;
        let mut width = 0usize;
        while i < chars.len() && chars[i].is_whitespace() == space {
            width += char_width(chars[i]);
            i += 1;
        }
        out.push(Token {
            start,
            end: i,
            width,
            kind: if space { Kind::Space } else { Kind::Word },
        });
    }
    out
}

/// A practical cell width for a character. Combining marks take no cell;
/// East Asian and emoji glyphs take two; everything else takes one.
pub fn char_width(c: char) -> usize {
    let u = c as u32;
    if c == '\t' {
        4
    } else if c.is_control() {
        0
    } else if matches!(
        u,
        0x0300..=0x036f
            | 0x0483..=0x0489
            | 0x0591..=0x05bd
            | 0x0610..=0x061a
            | 0x1ab0..=0x1aff
            | 0x1dc0..=0x1dff
            | 0x20d0..=0x20ff
            | 0xfe00..=0xfe0f
            | 0xfe20..=0xfe2f
    ) {
        0
    } else if matches!(
        u,
        0x1100..=0x115f
            | 0x2329..=0x232a
            | 0x2e80..=0xa4cf
            | 0xac00..=0xd7a3
            | 0xf900..=0xfaff
            | 0xfe10..=0xfe19
            | 0xfe30..=0xfe6f
            | 0xff00..=0xff60
            | 0xffe0..=0xffe6
            | 0x1f300..=0x1faff
            | 0x20000..=0x3fffd
    ) {
        2
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::document::Kind;

    fn doc(lines: &[&str]) -> Document {
        Document::from_text(
            "test.txt".into(),
            Kind::Txt,
            &lines.join("\n"),
        )
    }

    fn texts(l: &Layout, d: &Document) -> Vec<String> {
        l.rows
            .iter()
            .map(|r| {
                let chars: Vec<char> = d.lines[r.line].text.chars().collect();
                chars[r.start..r.end].iter().collect()
            })
            .collect()
    }

    #[test]
    fn a_short_line_is_one_row() {
        let d = doc(&["hello world"]);
        let l = Layout::new(&d, 20);
        assert_eq!(texts(&l, &d), ["hello world"]);
        assert_eq!(l.line_start, [0]);
    }

    #[test]
    fn wrapping_breaks_on_words_and_tracks_logical_lines() {
        let d = doc(&["one two three four", "", "after"]);
        let l = Layout::new(&d, 9);
        assert_eq!(texts(&l, &d), ["one two", "three", "four", "", "after"]);
        assert_eq!(l.line_start, [0, 3, 4]);
        assert_eq!(l.row_for(1, 0), 3);
        assert_eq!(l.row_for(2, 0), 4);
    }

    #[test]
    fn a_word_longer_than_the_column_is_cut() {
        let d = doc(&["abcdefghijkl"]);
        let l = Layout::new(&d, 5);
        assert_eq!(texts(&l, &d), ["abcde", "fghij", "kl"]);
    }

    #[test]
    fn leading_indent_is_kept_and_wide_characters_count_double() {
        let d = doc(&["  hi 世界"]);
        let l = Layout::new(&d, 8);
        assert_eq!(texts(&l, &d), ["  hi", "世界"]);
        assert_eq!(l.goal_for(&d, 0, 3), 3);
    }

    #[test]
    fn goal_round_trips_through_a_row() {
        let d = Document::from_text("x".into(), Kind::Txt, "abc def\nghi");
        let l = Layout::new(&d, 7);
        let goal = l.goal_for(&d, 0, 3);
        let row = l.row_for(0, 3);
        assert_eq!(l.col_for_goal(&d, row, goal), 3);
    }

    #[test]
    fn an_empty_document_has_no_rows() {
        let d = doc(&[]);
        assert!(Layout::new(&d, 40).rows.is_empty());
    }

    #[test]
    fn blank_lines_survive_as_rows() {
        let d = Document::from_text("x".into(), Kind::Txt, "a\n\n\nb");
        let l = Layout::new(&d, 20);
        assert_eq!(l.rows.len(), 4);
        assert_eq!(l.rows[1].start, l.rows[1].end);
        assert_eq!(l.rows[2].start, l.rows[2].end);
    }

    #[test]
    fn an_empty_line_still_has_a_row() {
        let d = Document::from_text("x".into(), Kind::Txt, "");
        let l = Layout::new(&d, 20);
        assert!(l.rows.is_empty());
        let d = Document::from_text("x".into(), Kind::Txt, "\n");
        let l = Layout::new(&d, 20);
        assert_eq!(l.rows.len(), 1, "one blank line before the final newline");
    }
}

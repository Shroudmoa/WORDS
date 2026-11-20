//! Drawing Reader Mode. Everything is painted in one frame-sized string, as
//! in the rest of WORDS: a full wipe, a header, the visible document rows, a
//! prompt line and a footer. The palette and text primitives come from the
//! existing modules, so the reader inherits the same terminal aesthetic.

use super::document::Style as TextStyle;
use super::layout::char_width;
use super::Reader;
use crate::style::{self, Rgb, AMBER, BG, CYAN, DIM, FG, GREEN, MAGENTA};
use crate::term::Out;
use crate::view::write_text;

/// Fixed opacity keeps the reader quiet: the document is the bright thing,
/// chrome sits at a whisper.
const CHROME: f64 = 0.65;

pub fn draw(r: &Reader, o: &mut Out, cols: usize, rows: usize) {
    if cols < 12 || rows < 6 {
        return;
    }
    // Every frame owns the whole screen. Wiping first means a resize, a
    // shorter document or a closed help panel can never leave old glyphs.
    for y in 0..rows {
        style::cells(o, 0, y, cols, BG);
    }

    let dim = style::mix(BG, DIM, CHROME);
    let cyan = style::mix(BG, CYAN, CHROME);

    // -- header --------------------------------------------------------------
    let title = r.document().map(|d| d.title.as_str()).unwrap_or("NO DOCUMENT");
    let title = truncate(title, cols.saturating_sub(26).max(8));
    write_text(o, cols, 2, 0, &format!(" READER  /  {title} "), FG);
    let kind = r
        .document()
        .map(|d| d.kind.label())
        .unwrap_or("----");
    let voice_tag = if r.voice.is_playing() {
        "  \u{25b6} VOICE"
    } else if r.voice.is_paused() {
        "  \u{2016} PAUSED"
    } else {
        ""
    };
    let tag = format!("{kind}  {}{voice_tag} ", r.ui.label());
    write_text(o, cols, cols.saturating_sub(tag.chars().count() + 2), 0, &tag, cyan);
    style::cells(o, 1, 1, cols.saturating_sub(2), dim);

    // -- document ------------------------------------------------------------
    let content_h = rows.saturating_sub(3);
    if let (Some(doc), Some(layout)) = (r.document(), r.layout()) {
        let x0 = (cols.saturating_sub(layout.width)) / 2;
        let gutter = x0.saturating_sub(2);
        let cursor_row = layout.row_for(r.cursor.line, r.cursor.col);
        for vr in 0..content_h {
            let ri = r.top + vr;
            let Some(row) = layout.rows.get(ri) else {
                break;
            };
            if ri == cursor_row && gutter < x0 {
                write_text(o, cols, gutter, vr + 2, "\u{2502}", style::mix(BG, CYAN, 0.45));
            }
            draw_row(r, doc, o, cols, x0, vr + 2, row);
        }
        if doc.is_empty() {
            let msg = "EMPTY DOCUMENT";
            write_text(
                o,
                cols,
                (cols.saturating_sub(msg.len())) / 2,
                2 + content_h / 2,
                msg,
                dim,
            );
        }
    } else {
        let msg = "NO DOCUMENT — PRESS M FOR THE PICKER, OR USE :e PATH";
        write_text(
            o,
            cols,
            cols.saturating_sub(msg.len() + 2) / 2,
            2 + content_h / 2,
            msg,
            dim,
        );
    }

    // -- prompt --------------------------------------------------------------
    let prompt_row = rows - 2;
    let prompt = match &r.ui {
        super::Ui::Search { forward, query } => {
            let q = String::from_utf8_lossy(query);
            Some((if *forward { "/" } else { "?" }.to_string() + &q + "\u{2588}", FG))
        }
        super::Ui::Command(buf) => {
            let q = String::from_utf8_lossy(buf);
            Some((format!(":{q}\u{2588}"), FG))
        }
        _ => None,
    };
    if let Some((text, color)) = prompt {
        write_text(o, cols, 2, prompt_row, &text, color);
    } else if let Some(msg) = r.message() {
        write_text(o, cols, 2, prompt_row, msg, AMBER);
    } else {
        let hint = if r.voice.is_playing() || r.voice.is_paused() {
            "space pause/resume · s stop · < > sentence · [ ] rate · V toggle voice"
        } else {
            "gg first · G last · ctrl-d/u/f/b scroll · H/M/L screen · space read aloud · ctrl-h help"
        };
        write_text(o, cols, 2, prompt_row, hint, dim);
    }

    // -- footer --------------------------------------------------------------
    let lines = r.document().map_or(0, |d| d.lines.len());
    let pct = if lines <= 1 {
        if lines == 1 { 100 } else { 0 }
    } else {
        r.cursor.line * 100 / (lines - 1)
    };
    let left = format!(
        "Ln {}/{}  Col {}  {}%",
        if lines == 0 { 0 } else { r.cursor.line + 1 },
        lines,
        r.cursor.col + 1,
        pct
    );
    write_text(o, cols, 2, rows - 1, &left, style::mix(BG, FG, 0.55));
    let right = "hjkl move  wbe word  / search  v select  : line  q back";
    let rx = cols.saturating_sub(right.chars().count() + 2);
    write_text(o, cols, rx, rows - 1, right, dim);

    if r.help {
        help(o, cols, rows);
    }
}

fn draw_row(
    r: &Reader,
    doc: &super::document::Document,
    o: &mut Out,
    cols: usize,
    x0: usize,
    y: usize,
    row: &super::layout::DisplayRow,
) {
    let line = &doc.lines[row.line];
    let text: Vec<char> = line.text.chars().collect();
    let hits = r.highlights(row.line);
    let voice_span = r.spoken();
    let anchor = r.anchor.unwrap_or(r.cursor);
    let selected = if r.ui == super::Ui::Visual {
        Some((anchor.min(r.cursor), anchor.max(r.cursor)))
    } else {
        None
    };

    let word_range = r.current_word();
    let bg_for = |col: usize| -> Rgb {
        let in_hit = hits.iter().any(|(a, b)| col >= *a && col < *b);
        let in_sel = selected.is_some_and(|(a, b)| {
            (row.line > a.line || (row.line == a.line && col >= a.col))
                && (row.line < b.line || (row.line == b.line && col < b.col))
        });
        let in_word = word_range.is_some_and(|(a, b)| in_range(a, b, row.line, col));
        let in_voice = voice_span.is_some_and(|s| in_sentence(s, row.line, col));
        if in_hit {
            style::mix(BG, AMBER, 0.38)
        } else if in_sel {
            style::mix(BG, MAGENTA, 0.32)
        } else if in_word {
            style::mix(BG, GREEN, 0.55)
        } else if in_voice {
            style::mix(BG, GREEN, 0.28)
        } else if row.line == r.cursor.line && col == r.cursor.col {
            style::mix(BG, CYAN, 0.22)
        } else {
            BG
        }
    };

    let mut i = row.start;
    let mut x = x0;
    while i < row.end && x < cols {
        let style = style_at(line.spans.as_slice(), i);
        let fg = color_for(style);
        let bg = bg_for(i);

        let mut j = i + 1;
        while j < row.end {
            let next = if style_at(line.spans.as_slice(), j) == style {
                color_for(style)
            } else {
                break
            };
            if next != fg || bg_for(j) != bg || x + run_width(&text[i..j]) >= cols {
                break;
            }
            j += 1;
        }

        let run: String = text[i..j].iter().collect();
        write_run(o, cols, x, y, &run, fg, bg);
        x += run_width(&text[i..j]);
        i = j;
    }
    // A text row can be shorter than the layout width; nothing to clear here
    // because the whole screen was wiped at the start of the frame.
}

/// Whether a char position falls inside the sentence being spoken. Both ends
/// are exclusive/inclusive as stored: `start` is the first word, `end` is one
/// past the last.
/// Whether a char sits in `[a, b)` across one or more display lines.
fn in_range(a: super::Pos, b: super::Pos, line: usize, col: usize) -> bool {
    let at = (line, col);
    (a.line, a.col) <= at && at < (b.line, b.col)
}

fn in_sentence(s: &crate::voice::Sentence, line: usize, col: usize) -> bool {
    if line < s.start.0 || line > s.end.0 {
        return false;
    }
    if line == s.start.0 && col < s.start.1 {
        return false;
    }
    if line == s.end.0 && col >= s.end.1 {
        return false;
    }
    true
}

fn style_at(spans: &[super::document::Span], i: usize) -> TextStyle {
    spans
        .iter()
        .find(|s| i >= s.start && i < s.end)
        .map(|s| s.style)
        .unwrap_or(TextStyle::Plain)
}

fn color_for(s: TextStyle) -> Rgb {
    match s {
        TextStyle::Plain => FG,
        TextStyle::Heading(1) => CYAN,
        TextStyle::Heading(_) => style::mix(CYAN, FG, 0.45),
        TextStyle::Bold => FG,
        TextStyle::Italic => style::mix(FG, CYAN, 0.45),
        TextStyle::Code => GREEN,
        TextStyle::Quote => style::mix(FG, DIM, 0.55),
        TextStyle::Bullet => CYAN,
        TextStyle::Rule => style::mix(BG, DIM, 0.9),
        TextStyle::Link => MAGENTA,
    }
}

fn run_width(chars: &[char]) -> usize {
    chars.iter().map(|c| char_width(*c)).sum()
}

/// Foreground-only runs reuse the existing writer. Highlighted runs need a
/// background, so they get one cursor move and one SGR pair of their own.
fn write_run(o: &mut Out, cols: usize, x: usize, y: usize, s: &str, fg: Rgb, bg: Rgb) {
    if x >= cols {
        return;
    }
    let room = cols - x;
    let mut used = 0usize;
    let mut end = 0usize;
    for c in s.chars() {
        let w = char_width(c);
        if used + w > room {
            break;
        }
        used += w;
        end += c.len_utf8();
    }
    let s = &s[..end];
    if bg == BG {
        write_text(o, cols, x, y, s, fg);
    } else {
        o.put(&format!(
            "\x1b[{};{}H\x1b[38;2;{};{};{}m\x1b[48;2;{};{};{}m{}\x1b[0m",
            y + 1,
            x + 1,
            fg[0], fg[1], fg[2],
            bg[0], bg[1], bg[2],
            s
        ));
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

const HELP: &[(&str, &str)] = &[
    ("h j k l / arrows", "move left / down / up / right"),
    ("w · b · e", "next / previous word · word end"),
    ("0 · $", "start / end of display row"),
    ("gg · G · :N", "first line · last line · line N"),
    ("N prefixes", "10j, 5w, 20G, ..."),
    ("ctrl-d · ctrl-u", "half page down / up"),
    ("ctrl-f · ctrl-b", "full page down / up"),
    ("H · M · L · zz", "top / middle / bottom · center"),
    ("/ · ?", "search forward / backward"),
    ("n · N", "next / previous match"),
    ("v · y", "start selection · copy it"),
    ("yy", "copy the current line"),
    ("space", "read aloud / pause / resume"),
    ("s · < · >", "stop · prev / next sentence"),
    ("[ · ] · V", "slower / faster · toggle voice"),
    (":rate N", "set words per minute"),
    (":voice NAME", "switch neural voice"),
    (":preview", "hear the selected voice"),
    (":e PATH", "open another document"),
    ("q · esc", "leave reader · cancel a prompt"),
];

fn help(o: &mut Out, cols: usize, rows: usize) {
    let kw = HELP.iter().map(|(k, _)| k.chars().count()).max().unwrap_or(0);
    let vw = HELP.iter().map(|(_, v)| v.chars().count()).max().unwrap_or(0);
    let w = (kw + vw + 9).min(cols.saturating_sub(2));
    let h = (HELP.len() + 4).min(rows.saturating_sub(2));
    if w < 12 || h < 5 {
        return;
    }
    let x = (cols - w) / 2;
    let y = (rows - h) / 2;
    for i in 0..h {
        style::cells(o, x, y + i, w, BG);
    }
    let edge = style::mix(BG, CYAN, CHROME);
    write_text(
        o,
        cols,
        x,
        y,
        &format!("\u{250c}{}\u{2510}", "\u{2500}".repeat(w - 2)),
        edge,
    );
    write_text(
        o,
        cols,
        x,
        y + h - 1,
        &format!("\u{2514}{}\u{2518}", "\u{2500}".repeat(w - 2)),
        edge,
    );
    for i in 1..h - 1 {
        write_text(o, cols, x, y + i, "\u{2502}", edge);
        write_text(o, cols, x + w - 1, y + i, "\u{2502}", edge);
    }
    write_text(
        o,
        cols,
        x + 3,
        y,
        " READER KEYS ",
        style::mix(BG, FG, CHROME),
    );
    for (i, (key, desc)) in HELP.iter().enumerate().take(h.saturating_sub(4)) {
        let ry = y + 2 + i;
        write_text(o, cols, x + 3, ry, key, style::mix(BG, CYAN, 0.9));
        let dx = x + 5 + kw;
        if dx < x + w - 1 {
            write_text(o, cols, dx, ry, desc, style::mix(BG, FG, 0.7));
        }
    }
}

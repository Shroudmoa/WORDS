//! Reader Mode: a whole document, top to bottom, with Vim-style navigation.
//!
//! This module is deliberately self-contained. It owns the document, its
//! wrapped layout, the cursor, search and selection; the engine only knows
//! which mode is on screen and the frame loop only knows where to route keys.
//! Nothing here edits the document, so it can later sit behind a speech
//! engine, a bookmark list or a theme without changing shape.

pub mod document;
pub mod layout;
mod render;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use crate::config;
use crate::engine::{KEY_DOWN, KEY_LEFT, KEY_RIGHT, KEY_UP};
use crate::voice::{self, Sentence, Voice};
use document::Document;
use layout::Layout;

/// Longest numeric prefix accepted before a motion.
const MAX_COUNT: usize = 10_000;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct Pos {
    /// Zero-based logical line.
    pub line: usize,
    /// Char offset into that line's rendered text. One past the end is legal
    /// (`$` puts the cursor there) but is clamped to the line length.
    pub col: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ui {
    Normal,
    Visual,
    Search { forward: bool, query: Vec<u8> },
    Command(Vec<u8>),
}

impl Ui {
    pub fn label(&self) -> &'static str {
        match self {
            Ui::Normal => "NORMAL",
            Ui::Visual => "VISUAL",
            Ui::Search { .. } => "SEARCH",
            Ui::Command(_) => "COMMAND",
        }
    }
}

#[derive(Clone, Debug)]
enum Pending {
    None,
    Count(String),
    /// Seen `g`, waiting for the second `g`.
    G,
    /// Seen `z`, waiting for the second `z`.
    Z,
    /// Seen `y` in NORMAL, waiting for `yy`.
    Y,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    None,
    /// Leave Reader Mode and hand the keyboard back to the typewriter.
    Leave,
    /// Quit the application.
    Quit,
    /// `:e path` asked for another document.
    Open(PathBuf),
}

/// All reader state. Kept out of `Engine` on purpose: the typewriter must not
/// have to know that documents exist.
pub struct Reader {
    doc: Option<Document>,
    /// Char arrays per logical line, so word motions do not rescan a long
    /// paragraph once per character.
    chars: Vec<Vec<char>>,
    layout: Option<Layout>,
    width: usize,
    page: usize,

    pub cursor: Pos,
    /// The first display row shown in the content area.
    pub top: usize,
    pub ui: Ui,
    /// Horizontal goal in cells, kept across vertical motions so a column
    /// survives a short wrapped line.
    goal: usize,
    anchor: Option<Pos>,
    search: Option<(bool, String)>,
    pending: Pending,
    pub message: Option<(String, Instant)>,
    pub help: bool,
    /// A `:` command whose effect belongs to the frame loop.
    pending_action: Option<Outcome>,

    /// A cloneable handle to the one engine owned by the frame loop. It is a
    /// handle, not a second worker: main and Reader see the same state.
    voice: Voice,
    sentences: Vec<Sentence>,
    /// The sentence the worker last said it was speaking.
    spoken: Option<usize>,
    /// Document range of the word currently being spoken.
    word_mark: Option<(Pos, Pos)>,
    /// Where play resumes after a pause, or the sentence a jump selected.
    voice_index: usize,
}

/// Convert a word boundary (char range inside a sentence) into document
/// positions. Sentence positions are kept per char, so hard wraps and Unicode
/// do not drift the highlight.
fn word_mark(sentence: &Sentence, from: usize, to: usize) -> Option<(Pos, Pos)> {
    let a = sentence.positions.get(from).copied()?;
    let b = sentence.positions.get(to.saturating_sub(1)).copied()?;
    Some((Pos { line: a.0, col: a.1 }, Pos { line: b.0, col: b.1 + 1 }))
}

impl Reader {
    /// Construct a reader with its own engine (tests only in the binary).
    #[cfg(test)]
    pub fn new(cols: usize, rows: usize) -> Reader {
        Self::with_voice(cols, rows, Voice::new(voice::Config::load()))
    }

    /// Construct a reader that shares the frame loop's engine.
    pub fn with_voice(cols: usize, rows: usize, voice: Voice) -> Reader {
        let mut r = Reader {
            doc: None,
            chars: Vec::new(),
            layout: None,
            width: 80,
            page: 20,
            cursor: Pos::default(),
            top: 0,
            ui: Ui::Normal,
            goal: 0,
            anchor: None,
            search: None,
            pending: Pending::None,
            message: None,
            help: false,
            pending_action: None,
            voice,
            sentences: Vec::new(),
            spoken: None,
            word_mark: None,
            voice_index: 0,
        };
        r.set_size(cols, rows);
        r
    }

    /// Terminal size changed. Only the wrapping width and the page height are
    /// kept; a resize rebuilds the layout and brings the cursor back on screen.
    pub fn set_size(&mut self, cols: usize, rows: usize) {
        let width = if cols < 24 {
            cols.max(8)
        } else {
            cols.saturating_sub(4).min(100)
        };
        self.page = rows.saturating_sub(3).max(1);
        if width != self.width {
            self.width = width;
            self.rebuild_layout();
        }
        self.ensure_visible();
    }

    fn rebuild_layout(&mut self) {
        let Some(doc) = &self.doc else {
            self.layout = None;
            return;
        };
        self.layout = Some(Layout::new(doc, self.width));
        self.ensure_visible();
    }

    /// Load a document, remembering where we were in the previous one and
    /// restoring the saved position for this one.
    pub fn open(&mut self, path: &Path) -> Result<(), String> {
        self.silence();
        self.remember();
        let doc = Document::load(path)?;
        self.chars = doc
            .lines
            .iter()
            .map(|l| l.text.chars().collect())
            .collect();
        self.doc = Some(doc);
        self.layout = None;
        self.rebuild_layout();
        self.cursor = history::load(path).unwrap_or_default();
        self.rebuild_sentences();
        self.voice_index = self.sentence_at_cursor();
        self.top = 0;
        self.ui = Ui::Normal;
        self.pending = Pending::None;
        self.anchor = None;
        self.search = None;
        self.help = false;
        self.goal = 0;
        self.clamp_cursor();
        self.ensure_visible();
        self.say("OPENED");
        Ok(())
    }

    pub fn document(&self) -> Option<&Document> {
        self.doc.as_ref()
    }

    pub fn layout(&self) -> Option<&Layout> {
        self.layout.as_ref()
    }

    /// The sentence currently being spoken, for the read-along highlight.
    pub fn spoken(&self) -> Option<&Sentence> {
        self.spoken.and_then(|i| self.sentences.get(i))
    }

    /// Document range of the current word, if any.
    pub fn current_word(&self) -> Option<(Pos, Pos)> {
        self.word_mark
    }

    /// Stop playback. Called on leave, quit and document change.
    pub fn silence(&mut self) {
        self.voice.stop();
        self.spoken = None;
        self.word_mark = None;
    }

    /// Pick up an edit to the voice config made from outside the reader.
    pub fn reload_voice_config(&mut self) {
        self.voice.reload_config();
    }

    fn rebuild_sentences(&mut self) {
        self.sentences = voice::sentences(&self.chars);
    }

    /// Which sentence contains the cursor, or the nearest one before it.
    fn sentence_at_cursor(&self) -> usize {
        let contains = |s: &Sentence| {
            (self.cursor.line > s.start.0
                || (self.cursor.line == s.start.0 && self.cursor.col >= s.start.1))
                && (self.cursor.line < s.end.0
                    || (self.cursor.line == s.end.0 && self.cursor.col < s.end.1))
        };
        self.sentences
            .iter()
            .position(contains)
            .or_else(|| {
                self.sentences.iter().rposition(|s| {
                    (s.start.0, s.start.1) <= (self.cursor.line, self.cursor.col)
                })
            })
            .unwrap_or(0)
    }

    fn goto_sentence(&mut self, index: usize) {
        let Some(s) = self.sentences.get(index) else {
            return;
        };
        self.cursor = Pos {
            line: s.start.0,
            col: s.start.1,
        };
        self.goal = 0;
        self.ensure_visible();
    }

    fn start_voice_from(&mut self, index: usize) {
        if self.sentences.is_empty() {
            self.say("NOTHING TO SPEAK");
            return;
        }
        if !self.voice.wants_reader() {
            self.say("VOICE OFF — EDIT ~/.config/words/voice");
            return;
        }
        self.voice_index = index.min(self.sentences.len() - 1);
        self.voice.speak(self.sentences.clone(), self.voice_index);
        self.say(format!(
            "READING {}/{}",
            self.voice_index + 1,
            self.sentences.len()
        ));
    }

    /// Space: pause at the exact word, resume there, or start reading.
    fn toggle_voice(&mut self) {
        if self.voice.is_playing() {
            self.voice_index = self.voice.current_sentence().unwrap_or(self.voice_index);
            self.voice.pause();
            self.say("PAUSED");
        } else if self.voice.is_paused() {
            self.voice.resume();
            self.say("RESUMED");
        } else {
            let at = self.sentence_at_cursor().max(self.voice_index);
            self.start_voice_from(at);
        }
    }

    /// `<` and `>` step one sentence, carrying playback with them if active.
    fn jump_sentence(&mut self, delta: isize) {
        if self.sentences.is_empty() {
            self.say("NO SENTENCES");
            return;
        }
        let base = self
            .voice
            .current_sentence()
            .unwrap_or(self.sentence_at_cursor());
        let index = (base as isize + delta).clamp(0, self.sentences.len() as isize - 1) as usize;
        self.voice_index = index;
        self.goto_sentence(index);
        if self.voice.is_playing() || self.voice.is_paused() {
            self.voice.seek_sentence(index);
        }
        self.say(format!("SENTENCE {}/{}", index + 1, self.sentences.len()));
    }

    /// Drain speech events and follow the spoken sentence with the cursor.
    fn poll_voice(&mut self) {
        self.voice.poll();
        if let Some(err) = self.voice.take_error() {
            self.say(format!("VOICE: {err}"));
        }
        self.word_mark = self
            .voice
            .current_word()
            .and_then(|(si, from, to)| word_mark(self.sentences.get(si)?, from, to));
        match self.voice.current_sentence() {
            Some(i) if Some(i) != self.spoken => {
                self.spoken = Some(i);
                self.voice_index = i;
                self.goto_sentence(i);
            }
            None if self.spoken.is_some() => {
                self.spoken = None;
                self.word_mark = None;
            }
            _ => {}
        }
    }

    pub fn selected_text(&self) -> String {
        let Some((a, b)) = self.selection() else {
            return String::new();
        };
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        let mut out = String::new();
        for (i, cs) in self.chars.iter().enumerate() {
            if i < a.line || i > b.line {
                continue;
            }
            let from = if i == a.line { a.col.min(cs.len()) } else { 0 };
            let to = if i == b.line { b.col.min(cs.len()) } else { cs.len() };
            if from < to {
                out.extend(cs[from..to].iter());
            }
            if i < b.line {
                out.push('\n');
            }
        }
        out
    }

    fn selection(&self) -> Option<(Pos, Pos)> {
        if self.ui == Ui::Visual {
            Some((self.anchor.unwrap_or(self.cursor), self.cursor))
        } else {
            None
        }
    }

    /// Persist this document's position. Called when leaving the mode, when
    /// opening another document and on the way out of the application.
    pub fn remember(&mut self) {
        let Some(doc) = &self.doc else {
            return;
        };
        history::save(&doc.path, self.cursor);
    }

    fn say(&mut self, msg: impl Into<String>) {
        self.message = Some((msg.into(), Instant::now()));
    }

    fn expire_message(&mut self) {
        if self
            .message
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed().as_secs() >= 4)
        {
            self.message = None;
        }
    }

    pub fn message(&self) -> Option<&str> {
        let (m, at) = self.message.as_ref()?;
        (at.elapsed().as_secs() < 4).then_some(m.as_str())
    }

    /// Show an error from the frame loop (a document that failed to open).
    pub fn report_error(&mut self, msg: impl Into<String>) {
        self.say(msg);
    }

    // -- cursor helpers ------------------------------------------------------

    fn lines(&self) -> usize {
        self.chars.len()
    }

    fn line_len(&self, line: usize) -> usize {
        self.chars.get(line).map_or(0, Vec::len)
    }

    fn char_at(&self, pos: Pos) -> Option<char> {
        self.chars.get(pos.line).and_then(|l| l.get(pos.col)).copied()
    }

    fn clamp_cursor(&mut self) {
        if self.lines() == 0 {
            self.cursor = Pos::default();
            return;
        }
        self.cursor.line = self.cursor.line.min(self.lines() - 1);
        self.cursor.col = self.cursor.col.min(self.line_len(self.cursor.line));
    }

    fn row(&self) -> usize {
        self.layout
            .as_ref()
            .map_or(0, |l| l.row_for(self.cursor.line, self.cursor.col))
    }

    /// Keep the current row inside the viewport with at least a line of
    /// context where the document is long enough to give it.
    fn ensure_visible(&mut self) {
        let Some(layout) = &self.layout else {
            self.top = 0;
            return;
        };
        if layout.rows.is_empty() {
            self.top = 0;
            return;
        }
        let r = layout.row_for(self.cursor.line, self.cursor.col);
        let last = layout.rows.len().saturating_sub(1);
        self.top = self.top.min(last);
        if r < self.top {
            self.top = r;
        } else if r >= self.top + self.page {
            self.top = r + 1 - self.page;
        }
    }

    fn update_goal(&mut self) {
        if let (Some(doc), Some(layout)) = (&self.doc, &self.layout) {
            self.goal = layout.goal_for(doc, self.cursor.line, self.cursor.col);
        }
    }

    /// Move to a display row, keeping the eye's horizontal goal.
    fn goto_row(&mut self, row: usize) {
        let Some(layout) = &self.layout.clone() else {
            return;
        };
        if layout.rows.is_empty() {
            return;
        }
        let row = row.min(layout.rows.len() - 1);
        let r = layout.rows[row];
        if let Some(doc) = &self.doc {
            self.cursor = Pos {
                line: r.line,
                col: layout.col_for_goal(doc, row, self.goal),
            };
        }
        self.ensure_visible();
    }

    fn move_rows(&mut self, delta: isize) {
        let r = self.row() as isize + delta;
        self.goto_row(r.max(0) as usize);
    }

    fn scroll(&mut self, rows: isize) {
        self.move_rows(rows);
    }

    // -- word motions --------------------------------------------------------

    /// Skip whitespace, crossing line breaks, landing on a word character.
    fn skip_space_forward(&mut self) {
        loop {
            if self.cursor.line >= self.lines() {
                return;
            }
            while self.cursor.col < self.line_len(self.cursor.line)
                && self
                    .char_at(self.cursor)
                    .is_some_and(char::is_whitespace)
            {
                self.cursor.col += 1;
            }
            if self.cursor.col < self.line_len(self.cursor.line) {
                return;
            }
            if self.cursor.line + 1 >= self.lines() {
                return;
            }
            self.cursor.line += 1;
            self.cursor.col = 0;
        }
    }

    fn skip_word_forward(&mut self) {
        // A line break is a word boundary: `w` stops at the end of a line,
        // and the following space-skip is what carries it onto the next one.
        while self.cursor.line < self.lines()
            && self.cursor.col < self.line_len(self.cursor.line)
            && self
                .char_at(self.cursor)
                .is_some_and(|c| !c.is_whitespace())
        {
            self.cursor.col += 1;
        }
    }

    fn word_forward(&mut self, count: usize) {
        for _ in 0..count {
            self.skip_space_forward();
            if self.cursor.line >= self.lines() {
                break;
            }
            self.skip_word_forward();
            // Vim's `w` lands on the *next* word, not on the space that ends
            // this one.
            self.skip_space_forward();
        }
        self.update_goal();
        self.ensure_visible();
    }

    /// Step back one character, crossing a line break when needed.
    fn step_back(&mut self) -> bool {
        if self.cursor.col > 0 {
            self.cursor.col -= 1;
            return true;
        }
        if self.cursor.line == 0 {
            return false;
        }
        self.cursor.line -= 1;
        self.cursor.col = self.line_len(self.cursor.line);
        true
    }

    fn skip_space_back(&mut self) {
        loop {
            if self.cursor.col == 0 && self.cursor.line == 0 {
                return;
            }
            let before = Pos {
                line: self.cursor.line,
                col: self.cursor.col.saturating_sub(1),
            };
            let on_line = self.cursor.col > 0;
            let c = if on_line {
                self.char_at(before)
            } else {
                None
            };
            if c.is_some_and(char::is_whitespace) {
                self.cursor.col -= 1;
            } else {
                return;
            }
        }
    }

    fn skip_word_back(&mut self) {
        loop {
            if self.cursor.col == 0 && self.cursor.line == 0 {
                return;
            }
            let before = Pos {
                line: self.cursor.line,
                col: self.cursor.col.saturating_sub(1),
            };
            let on_line = self.cursor.col > 0;
            let c = if on_line {
                self.char_at(before)
            } else {
                None
            };
            if c.is_some_and(|c| !c.is_whitespace()) {
                self.cursor.col -= 1;
            } else {
                return;
            }
        }
    }

    fn word_back(&mut self, count: usize) {
        for _ in 0..count {
            if !self.step_back() {
                break;
            }
            self.skip_space_back();
            self.skip_word_back();
        }
        self.update_goal();
        self.ensure_visible();
    }

    fn word_end(&mut self, count: usize) {
        for i in 0..count {
            if i > 0 {
                // Start looking for the next word after the current one.
                if self.cursor.col < self.line_len(self.cursor.line) {
                    self.cursor.col += 1;
                } else if self.cursor.line + 1 < self.lines() {
                    self.cursor.line += 1;
                    self.cursor.col = 0;
                }
            }
            self.skip_space_forward();
            if self.cursor.line >= self.lines() {
                break;
            }
            // Walk to the last character of the word.
            loop {
                let next = Pos {
                    line: self.cursor.line,
                    col: self.cursor.col + 1,
                };
                if self.char_at(next).is_some_and(|c| !c.is_whitespace()) {
                    self.cursor.col += 1;
                } else {
                    break;
                }
            }
        }
        self.update_goal();
        self.ensure_visible();
    }

    // -- search --------------------------------------------------------------

    fn lower_map(s: &str) -> (Vec<char>, Vec<usize>) {
        let mut low = Vec::new();
        let mut orig = Vec::new();
        for (i, c) in s.chars().enumerate() {
            for lc in c.to_lowercase() {
                low.push(lc);
                orig.push(i);
            }
        }
        (low, orig)
    }

    /// All occurrences of `query` in one line, as char ranges in the original
    /// text. Case-insensitive, including for Unicode with multi-char folds.
    fn matches_in_line(line: &str, query: &str) -> Vec<(usize, usize)> {
        if query.is_empty() {
            return Vec::new();
        }
        let (low, map) = Self::lower_map(line);
        let (needle, _) = Self::lower_map(query);
        if needle.is_empty() || needle.len() > low.len() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for i in 0..=low.len() - needle.len() {
            if low[i..i + needle.len()] == needle[..] {
                let start = map[i];
                let end = map[i + needle.len() - 1] + 1;
                out.push((start, end));
            }
        }
        out
    }

    /// Char ranges to highlight on a logical line, from the last search.
    pub fn highlights(&self, line: usize) -> Vec<(usize, usize)> {
        let (Some(doc), Some((_, query))) = (&self.doc, &self.search) else {
            return Vec::new();
        };
        doc.lines
            .get(line)
            .map(|l| Self::matches_in_line(&l.text, query))
            .unwrap_or_default()
    }

    fn find_in_line(&self, line: usize, query: &str, from: usize, forward: bool) -> Option<usize> {
        let text = self.chars.get(line)?.iter().copied().collect::<String>();
        let hits = Self::matches_in_line(&text, query);
        if forward {
            hits.into_iter().map(|(s, _)| s).find(|&s| s >= from)
        } else {
            hits.into_iter()
                .map(|(s, _)| s)
                .filter(|&s| s < from)
                .next_back()
        }
    }

    /// Find the next match from `from`, wrapping at the end of the document.
    fn find_next(&self, from: Pos, query: &str, forward: bool) -> Option<Pos> {
        let n = self.lines();
        if n == 0 {
            return None;
        }
        let first: Box<dyn Iterator<Item = usize>> = if forward {
            Box::new(from.line..n)
        } else {
            Box::new((0..=from.line).rev())
        };
        for li in first {
            let col = if li == from.line {
                from.col
            } else if forward {
                0
            } else {
                self.line_len(li)
            };
            if let Some(col) = self.find_in_line(li, query, col, forward) {
                return Some(Pos { line: li, col });
            }
        }
        let rest: Box<dyn Iterator<Item = usize>> = if forward {
            Box::new(0..from.line)
        } else {
            Box::new((from.line + 1..n).rev())
        };
        for li in rest {
            let col = if forward { 0 } else { self.line_len(li) };
            if let Some(col) = self.find_in_line(li, query, col, forward) {
                return Some(Pos { line: li, col });
            }
        }
        None
    }

    fn commit_search(&mut self) {
        let (forward, bytes) = match &self.ui {
            Ui::Search { forward, query } => (*forward, query.clone()),
            _ => return,
        };
        let query = String::from_utf8_lossy(&bytes).into_owned();
        self.ui = Ui::Normal;
        if query.is_empty() {
            self.search = None;
            self.say("EMPTY SEARCH");
            return;
        }
        self.search = Some((forward, query.clone()));
        match self.find_next(self.cursor, &query, forward) {
            Some(pos) => {
                self.goal = 0;
                self.cursor = pos;
                self.ensure_visible();
                self.say("MATCH");
            }
            None => self.say("NO MATCH"),
        }
    }

    fn repeat_search(&mut self, forward: bool) {
        let Some((_, query)) = self.search.clone() else {
            self.say("NO SEARCH");
            return;
        };
        let from = if forward {
            Pos {
                line: self.cursor.line,
                col: self.cursor.col + 1,
            }
        } else {
            self.cursor
        };
        match self.find_next(from, &query, forward) {
            Some(pos) => {
                self.goal = 0;
                self.cursor = pos;
                self.ensure_visible();
            }
            None => self.say("NO MORE MATCHES"),
        }
    }

    // -- commands and clipboard ----------------------------------------------

    fn commit_command(&mut self) {
        let Ui::Command(bytes) = &self.ui else {
            return;
        };
        let cmd = String::from_utf8_lossy(bytes).trim().to_string();
        self.ui = Ui::Normal;
        if let Ok(n) = cmd.parse::<usize>() {
            if n == 0 {
                self.say("LINES START AT 1");
                return;
            }
            self.cursor = Pos {
                line: n - 1,
                col: 0,
            };
            self.goal = 0;
            self.clamp_cursor();
            self.ensure_visible();
            return;
        }
        match cmd.as_str() {
            "q" | "quit" => {
                self.silence();
                self.say("LEAVING");
                self.pending_action = Some(Outcome::Leave);
            }
            "help" => self.help = true,
            "voice" => {
                let on = self.voice.toggle_enabled();
                self.say(if on { "VOICE ON" } else { "VOICE OFF" });
            }
            "preview" => {
                self.voice.preview();
                self.say("PREVIEW");
            }
            "voices" => self.say("SEE: words --voice-doctor"),
            _ if cmd.starts_with("voice ") => {
                let name = cmd[6..].trim();
                if name.is_empty() {
                    self.say("VOICE NAME REQUIRED");
                } else {
                    self.voice.set_voice(name);
                    self.say(format!("VOICE {name} — :preview TO HEAR IT"));
                }
            }
            _ if cmd.starts_with("rate ") => {
                let raw = cmd[5..].trim();
                match raw.parse::<i32>() {
                    Ok(n) => {
                        self.voice.set_rate(n);
                        self.say(format!("RATE {}", self.voice.config().rate));
                    }
                    Err(_) => self.say("BAD RATE"),
                }
            }
            _ if cmd.starts_with("e ") || cmd.starts_with("edit ") => {
                let raw = cmd.split_once(' ').map(|(_, p)| p).unwrap_or("").trim();
                if raw.is_empty() {
                    self.say("PATH REQUIRED");
                    return;
                }
                self.pending_action = Some(Outcome::Open(expand_path(raw)));
            }
            _ => self.say("UNKNOWN COMMAND"),
        }
    }

    fn copy_selection(&mut self) {
        let text = self.selected_text();
        if text.is_empty() {
            self.say("NOTHING SELECTED");
            return;
        }
        let ok = copy_to_clipboard(&text);
        self.say(if ok {
            format!("COPIED {} CHARS", text.chars().count())
        } else {
            "NO CLIPBOARD AVAILABLE".to_string()
        });
    }

    // -- keys ----------------------------------------------------------------

    pub fn key(&mut self, b: u8) -> Outcome {
        self.expire_message();
        self.poll_voice();

        // Prompts own the keyboard: every character typed goes into them.
        match &self.ui {
            Ui::Search { .. } | Ui::Command(_) => return self.prompt_key(b),
            _ => {}
        }

        let count = match &self.pending {
            Pending::Count(s) => s.parse::<usize>().unwrap_or(1).clamp(1, MAX_COUNT),
            _ => 1,
        };
        let pending = std::mem::replace(&mut self.pending, Pending::None);

        // Multi-key sequences: gg, zz and yy are the only ones so far.
        match pending {
            Pending::G if b == b'g' => {
                self.goto_line(count.saturating_sub(1));
                return Outcome::None;
            }
            Pending::Z if b == b'z' => {
                self.center();
                return Outcome::None;
            }
            Pending::Y if b == b'y' => {
                let pos = self.cursor;
                self.cursor = Pos { line: pos.line, col: 0 };
                self.ui = Ui::Visual;
                self.anchor = Some(self.cursor);
                self.cursor.col = self.line_len(pos.line);
                self.copy_selection();
                self.ui = Ui::Normal;
                self.anchor = None;
                self.cursor = pos;
                return Outcome::None;
            }
            _ => {}
        }

        if b.is_ascii_digit() {
            match pending {
                Pending::Count(mut s) => {
                    if s.len() < 6 {
                        s.push(b as char);
                    }
                    self.pending = Pending::Count(s);
                    return Outcome::None;
                }
                _ if b != b'0' => {
                    self.pending = Pending::Count((b as char).to_string());
                    return Outcome::None;
                }
                // A bare `0` is the beginning-of-line motion, handled below.
                _ => {}
            }
        }

        match b {
            b'q' | b'Q' => {
                self.silence();
                self.remember();
                Outcome::Leave
            }
            b'\x03' => {
                self.silence();
                self.remember();
                Outcome::Quit
            }
            0 | 0x1b => {
                // The key reader cannot distinguish a bare escape from an
                // unrecognised escape sequence, so both arrive as NUL.
                if self.ui == Ui::Visual {
                    self.ui = Ui::Normal;
                    self.anchor = None;
                    Outcome::None
                } else {
                    self.silence();
                    self.remember();
                    Outcome::Leave
                }
            }
            0x04 => {
                self.scroll(self.page as isize / 2);
                Outcome::None
            }
            0x15 => {
                self.scroll(-(self.page as isize / 2));
                Outcome::None
            }
            0x06 => {
                self.scroll(self.page as isize);
                Outcome::None
            }
            0x02 => {
                self.scroll(-(self.page as isize));
                Outcome::None
            }
            0x08 => {
                self.help = !self.help;
                Outcome::None
            }
            b' ' => {
                self.toggle_voice();
                Outcome::None
            }
            b's' | b'S' => {
                self.silence();
                self.say("STOPPED");
                Outcome::None
            }
            b'>' => {
                self.jump_sentence(1);
                Outcome::None
            }
            b'<' => {
                self.jump_sentence(-1);
                Outcome::None
            }
            b'[' => {
                let rate = self.voice.config().rate - 10;
                self.voice.set_rate(rate);
                self.say(format!("RATE {}", self.voice.config().rate));
                Outcome::None
            }
            b']' => {
                let rate = self.voice.config().rate + 10;
                self.voice.set_rate(rate);
                self.say(format!("RATE {}", self.voice.config().rate));
                Outcome::None
            }
            b'V' => {
                let on = self.voice.toggle_enabled();
                self.say(if on { "VOICE ON" } else { "VOICE OFF" });
                Outcome::None
            }
            b'h' | KEY_LEFT => {
                for _ in 0..count {
                    self.move_h(-1);
                }
                self.update_goal();
                Outcome::None
            }
            b'l' | KEY_RIGHT => {
                for _ in 0..count {
                    self.move_h(1);
                }
                self.update_goal();
                Outcome::None
            }
            b'j' | KEY_DOWN => {
                self.move_rows(count as isize);
                self.update_goal();
                Outcome::None
            }
            b'k' | KEY_UP => {
                self.move_rows(-(count as isize));
                self.update_goal();
                Outcome::None
            }
            b'w' => {
                self.word_forward(count);
                Outcome::None
            }
            b'b' => {
                self.word_back(count);
                Outcome::None
            }
            b'e' => {
                self.word_end(count);
                Outcome::None
            }
            b'0' => {
                self.cursor.col = self.layout.as_ref().map_or(0, |l| l.row_start(self.row()));
                self.goal = 0;
                Outcome::None
            }
            b'$' => {
                if count > 1 {
                    self.move_rows(count as isize - 1);
                }
                self.cursor.col = self.line_len(self.cursor.line);
                self.update_goal();
                Outcome::None
            }
            b'g' => {
                self.pending = Pending::G;
                Outcome::None
            }
            b'G' => {
                self.goto_line(if count > 1 {
                    count - 1
                } else {
                    self.lines().saturating_sub(1)
                });
                Outcome::None
            }
            b'H' => {
                self.goto_row(self.top);
                Outcome::None
            }
            b'M' => {
                self.goto_row(self.top + self.page / 2);
                Outcome::None
            }
            b'L' => {
                self.goto_row(self.top + self.page.saturating_sub(1));
                Outcome::None
            }
            b'z' => {
                self.pending = Pending::Z;
                Outcome::None
            }
            b'v' => {
                if self.ui == Ui::Visual {
                    self.ui = Ui::Normal;
                    self.anchor = None;
                } else {
                    self.ui = Ui::Visual;
                    self.anchor = Some(self.cursor);
                }
                Outcome::None
            }
            b'y' => {
                if self.ui == Ui::Visual {
                    self.copy_selection();
                    self.ui = Ui::Normal;
                    self.anchor = None;
                } else {
                    self.pending = Pending::Y;
                }
                Outcome::None
            }
            b'/' => {
                self.ui = Ui::Search {
                    forward: true,
                    query: Vec::new(),
                };
                self.help = false;
                Outcome::None
            }
            b'?' => {
                self.ui = Ui::Search {
                    forward: false,
                    query: Vec::new(),
                };
                self.help = false;
                Outcome::None
            }
            b'n' => {
                self.repeat_search(true);
                Outcome::None
            }
            b'N' => {
                self.repeat_search(false);
                Outcome::None
            }
            b':' => {
                self.ui = Ui::Command(Vec::new());
                self.help = false;
                Outcome::None
            }
            b'\n' | b'\r' => {
                self.move_rows(count as isize);
                Outcome::None
            }
            _ => {
                self.pending = Pending::None;
                Outcome::None
            }
        }
    }

    fn prompt_key(&mut self, b: u8) -> Outcome {
        match &mut self.ui {
            Ui::Search { query, .. } => match b {
                0 | 0x1b => {
                    self.ui = Ui::Normal;
                    self.say("SEARCH CANCELLED");
                }
                b'\n' | b'\r' => self.commit_search(),
                0x7f | 0x08 => {
                    query.pop();
                }
                c if c >= 0x20 && c != 0x7f => query.push(c),
                _ => {}
            },
            Ui::Command(buf) => match b {
                0 | 0x1b => {
                    self.ui = Ui::Normal;
                    self.say("COMMAND CANCELLED");
                }
                b'\n' | b'\r' => self.commit_command(),
                0x7f | 0x08 => {
                    buf.pop();
                }
                c if c >= 0x20 && c != 0x7f => buf.push(c),
                _ => {}
            },
            _ => {}
        }
        self.pending_action.take().unwrap_or(Outcome::None)
    }

    /// Horizontal motion by one cell on the current display row, crossing a
    /// wrap at either end. `0` and `$` use the raw row bounds directly.
    fn move_h(&mut self, dir: isize) {
        let Some(layout) = &self.layout else {
            return;
        };
        if layout.rows.is_empty() {
            return;
        }
        let row = self.row() as isize + if dir > 0 && self.cursor.col >= layout.row_end(self.row()) {
            1
        } else if dir < 0 && self.cursor.col <= layout.row_start(self.row()) {
            -1
        } else {
            0
        };
        let row = row.clamp(0, layout.rows.len() as isize - 1) as usize;
        let r = layout.rows[row];
        let target = if dir > 0 {
            self.cursor.col + 1
        } else {
            self.cursor.col.saturating_sub(1)
        };
        self.cursor = Pos {
            line: r.line,
            col: target.clamp(r.start, r.end),
        };
        self.ensure_visible();
    }

    fn goto_line(&mut self, line: usize) {
        self.cursor = Pos { line, col: 0 };
        self.goal = 0;
        self.clamp_cursor();
        self.ensure_visible();
    }

    fn center(&mut self) {
        let r = self.row();
        self.top = r.saturating_sub(self.page / 2);
        self.ensure_visible();
    }

    /// Draw into the frame buffer. Kept as a method so callers do not have to
    /// know how the reader stores its state.
    pub fn draw(&mut self, o: &mut crate::term::Out, cols: usize, rows: usize) {
        self.set_size(cols, rows);
        self.poll_voice();
        render::draw(self, o, cols, rows);
    }
}

/// Expand the conventional `~/` prefix. Everything else is left for the
/// document loader to report on by name.
fn expand_path(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(p)
}

fn copy_to_clipboard(text: &str) -> bool {
    let tools: [(&str, &[&str]); 3] = [
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
        ("pbcopy", &[]),
    ];
    for (name, args) in tools {
        let Ok(mut child) = Command::new(name)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            continue;
        };
        let wrote = child
            .stdin
            .take()
            .map(|mut s| s.write_all(text.as_bytes()).is_ok())
            .unwrap_or(false);
        if wrote && child.wait().is_ok_and(|s| s.success()) {
            return true;
        }
    }
    false
}

/// Remember the last reading position per document, in a plain text file in
/// the config directory. Lines are `path<TAB>line<TAB>col`; a malformed line
/// is skipped rather than refusing the rest.
mod history {
    use super::*;

    fn key(path: &Path) -> String {
        let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        canonical.to_string_lossy().into_owned()
    }

    pub fn load(path: &Path) -> Option<Pos> {
        load_at(config::positions_file()?, path)
    }

    pub(super) fn load_at(file: PathBuf, path: &Path) -> Option<Pos> {
        let text = fs::read_to_string(file).ok()?;
        let wanted = key(path);
        text.lines().find_map(|line| {
            let mut fields = line.split('\t');
            let (p, l, c) = (fields.next()?, fields.next()?, fields.next()?);
            if p != wanted {
                return None;
            }
            Some(Pos {
                line: l.parse().ok()?,
                col: c.parse().ok()?,
            })
        })
    }

    pub fn save(path: &Path, pos: Pos) {
        if let Some(file) = config::positions_file() {
            save_at(file, path, pos);
        }
    }

    pub(super) fn save_at(file: PathBuf, path: &Path, pos: Pos) {
        let mut lines: Vec<String> = fs::read_to_string(&file)
            .unwrap_or_default()
            .lines()
            .filter(|l| {
                l.split_once('\t')
                    .is_some_and(|(p, _)| p != key(path))
            })
            .map(str::to_string)
            .collect();
        lines.push(format!("{}\t{}\t{}", key(path), pos.line, pos.col));
        let _ = fs::write(file, lines.join("\n") + "\n");
    }
}

#[cfg(test)]
mod tests;

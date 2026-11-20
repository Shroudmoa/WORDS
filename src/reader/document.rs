//! Document loading and Markdown rendering.
//!
//! A document is kept as logical lines of already-rendered text: Markdown
//! markers become styles, not characters. The original bytes stay in `raw` so
//! nothing about the file is lost; the rendered lines are what layout, search
//! and selection work on.

use std::fs;
use std::path::{Path, PathBuf};

/// A ceiling on what one document may occupy. Reader Mode is for reading, not
/// for gropping through a core dump; a larger file is refused with a message.
pub const MAX_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_LINES: usize = 200_000;

/// A tab is four spaces in a terminal. Expanding here keeps every later
/// character index an index into a rendered line.
const TAB: &str = "    ";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Txt,
    Md,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Txt => "TXT",
            Kind::Md => "MD",
        }
    }
}

/// Which document formats this build understands.
pub fn kind_of(path: &Path) -> Option<Kind> {
    match path.extension().and_then(|e| e.to_str()) {
        Some(e) if e.eq_ignore_ascii_case("txt") => Some(Kind::Txt),
        Some(e) if e.eq_ignore_ascii_case("md") => Some(Kind::Md),
        _ => None,
    }
}

/// A text style on a range of characters in a line. Ranges are half-open and
/// measured in `char`s, not bytes, so Unicode prose behaves.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Style {
    Plain,
    Heading(u8),
    Bold,
    Italic,
    Code,
    Quote,
    Bullet,
    Rule,
    Link,
}

/// A run of characters sharing one style.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub style: Style,
}

/// One logical line: the text a reader sees, plus its styling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    pub spans: Vec<Span>,
}

impl Line {
    pub fn blank() -> Line {
        Line {
            text: String::new(),
            spans: Vec::new(),
        }
    }

    pub fn single(text: impl Into<String>, style: Style) -> Line {
        let text = text.into();
        let end = text.chars().count();
        let spans = if end == 0 {
            Vec::new()
        } else {
            vec![Span {
                start: 0,
                end,
                style,
            }]
        };
        Line { text, spans }
    }

    pub fn len(&self) -> usize {
        self.text.chars().count()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

/// A loaded document. `raw` is untouched file content; `lines` are what gets
/// laid out on screen.
#[derive(Clone, Debug)]
pub struct Document {
    pub path: PathBuf,
    pub title: String,
    pub kind: Kind,
    /// The file's content, exactly as read (lossy only for invalid UTF-8).
    /// Rendering never writes through it; keeping it intact leaves room for
    /// bookmarks, speech and diffing without re-reading the file.
    #[allow(dead_code)]
    pub raw: String,
    pub lines: Vec<Line>,
}

impl Document {
    /// Load a `.txt` or `.md` file. Errors name the path and the reason, so
    /// the caller can show them rather than leaving a blank screen.
    pub fn load(path: &Path) -> Result<Document, String> {
        let kind = kind_of(path)
            .ok_or_else(|| format!("{}: reader supports .txt and .md", path.display()))?;
        let meta = fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if !meta.is_file() {
            return Err(format!("{}: not a file", path.display()));
        }
        if meta.len() > MAX_BYTES {
            return Err(format!(
                "{}: document is larger than {} MiB",
                path.display(),
                MAX_BYTES / (1024 * 1024)
            ));
        }
        let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        // A binary file with a .txt name is still readable: replacement
        // characters are honest about what could not be decoded.
        let raw = String::from_utf8_lossy(&bytes).into_owned();
        if raw.lines().count() > MAX_LINES {
            return Err(format!("{}: more than {MAX_LINES} lines", path.display()));
        }
        Ok(Self::from_text(path.to_path_buf(), kind, &raw))
    }

    /// Build a document from text, without touching the filesystem. Also the
    /// seam the tests use to exercise layout and navigation on tiny inputs.
    pub fn from_text(path: PathBuf, kind: Kind, raw: &str) -> Document {
        let lines = match kind {
            Kind::Txt => plain(raw),
            Kind::Md => markdown(raw),
        };
        let title = title_of(raw, kind).unwrap_or_else(|| {
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "DOCUMENT".to_string())
        });
        Document {
            path,
            title,
            kind,
            raw: raw.to_string(),
            lines,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(Line::is_empty)
    }
}

fn title_of(raw: &str, kind: Kind) -> Option<String> {
    if kind != Kind::Md {
        return None;
    }
    raw.split('\n')
        .take(32)
        .find_map(|line| heading(line).and_then(|(_, text)| {
            let t = text.trim();
            (!t.is_empty()).then(|| t.to_string())
        }))
}

/// Plain text: one line per source line, with blank lines kept for paragraph
/// spacing and tabs expanded so columns mean something.
fn plain(raw: &str) -> Vec<Line> {
    if raw.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut lines: Vec<&str> = raw.split('\n').collect();
    if raw.ends_with('\n') {
        lines.pop();
    }
    for line in lines {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let text = line.replace('\t', TAB);
        out.push(if text.is_empty() {
            Line::blank()
        } else {
            Line::single(text, Style::Plain)
        });
    }
    out
}

/// A deliberately small Markdown renderer: headings, emphasis, inline code,
/// links, lists, blockquotes, fenced code and rules. Unknown syntax is left as
/// text, so nothing a document says is silently swallowed.
fn markdown(raw: &str) -> Vec<Line> {
    if raw.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut fence: Option<String> = None;
    let mut lines: Vec<&str> = raw.split('\n').collect();
    if raw.ends_with('\n') {
        lines.pop();
    }

    for raw_line in lines {
        let raw_line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let line = raw_line.replace('\t', TAB);
        let trimmed = line.trim();

        if let Some(marker) = fence.clone() {
            if trimmed.starts_with(&marker) {
                fence = None;
                out.push(Line::blank());
            } else {
                out.push(Line::single(line, Style::Code));
            }
            continue;
        }
        if let Some(marker) = fence_start(trimmed) {
            fence = Some(marker);
            out.push(Line::blank());
            continue;
        }
        if trimmed.is_empty() {
            out.push(Line::blank());
            continue;
        }
        if let Some((level, text)) = heading(&line) {
            out.push(inline(text, Style::Heading(level)));
            continue;
        }
        if is_rule(trimmed) {
            out.push(Line::single("\u{2500}".repeat(24), Style::Rule));
            continue;
        }
        if let Some(rest) = line.strip_prefix("> ").or_else(|| line.strip_prefix('>')) {
            out.push(inline(rest, Style::Quote));
            continue;
        }
        if let Some((prefix, rest)) = list_prefix(&line) {
            let mut l = Line::single(prefix, Style::Bullet);
            let tail = inline(rest, Style::Plain);
            let off = l.len();
            l.text.push_str(&tail.text);
            l.spans.extend(tail.spans.into_iter().map(|s| Span {
                start: s.start + off,
                end: s.end + off,
                style: s.style,
            }));
            out.push(l);
            continue;
        }
        out.push(inline(&line, Style::Plain));
    }
    out
}

fn fence_start(s: &str) -> Option<String> {
    for mark in ["```", "~~~"] {
        if s.starts_with(mark) {
            return Some(mark.to_string());
        }
    }
    None
}

fn heading(line: &str) -> Option<(u8, &str)> {
    let n = line.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&n) && line.as_bytes().get(n) == Some(&b' ') {
        Some((n as u8, &line[n + 1..]))
    } else {
        None
    }
}

fn is_rule(s: &str) -> bool {
    let compact: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    compact.len() >= 3 && compact.chars().all(|c| c == compact.chars().next().unwrap())
        && matches!(compact.chars().next(), Some('-' | '*' | '_'))
}

fn list_prefix(line: &str) -> Option<(String, &str)> {
    for mark in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(mark) {
            return Some(("\u{2022} ".to_string(), rest));
        }
    }
    // An ordered list keeps its number: it says something the bullet does not.
    let digits = line.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 && digits < 4 && line[digits..].starts_with(". ") {
        return Some((line[..digits + 2].to_string(), &line[digits + 2..]));
    }
    None
}

/// Turn inline Markdown into a styled line. Markers are consumed; the text
/// between them is kept verbatim, including spaces.
fn inline(text: &str, base: Style) -> Line {
    let src: Vec<char> = text.chars().collect();
    let mut chars: Vec<char> = Vec::with_capacity(src.len());
    let mut spans: Vec<Span> = Vec::new();
    let mut i = 0;

    while i < src.len() {
        // Backslash escapes the next character.
        if src[i] == '\\' && i + 1 < src.len() {
            push(&mut chars, &mut spans, &src[i + 1..i + 2], base);
            i += 2;
            continue;
        }
        // Inline code wins over emphasis: backticks are not word markers.
        if src[i] == '`' {
            if let Some(j) = find(&src, i + 1, &['`']) {
                push(&mut chars, &mut spans, &src[i + 1..j], Style::Code);
                i = j + 1;
                continue;
            }
        }
        // Links: keep the label, drop the destination (it is not prose).
        if src[i] == '[' {
            if let Some(close) = find(&src, i + 1, &[']']) {
                if src.get(close + 1) == Some(&'(') {
                    if let Some(end) = find(&src, close + 2, &[')']) {
                        push(&mut chars, &mut spans, &src[i + 1..close], Style::Link);
                        i = end + 1;
                        continue;
                    }
                }
            }
        }
        // Emphasis markers: try the double form first, so an unclosed `**`
        // is never mistaken for a pair of single `*` markers.
        if matches!(src[i], '*' | '_') {
            let c = src[i];
            let double = [c, c];
            if starts(&src, i, &double) {
                if let Some(j) = find(&src, i + 2, &double) {
                    push(&mut chars, &mut spans, &src[i + 2..j], Style::Bold);
                    i = j + 2;
                } else {
                    // A marker with no close is text, not a style change.
                    push(&mut chars, &mut spans, &double, base);
                    i += 2;
                }
                continue;
            }
            if let Some(j) = find(&src, i + 1, &[c]) {
                push(&mut chars, &mut spans, &src[i + 1..j], Style::Italic);
                i = j + 1;
                continue;
            }
        }
        push(&mut chars, &mut spans, &src[i..i + 1], base);
        i += 1;
    }

    Line {
        text: chars.into_iter().collect(),
        spans,
    }
}

fn starts(src: &[char], at: usize, mark: &[char]) -> bool {
    at + mark.len() <= src.len() && &src[at..at + mark.len()] == mark
}

fn find(src: &[char], from: usize, mark: &[char]) -> Option<usize> {
    if mark.is_empty() || from >= src.len() {
        return None;
    }
    (from..=src.len() - mark.len()).find(|&i| &src[i..i + mark.len()] == mark)
}

/// Append a run of source characters and record the span it occupies.
/// Adjacent runs with the same style are coalesced so rendering emits one
/// SGR per style change, not one per character.
fn push(chars: &mut Vec<char>, spans: &mut Vec<Span>, run: &[char], style: Style) {
    if run.is_empty() {
        return;
    }
    let start = chars.len();
    chars.extend_from_slice(run);
    let end = chars.len();
    if let Some(last) = spans.last_mut() {
        if last.end == start && last.style == style {
            last.end = end;
            return;
        }
    }
    spans.push(Span { start, end, style });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn md(s: &str) -> Vec<Line> {
        markdown(s)
    }

    #[test]
    fn plain_text_keeps_paragraphs_and_expands_tabs() {
        let lines = plain("one\n\ntwo\tthree\n");
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].text, "one");
        assert!(lines[1].is_empty());
        assert_eq!(lines[2].text, "two    three");
        assert_eq!(lines[0].spans[0].style, Style::Plain);
    }

    #[test]
    fn an_empty_document_has_no_lines() {
        assert!(plain("").is_empty());
        assert!(md("").is_empty());
        assert!(Document::from_text(PathBuf::from("x.txt"), Kind::Txt, "").is_empty());
    }

    #[test]
    fn a_markdown_heading_is_styled_and_stripped() {
        let lines = md("# Hello\n\n## Sub *title*\n");
        assert_eq!(lines[0].text, "Hello");
        assert_eq!(lines[0].spans[0].style, Style::Heading(1));
        assert!(lines[1].is_empty());
        assert_eq!(lines[2].text, "Sub title");
        assert!(lines[2].spans.iter().any(|s| s.style == Style::Italic));
    }

    #[test]
    fn emphasis_code_and_links_render_as_text_with_styles() {
        let l = &md("a **bold** and *it* and `code` and [label](http://x)\n")[0];
        assert_eq!(l.text, "a bold and it and code and label");
        let styles: Vec<Style> = l
            .spans
            .iter()
            .flat_map(|s| (s.start..s.end).map(|_| s.style))
            .collect();
        assert_eq!(styles[2], Style::Bold);
        assert!(styles.contains(&Style::Italic));
        assert!(styles.contains(&Style::Code));
        assert!(styles.contains(&Style::Link));
    }

    #[test]
    fn lists_quotes_rules_and_fences_render() {
        let lines = md("- one\n1. two\n> quote\n---\n```\nraw **not bold**\n```\n");
        assert_eq!(lines[0].text, "\u{2022} one");
        assert_eq!(lines[0].spans[0].style, Style::Bullet);
        assert_eq!(lines[1].text, "1. two");
        assert_eq!(lines[2].text, "quote");
        assert!(lines[2].spans.iter().all(|s| s.style == Style::Quote));
        assert_eq!(lines[3].spans[0].style, Style::Rule);
        assert_eq!(lines[5].text, "raw **not bold**");
        assert!(lines[5].spans.iter().all(|s| s.style == Style::Code));
    }

    #[test]
    fn markup_that_never_closes_is_left_alone() {
        assert_eq!(md("**open\n")[0].text, "**open");
        assert_eq!(md("[label\n")[0].text, "[label");
    }

    #[test]
    fn the_original_content_is_preserved_next_to_the_rendered_lines() {
        let src = "# Title\n\n- **bold** item\n";
        let d = Document::from_text(PathBuf::from("x.md"), Kind::Md, src);
        assert_eq!(d.raw, src);
        assert_ne!(d.lines[0].text, "# Title", "markers are rendered away");
    }

    #[test]
    fn a_title_comes_from_the_first_heading() {
        let d = Document::from_text(
            PathBuf::from("notes.md"),
            Kind::Md,
            "some intro\n# The Title\nbody\n",
        );
        assert_eq!(d.title, "The Title");
        let d = Document::from_text(PathBuf::from("notes.txt"), Kind::Txt, "# not markdown\n");
        assert_eq!(d.title, "notes.txt");
    }

    #[test]
    fn unsupported_and_missing_paths_are_refused_by_name() {
        let err = Document::load(Path::new("/nonexistent/nope.pdf")).unwrap_err();
        assert!(err.contains(".txt and .md"), "{err}");
        let err = Document::load(Path::new("/nonexistent/nope.txt")).unwrap_err();
        assert!(err.contains("nope.txt"), "{err}");
    }
}

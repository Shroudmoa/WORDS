//! The typewriter itself: state machine, timing model, and key handling.

use crate::font;
use crate::phrases;
use crate::style::{self, Rng};

// -- tuning -----------------------------------------------------------------

pub const MIN_CPS: f64 = 4.0;
pub const MAX_CPS: f64 = 110.0;
pub const MIN_HOLD: f64 = 0.7;
pub const MAX_HOLD: f64 = 7.0;
pub const MIN_GAP: f64 = 0.25;
pub const MAX_GAP: f64 = 6.0;
/// How long the controls linger after your last keypress.
pub const HUD_IDLE: f64 = 3.0;
pub const BOOT_TIME: f64 = 0.8;
pub const GLITCH_ON_TYPED: f64 = 0.05;
pub const GLITCH_AMBIENT: (f64, f64) = (4.0, 11.0);
pub const GLITCH_LEN: (f64, f64) = (0.04, 0.15);

/// Sentinels the key reader produces for arrow keys (which arrive as 3 bytes).
pub const KEY_UP: u8 = 0x80;
pub const KEY_DOWN: u8 = 0x81;
pub const KEY_LEFT: u8 = 0x82;
pub const KEY_RIGHT: u8 = 0x83;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Boot,
    Typing,
    Holding,
    Gap,
    FadingOut,
    Resting,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Boot => "BOOT",
            Phase::Typing => "WRITE",
            Phase::Holding => "HOLD",
            Phase::Gap => "NEXT",
            Phase::FadingOut => "FADE",
            Phase::Resting => "IDLE",
        }
    }
}

pub enum Action {
    None,
    Quit,
}

pub struct Glyph {
    /// Source character. The terminal font draws this; the pixel font draws the
    /// canvas beside it.
    pub ch: char,
    /// `Some` in pixel-font mode, `None` when the terminal's own font is in use.
    pub canvas: Option<style::Canvas>,
    pub col: usize,
    pub row: usize,
    pub w: usize,
    /// Where it was last painted, so a jittered glyph can be wiped clean.
    pub at: Option<(usize, usize)>,
}

pub struct Engine {
    // content
    pub phrases: Vec<String>,
    pub idx: usize,
    pub glyphs: Vec<Glyph>,
    pub typed: usize,
    pub cycle: u32,
    pub text: String,

    // timing
    pub phase: Phase,
    pub level: f64,
    pub budget: f64,
    pub hold: f64,
    pub hold_total: f64,
    pub gap: f64,
    pub boot: f64,
    pub rest: f64,
    pub vis: f64,
    pub vis_target: f64,
    pub elapsed: f64,
    pub idle: f64,

    // presentation
    pub cols: u16,
    pub rows: u16,
    /// Vertical scale of the pixel font only. Always 1 in terminal-font mode.
    pub cell_h: usize,
    /// `true` = the terminal's own font (readable, the default).
    /// `false` = the 5x7 pixel font, drawn with background colour.
    pub pixel: bool,
    pub glitch: f64,
    pub next_glitch: f64,
    pub rand: Rng,
    pub block: Option<(usize, usize, usize, usize)>,
    pub painted_upto: usize,
    pub help: bool,
    pub rapid: bool,
    pub pin: bool,
    pub hud: f64,
    pub hud_target: f64,
    /// A short message for the control strip -- currently only "RELOADED" --
    /// with the seconds it has left to show.
    pub notice: Option<(String, f64)>,
    pub full: bool,
    /// Where the last painted cursor sat, as `(x, y, cells tall)`. The terminal
    /// has no per-frame clear, and the cursor moves with every keystroke, so the
    /// frame that moves it has to repaint what it was covering.
    pub cursor_at: Option<(usize, usize, usize)>,
    /// Where the control bar and strip were last painted, as `(bar row, rows)`.
    /// The terminal has no per-frame clear, so the next frame has to wipe this
    /// before it can stop drawing there.
    pub hud_block: Option<(usize, usize)>,
    /// The last thing painted outside those footprints -- the boot overlay or
    /// the help panel -- as `(x, y, w, h)`, wiped the frame it goes away.
    pub overlay: Option<(usize, usize, usize, usize)>,
}

impl Engine {
    pub fn new(seed: u64, cols: u16, rows: u16) -> Engine {
        let mut rand = Rng::new(seed);
        let mut phrases: Vec<String> =
            phrases::PRELUDE.iter().map(|s| s.to_string()).collect();
        let prelude = phrases.len();
        phrases.extend(phrases::pool(&mut rand));
        phrases::shuffle(&mut phrases[prelude..], &mut rand);

        let mut e = Engine {
            phrases,
            idx: 0,
            glyphs: Vec::new(),
            typed: 0,
            cycle: 1,
            text: String::new(),
            phase: Phase::Boot,
            level: 34.0,
            budget: 0.0,
            hold: 0.0,
            hold_total: 0.0,
            gap: 0.0,
            boot: 0.0,
            rest: 0.0,
            vis: 0.0,
            vis_target: 1.0,
            elapsed: 0.0,
            idle: 0.0,
            cols,
            rows,
            cell_h: 1,
            pixel: false,
            glitch: 0.0,
            next_glitch: 99.0,
            rand,
            block: None,
            painted_upto: 0,
            cursor_at: None,
            help: false,
            rapid: false,
            pin: false,
            hud: 0.0,
            hud_target: 1.0,
            notice: None,
            full: true,
            hud_block: None,
            overlay: None,
        };
        e.next_glitch = e.glitch_soon(GLITCH_AMBIENT.0, GLITCH_AMBIENT.1);
        e.load(0);
        e
    }

    // -- speed model ---------------------------------------------------------
    //
    // One knob drives all three timings, so "faster" always means the same
    // thing: quicker strokes, shorter holds, shorter gaps.

    pub fn cps(&self) -> f64 {
        MIN_CPS * (MAX_CPS / MIN_CPS).powf(self.level / 100.0)
    }
    pub fn hold_len(&self) -> f64 {
        MIN_HOLD + (MAX_HOLD - MIN_HOLD) * (self.level / 100.0).powf(1.15)
    }
    pub fn gap_len(&self) -> f64 {
        MIN_GAP + (MAX_GAP - MIN_GAP) * (self.level / 100.0).powf(1.15)
    }

    // -- fonts ---------------------------------------------------------------
    //
    // Two ways to put a character on screen. The terminal's own font is the
    // default because it is the only one of the two you can actually read; the
    // 5x7 pixel font is a single `F` away for anyone who wants the block look.
    //
    // `char_w` and `gap` are the two numbers every piece of layout agrees on,
    // so they are free functions rather than methods: `load` needs them before
    // the engine's own font state is settled.

    /// Cells tall for one row of text at the pixel font's current scale.
    pub fn line_h(&self) -> usize {
        self.line_h_at(self.cell_h)
    }

    fn line_h_at(&self, cell_h: usize) -> usize {
        if self.pixel {
            font::ROWS * cell_h
        } else {
            1
        }
    }

    // -- content -------------------------------------------------------------

    /// Usable width for text: the window minus the two side rails and a gutter.
    /// The terminal font is capped well below that, so a long sentence wraps
    /// into a tidy centred column instead of stretching across a wide window.
    fn max_text_cols(&self) -> usize {
        let avail = (self.cols as usize).saturating_sub(4).max(12);
        if self.pixel {
            avail
        } else {
            avail.min(56)
        }
    }

    /// Rows the chrome costs us: the top rule above, and the bar, its scale and
    /// the control strip below. On a window too short for that, the chrome gives
    /// way so at least one line of text always fits.
    const CHROME_ROWS: usize = 5;

    /// Shortest line we can lay out, which is what the chrome has to leave room
    /// for.
    fn min_line_h(&self) -> usize {
        self.line_h_at(if self.pixel { 2 } else { 1 })
    }

    /// Usable height for text, in rows of `line_h_at(cell_h)` cells. Returns 0
    /// when even a single line at this scale wouldn't fit under the chrome.
    fn max_text_rows(&self, cell_h: usize) -> usize {
        let rows = self.rows as usize;
        let chrome = Self::CHROME_ROWS.min(rows.saturating_sub(self.min_line_h()));
        rows.saturating_sub(chrome) / self.line_h_at(cell_h)
    }

    /// Rows actually reserved for chrome, mirroring `max_text_rows`.
    pub fn chrome_rows(&self) -> usize {
        let rows = self.rows as usize;
        Self::CHROME_ROWS.min(rows.saturating_sub(self.min_line_h()))
    }

    /// Flow the phrase into glyphs.
    ///
    /// Preference order: keep the requested font if the whole phrase fits, drop
    /// to the small font rather than truncate, and only ever clip a phrase that
    /// won't fit at the small font either (a very short window, say).
    fn load(&mut self, idx: usize) {
        let pixel = self.pixel;
        let n = self.phrases.len();
        if n == 0 {
            return;
        }
        // A phrase the current font has no way to show -- the pixel font has no
        // Cyrillic, say -- is stepped over rather than rendered as a blank, and
        // an undisplayable list leaves the current phrase alone.
        let mut idx = idx % n;
        let text = loop {
            let t = if pixel {
                phrases::ascii(&self.phrases[idx])
            } else {
                phrases::screen(&self.phrases[idx])
            };
            if !t.is_empty() {
                break t;
            }
            let next = (idx + 1) % n;
            if next == idx {
                return;
            }
            idx = next;
        };
        self.idx = idx;
        let max_w = self.max_text_cols();

        // Try the requested scale, then the small one. A scale that shows the
        // whole phrase always wins; failing that, show as much as possible and
        // keep the larger font when it isn't what cost us the words.
        let want = if pixel { 2 } else { 1 };
        let scales: &[usize] = if want == 1 { &[1] } else { &[2, 1] };
        let mut best: Option<(usize, Vec<Vec<String>>, bool)> = None;
        for &cell_h in scales {
            let rows = self.max_text_rows(cell_h);
            if rows == 0 {
                continue; // no room for a single line at this scale
            }
            let (mut lines, mut cut) = flow(&text, max_w, rows, pixel);
            // Never hand back more rows than the window can show.
            if lines.len() > rows {
                lines.truncate(rows);
                cut = true;
            }
            let better = match &best {
                None => true,
                Some((_bc, bl, bcut)) => match (*bcut, cut) {
                    (true, false) => true,
                    (false, true) => false,
                    // Both clipped: prefer more words on screen.
                    _ => words(&lines) > words(bl),
                },
            };
            if better {
                best = Some((cell_h, lines, cut));
            }
            if !cut {
                break;
            }
        }
        // Nothing fit: the window is shorter than one line of text. Show a
        // single clipped line rather than nothing.
        let (cell_h, lines, cut) = best.unwrap_or_else(|| {
            let (l, c) = flow(&text, max_w, 1, pixel);
            (1, l, c)
        });
        self.cell_h = cell_h;
        let gap = gap(pixel);

        // Centre each line inside the block, so a wrapped sentence reads as one
        // centred block rather than a ragged left-aligned column.
        let widths: Vec<usize> = lines
            .iter()
            .map(|l| {
                l.iter()
                    .map(|word| word.chars().map(|c| char_w(pixel, c)).sum::<usize>())
                    .sum::<usize>()
                    + gap * l.len().saturating_sub(1)
            })
            .collect();
        let widest = widths.iter().copied().max().unwrap_or(0);

        self.glyphs.clear();
        for (row, line) in lines.iter().enumerate() {
            let mut col = (widest - widths[row]) / 2;
            for (i, word) in line.iter().enumerate() {
                if i > 0 {
                    col += gap;
                }
                for c in word.chars() {
                    let w = char_w(pixel, c);
                    self.glyphs.push(Glyph {
                        ch: c,
                        canvas: pixel.then(|| style::Canvas::from_font(&font::of(c), cell_h)),
                        col,
                        row,
                        w,
                        at: None,
                    });
                    col += w;
                }
            }
        }
        // Signal that the sentence was clipped rather than shown in full.
        if cut {
            let last = self.glyphs.len().saturating_sub(1);
            if let Some(g) = self.glyphs.get_mut(last) {
                g.ch = '.';
                g.canvas = pixel.then(|| style::Canvas::from_font(&font::DOT, cell_h));
            }
        }

        self.text = text;
        self.typed = 0;
        self.painted_upto = 0;
        self.cursor_at = None;
        self.full = true;
        self.schedule_hold();
    }

    fn schedule_hold(&mut self) {
        self.hold_total = (self.hold_len() * (0.825 + self.rand.f64() * 0.35)).max(0.12);
        self.hold = self.hold_total;
        self.gap = (self.gap_len() * (0.8 + self.rand.f64() * 0.4)).max(0.05);
    }

    fn glitch_soon(&mut self, lo: f64, hi: f64) -> f64 {
        lo + self.rand.f64() * (hi - lo)
    }

    fn kick(&mut self) {
        self.glitch = self.glitch_soon(GLITCH_LEN.0, GLITCH_LEN.1);
    }

    // -- simulation ----------------------------------------------------------

    pub fn update(&mut self, dt: f64) {
        if self.rapid {
            self.level = (self.level + dt * 26.0) % 100.0;
        }
        self.idle += dt;
        self.elapsed += dt;

        // The word fading in / out.
        self.vis += (self.vis_target - self.vis) * (dt * 3.4).min(1.0);

        let was_glitching = self.glitch > 0.0;
        if was_glitching {
            self.glitch -= dt;
            if self.glitch <= 0.0 {
                // The final turbulent frame leaves debris; repaint once more.
                self.glitch = 0.0;
                self.full = true;
            } else {
                self.full = true;
            }
        }
        self.next_glitch -= dt;
        if self.next_glitch <= 0.0 {
            self.next_glitch = self.glitch_soon(GLITCH_AMBIENT.0, GLITCH_AMBIENT.1);
            self.kick();
        }

        self.hud += (self.hud_target - self.hud) * (dt * 7.0).min(1.0);

        if let Some((_, left)) = &mut self.notice {
            *left -= dt;
            if *left <= 0.0 {
                self.notice = None;
            }
        }

        match self.phase {
            Phase::Boot => {
                self.boot += dt;
                if self.boot >= BOOT_TIME {
                    self.phase = Phase::Typing;
                    self.full = true;
                }
            }
            Phase::Typing => {
                self.budget += self.cps() * dt;
                while self.budget >= 1.0 && self.typed < self.glyphs.len() {
                    self.budget -= 1.0;
                    self.typed += 1;
                    if self.rand.chance(GLITCH_ON_TYPED) {
                        self.kick();
                    }
                }
                if self.typed >= self.glyphs.len() {
                    self.phase = Phase::Holding;
                    self.schedule_hold();
                    // Longer lines deserve a longer look.
                    self.hold_total += self.glyphs.len() as f64 * 0.02;
                    self.hold = self.hold_total;
                }
            }
            Phase::Holding => {
                self.hold -= dt;
                if self.hold <= 0.0 {
                    self.phase = Phase::Gap;
                }
            }
            Phase::Gap => {
                self.gap -= dt;
                if self.gap <= 0.0 {
                    self.advance_line();
                }
            }
            Phase::FadingOut => {
                if self.vis <= 0.02 {
                    self.phase = Phase::Resting;
                    self.rest = 1.7;
                    self.full = true;
                }
            }
            Phase::Resting => {
                self.rest -= dt;
                if self.rest <= 0.0 {
                    self.cycle += 1;
                    self.phrases = phrases::pool(&mut self.rand);
                    self.vis = 0.0;
                    self.load(0);
                    self.phase = Phase::Typing;
                }
            }
        }
    }

    fn advance_line(&mut self) {
        if self.idx + 1 < self.phrases.len() {
            self.load(self.idx + 1);
            self.phase = Phase::Typing;
        } else {
            self.phase = Phase::FadingOut;
            self.vis_target = 0.0;
        }
    }

    pub fn skip(&mut self) {
        if matches!(self.phase, Phase::Boot | Phase::Resting) {
            return;
        }
        self.vis = 1.0;
        self.vis_target = 1.0;
        self.advance_line();
        if self.phase == Phase::FadingOut {
            self.vis = 0.0;
        }
    }

    // -- input ---------------------------------------------------------------

    pub fn key(&mut self, b: u8) -> Action {
        self.idle = 0.0;
        match b {
            b'q' | b'Q' | 0x03 | 0x04 => return Action::Quit,
            b'?' | b'h' | b'H' => {
                // The panel paints over the middle of the screen and nothing
                // else redraws that area, so closing it needs a forced repaint
                // to wipe the backdrop and border it left behind.
                self.help = !self.help;
                self.full = true;
            }
            b'r' | b'R' => self.rapid = !self.rapid,
            b'g' | b'G' => self.kick(),
            b'b' | b'B' => self.pin = !self.pin,
            b'f' | b'F' => {
                self.pixel = !self.pixel;
                let idx = self.idx;
                self.load(idx);
                if matches!(self.phase, Phase::FadingOut | Phase::Resting) {
                    self.phase = Phase::FadingOut;
                } else {
                    self.phase = Phase::Typing;
                }
            }
            b' ' | b'\n' | b'\r' => self.skip(),
            b'w' | b'W' | b'+' | b'=' | KEY_UP => {
                self.level = (self.level + 8.0).min(100.0);
                self.rapid = false;
                self.schedule_hold();
            }
            b's' | b'S' | b'-' | b'_' | KEY_DOWN => {
                self.level = (self.level - 8.0).max(0.0);
                self.rapid = false;
                self.schedule_hold();
            }
            KEY_LEFT => {
                if self.idx > 0 {
                    let i = self.idx - 1;
                    self.load(i);
                    self.phase = Phase::Typing;
                }
            }
            KEY_RIGHT => self.skip(),
            _ => {}
        }
        Action::None
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        if cols != self.cols || rows != self.rows {
            self.cols = cols;
            self.rows = rows;
            self.full = true;
        }
    }

    /// The bar shows while you're using it and leaves three seconds after your
    /// last keypress. `pin`, auto-ramp and the help panel all hold it open.
    pub fn hide_hud(&mut self) {
        let want = if self.pin || self.rapid || self.help || self.idle < HUD_IDLE {
            1.0
        } else {
            0.0
        };
        self.hud_target = want;
    }

    /// Swap in a user-supplied phrase list and start over with it.
    pub fn use_phrases(&mut self, mut v: Vec<String>) {
        phrases::shuffle(&mut v, &mut self.rand);
        self.phrases = v;
        self.vis = 0.0;
        self.vis_target = 1.0;
        self.load(0);
        self.phase = Phase::Typing;
    }

    /// Swap in a whole set of phrase sections, mixed into one cycle.
    pub fn use_sections(&mut self, secs: &[phrases::Section]) {
        let cycle = phrases::mix(secs, &mut self.rand);
        self.use_phrases(cycle);
    }

    /// Take on a new phrase set without restarting the session clock, and say so
    /// in the control strip for a couple of seconds.
    pub fn reload(&mut self, secs: &[phrases::Section]) {
        let cycle = self.cycle;
        let elapsed = self.elapsed;
        self.use_sections(secs);
        self.cycle = cycle;
        self.elapsed = elapsed;
        self.notice = Some(("RELOADED".to_string(), 2.5));
        // Bring the control strip up, since the whole point is that you just
        // edited the file in another window.
        self.idle = 0.0;
        self.hud_target = 1.0;
        self.kick();
    }

    // -- derived layout ------------------------------------------------------

    /// Pixel extent of the text block, in terminal cells. Centring uses this,
    /// so it must be the true right edge of the last glyph, not that minus a
    /// word gap (which only exists *between* words).
    pub fn block_size(&self) -> (usize, usize) {
        let mut w = 0;
        let mut rows = 0;
        for g in &self.glyphs {
            w = w.max(g.col + g.w);
            rows = rows.max(g.row + 1);
        }
        (w, rows * self.line_h())
    }

    /// Widest single line, which is what the block is sized against.
    pub fn block_width(&self) -> usize {
        let mut per_row: Vec<usize> = vec![0; self.line_count()];
        for g in &self.glyphs {
            per_row[g.row] = per_row[g.row].max(g.col + g.w);
        }
        per_row.into_iter().max().unwrap_or(0)
    }

    fn line_count(&self) -> usize {
        self.glyphs.iter().map(|g| g.row + 1).max().unwrap_or(0)
    }

    pub fn origin(&self) -> (usize, usize) {
        let bw = self.block_width();
        let bh = self.block_size().1;
        (
            (self.cols as usize).saturating_sub(bw) / 2,
            (self.rows as usize).saturating_sub(bh) / 2,
        )
    }
}

/// Greedy word wrap into at most `max_rows` lines of `max_w` columns.
/// Returns the lines plus whether anything was cut for space.
/// Cells one character occupies in the given font. The pixel font is 5 columns
/// wide with a blank between glyphs; the terminal font is simply one cell.
fn char_w(pixel: bool, c: char) -> usize {
    if pixel {
        font::width(c)
    } else {
        1
    }
}

/// Cells between two words. The pixel font needs air around its 5-wide glyphs;
/// a terminal font brings its own side bearings.
fn gap(pixel: bool) -> usize {
    if pixel {
        3
    } else {
        1
    }
}

/// Total words in a laid-out phrase, used to compare clipped candidates.
fn words(lines: &[Vec<String>]) -> usize {
    lines.iter().map(|l| l.len()).sum()
}

fn flow(text: &str, max_w: usize, max_rows: usize, pixel: bool) -> (Vec<Vec<String>>, bool) {
    let gap = gap(pixel);
    let mut lines: Vec<Vec<String>> = Vec::new();
    let mut line: Vec<String> = Vec::new();
    let mut used = 0usize;
    let total_words = text.split_whitespace().count();

    fn push_line(
        lines: &mut Vec<Vec<String>>,
        line: &mut Vec<String>,
        used: &mut usize,
    ) {
        if !line.is_empty() {
            lines.push(std::mem::take(line));
            *used = 0;
        }
    }

    for word in text.split_whitespace() {
        let w: usize = word.chars().map(|c| char_w(pixel, c)).sum();
        let need = used + if line.is_empty() { 0 } else { gap } + w;

        if !line.is_empty() && need > max_w {
            push_line(&mut lines, &mut line, &mut used);
            if lines.len() >= max_rows {
                return (lines, true);
            }
        }

        // A single word wider than the window gets hard-split by character.
        if w > max_w {
            let mut chunk = String::new();
            let mut cw = 0usize;
            for c in word.chars() {
                let cwid = char_w(pixel, c);
                if cw + cwid > max_w && !chunk.is_empty() {
                    line.push(std::mem::take(&mut chunk));
                    push_line(&mut lines, &mut line, &mut used);
                    if lines.len() >= max_rows {
                        return (lines, true);
                    }
                    cw = 0;
                }
                chunk.push(c);
                cw += cwid;
            }
            if !chunk.is_empty() {
                line.push(chunk);
                used = cw;
            }
            continue;
        }

        if !line.is_empty() {
            used += gap;
        }
        used += w;
        line.push(word.to_string());
    }
    push_line(&mut lines, &mut line, &mut used);

    let shown: usize = lines.iter().map(|l| l.len()).sum();
    (lines, shown < total_words)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eng(cols: u16, rows: u16) -> Engine {
        let mut e = Engine::new(1, cols, rows);
        e.phase = Phase::Typing;
        e
    }

    /// The same engine, but on the 5x7 pixel font.
    fn pixel_eng(cols: u16, rows: u16) -> Engine {
        let mut e = eng(cols, rows);
        e.pixel = true;
        e
    }

    #[test]
    fn canvas_is_double_height_at_big_font() {
        let c = style::Canvas::from_font(&font::A, 2);
        assert_eq!(c.rows.len(), 14);
        assert_eq!(c.w, 5);
        let c = style::Canvas::from_font(&font::A, 1);
        assert_eq!(c.rows.len(), 7);
    }

    #[test]
    fn the_terminal_font_is_the_default() {
        let mut e = eng(100, 30);
        e.use_phrases(vec!["WAKE UP".into()]);
        assert!(!e.pixel, "readability wins by default");
        assert_eq!(
            e.glyphs.iter().map(|g| g.w).max().unwrap(),
            1,
            "one terminal cell per character"
        );
        assert_eq!(e.block_size().1, 1, "one row tall");
        assert!(e.glyphs.iter().all(|g| g.canvas.is_none()));
    }

    #[test]
    fn f_key_switches_to_the_pixel_font_and_back() {
        let mut e = eng(100, 30);
        e.use_phrases(vec!["WAKE UP".into()]);
        assert_eq!(e.block_size().1, 1);
        e.key(b'f');
        assert!(e.pixel);
        assert_eq!(e.block_size().1, 14, "pixel glyphs are 14 cells tall");
        e.key(b'f');
        assert!(!e.pixel);
        assert_eq!(e.block_size().1, 1);
    }

    /// Whichever font is in use, the block has to land in the middle of the
    /// window and never overflow it.
    #[test]
    fn both_fonts_centre_and_fit() {
        for pixel in [false, true] {
            for (c, r) in [(200u16, 60u16), (100, 30), (80, 24), (60, 20), (44, 14), (30, 12)] {
                let mut e = eng(c, r);
                e.pixel = pixel;
                for p in phrases::pool(&mut Rng::new(11)) {
                    e.use_phrases(vec![p.clone()]);
                    let (bw, bh) = e.block_size();
                    let (x, y) = e.origin();
                    let label = format!("{} @ {c}x{r}: {p:?}", if pixel { "pixel" } else { "term" });
                    assert!(bw <= c as usize, "{label}: width {bw}");
                    assert!(bh <= r as usize, "{label}: height {bh}");
                    assert!(x + bw <= c as usize, "{label}: overflows right");
                    assert!(y + bh <= r as usize, "{label}: overflows bottom");
                }
            }
        }
    }

    /// The terminal font caps its measure so a long sentence wraps into a
    /// centred column instead of railing across a wide window.
    #[test]
    fn the_terminal_font_wraps_into_a_measure() {
        let mut e = eng(200, 40);
        e.use_phrases(vec![
            "THE CITY NEVER REALLY SLEEPS AND IT KNOWS YOUR NAME BY NOW".into(),
        ]);
        let rows = e.glyphs.iter().map(|g| g.row + 1).max().unwrap();
        assert!(rows > 1, "a 53-character sentence wraps at 56 columns");
        assert!(e.block_width() <= 56, "measure stays inside its cap");
    }

    #[test]
    fn pixel_font_centres_in_the_window() {
        let mut e = pixel_eng(100, 30);
        e.use_phrases(vec!["WAKE UP".into()]);
        let bw = e.block_width();
        let bh = e.block_size().1;
        assert_eq!(bh, 14, "one row of big glyphs is 14 cells tall");
        let (x, y) = e.origin();
        assert_eq!(x, (100 - bw) / 2, "block is horizontally centred");
        assert_eq!(y, (30 - bh) / 2, "block is vertically centred");
    }

    #[test]
    fn narrow_windows_wrap_and_shrink() {
        let mut e = pixel_eng(40, 12);
        e.use_phrases(vec![
            "THE CITY NEVER REALLY SLEEPS AND IT KNOWS YOUR NAME".into(),
        ]);
        let bw = e.block_width();
        let bh = e.block_size().1;
        assert!(bw <= 40, "block {bw} must fit the window");
        assert!(bh <= 12, "block height {bh} must fit the window");
    }

    #[test]
    fn speed_knob_moves_all_three_timings_together() {
        let mut e = eng(80, 24);
        let (c0, h0, g0) = (e.cps(), e.hold_len(), e.gap_len());
        e.level = 100.0;
        assert!(e.cps() > c0 && e.hold_len() > h0 && e.gap_len() > g0);
        e.level = 0.0;
        assert!(e.cps() < c0 && e.hold_len() < h0 && e.gap_len() < g0);
    }

    #[test]
    fn hud_hides_after_three_idle_seconds() {
        let mut e = eng(80, 24);
        e.idle = 0.0;
        e.hide_hud();
        assert_eq!(e.hud_target, 1.0, "up while you are using it");
        e.idle = HUD_IDLE - 0.5;
        e.hide_hud();
        assert_eq!(e.hud_target, 1.0, "still up just before the deadline");
        e.idle = HUD_IDLE + 0.5;
        e.hide_hud();
        assert_eq!(e.hud_target, 0.0, "gone once you stop typing");
        e.pin = true;
        e.hide_hud();
        assert_eq!(e.hud_target, 1.0, "pinned bar stays");
    }

    #[test]
    fn every_builtin_phrase_fits_a_small_window() {
        for pixel in [false, true] {
            for (cols, rows) in [(60u16, 18u16), (40, 12), (24, 10), (200, 60)] {
                let mut e = eng(cols, rows);
                e.pixel = pixel;
                for p in phrases::pool(&mut Rng::new(7)) {
                    e.use_phrases(vec![p.clone()]);
                    let bw = e.block_width();
                    let bh = e.block_size().1;
                    assert!(bw <= cols as usize, "{p:?}: width {bw} > {cols}");
                    assert!(bh <= rows as usize, "{p:?}: height {bh} > {rows}");
                    let (x, y) = e.origin();
                    assert!(x + bw <= cols as usize, "{p:?}: overflows right edge");
                    assert!(y + bh <= rows as usize, "{p:?}: overflows bottom edge");
                }
            }
        }
    }

    #[test]
    fn overlong_phrases_are_clipped_not_overflowing() {
        for pixel in [false, true] {
            let mut e = eng(30, 10);
            e.pixel = pixel;
            e.use_phrases(vec!["A VERY LONG SENTENCE ".repeat(12)]);
            let bw = e.block_width();
            let bh = e.block_size().1;
            assert!(bw <= 30 && bh <= 10, "clipped to {bw}x{bh}");
        }
    }

    #[test]
    fn text_block_never_eats_the_chrome() {
        for pixel in [false, true] {
            for (c, r) in [(200u16, 60u16), (100, 30), (80, 24), (60, 20), (40, 14), (30, 12)] {
                let mut e = eng(c, r);
                e.pixel = pixel;
                for p in phrases::pool(&mut Rng::new(3)) {
                    e.use_phrases(vec![p.clone()]);
                    let bh = e.block_size().1;
                    let bw = e.block_width();
                    let avail = r as usize - e.chrome_rows();
                    assert!(
                        bh <= avail,
                        "{p:?} at {c}x{r}: text {bh} rows, only {avail} available"
                    );
                    assert!(bw <= c as usize, "{p:?} at {c}x{r}: text {bw} cols");
                }
            }
        }
    }

    #[test]
    fn short_phrases_keep_the_big_font() {
        let mut e = pixel_eng(100, 30);
        e.use_phrases(vec!["WAKE UP".into()]);
        assert_eq!(e.cell_h, 2, "short phrases stay large");
        assert_eq!(e.block_size().1, 14);
    }

    #[test]
    fn long_phrases_use_every_row_available() {
        let long = "EVERY MACHINE DREAMS IN ONES AND ZEROS";
        let mut e = pixel_eng(100, 30);
        e.use_phrases(vec![long.into()]);
        assert_eq!(e.cell_h, 1, "drops a scale to gain rows for a long phrase");
        let rows = (30 - e.chrome_rows()) / 7;
        assert_eq!(e.glyphs.iter().map(|g| g.row + 1).max().unwrap(), rows);
        assert!(e.glyphs.len() >= 30, "shows most of the phrase");
    }

    #[test]
    fn wrapped_lines_are_each_centred() {
        // Narrow enough to force a wrap; every line should sit inside the block
        // with a symmetric margin.
        // 38 columns forces a wrap; 30 rows leaves room for all of it at the
        // big font (2 lines of 14 cells).
        let mut e = pixel_eng(38, 44);
        e.use_phrases(vec!["THE HUM YOU HEAR IS THE WORLD THINKING".into()]);
        let nlines = e.glyphs.iter().map(|g| g.row + 1).max().unwrap();
        assert!(nlines > 1, "this phrase must wrap at 38 columns, got {nlines} line(s)");
        let bw = e.block_width();
        assert_eq!(e.block_size().1, nlines * 7 * e.cell_h);
        for r in 0..nlines {
            let lo = e.glyphs.iter().filter(|g| g.row == r).map(|g| g.col).min().unwrap();
            let hi = e.glyphs.iter().filter(|g| g.row == r).map(|g| g.col + g.w).max().unwrap();
            let left_pad = lo;
            let right_pad = bw - hi;
            assert!(
                left_pad.abs_diff(right_pad) <= 1,
                "line {r}: margins {left_pad}/{right_pad} inside a {bw}-wide block"
            );
        }
    }

    #[test]
    fn flow_wraps_on_word_boundaries() {
        let width_of = |l: &[String]| -> usize {
            l.iter().map(|x| x.chars().map(font::width).sum::<usize>()).sum::<usize>()
                + 3 * l.len().saturating_sub(1)
        };

        // At 40 columns only one word fits per line, so this needs 5 rows and
        // asking for 4 must report a clip rather than silently overflowing.
        const MAX_W: usize = 40;
        let (narrow, cut) = flow("THE CITY NEVER REALLY SLEEPS", MAX_W, 4, true);
        assert!(cut, "reports the clip when rows run out");
        assert_eq!(narrow.len(), 4, "never returns more rows than allowed");
        for l in &narrow {
            assert!(width_of(l) <= MAX_W, "line fits: {l:?}");
        }

        // Widen the window and the same phrase fits without clipping.
        let (wide, cut) = flow("THE CITY NEVER REALLY SLEEPS", 200, 4, true);
        assert!(!cut, "fits on one line when there is room");
        assert_eq!(wide.len(), 1);
        for l in &wide {
            assert!(width_of(l) <= 200, "line fits: {l:?}");
        }
    }

    /// With the terminal font, words are one cell wide, so the same phrase
    /// wraps far later -- the wrap has to agree with that.
    #[test]
    fn flow_measures_the_terminal_font_in_cells() {
        let (lines, cut) = flow("THE CITY NEVER REALLY SLEEPS", 40, 4, false);
        assert!(!cut);
        assert_eq!(lines.len(), 1, "27 characters still fit on one 40-cell row");
        for l in &lines {
            let w: usize = l.iter().map(|x| x.chars().count()).sum::<usize>()
                + l.len().saturating_sub(1);
            assert!(w <= 40, "line fits: {l:?}");
        }
    }

    /// A cell keeps whatever the last frame left in it, so a painter that stops
    /// drawing has to wipe its own rows -- the control bar used to just stop and
    /// leave its last lit frame sitting there.
    #[test]
    fn fading_the_control_bar_wipes_the_rows_it_used() {
        use crate::{term::Out, view};

        for pixel in [false, true] {
            let mut e = eng(80, 24);
            e.pixel = pixel;
            e.hud = 1.0; // the real loop ramps this up over the first few frames
            let mut o = Out::new();
            view::draw(&mut e, &mut o);
            let (bar_row, rows) = e.hud_block.expect("the bar paints on the first frame");

            // Idle past the deadline: the bar is gone, and the frame that takes
            // it down has to repaint what it covered.
            e.idle = HUD_IDLE + 1.0;
            e.hud = 0.0;
            let mut o2 = Out::new();
            view::draw(&mut e, &mut o2);
            assert!(e.hud_block.is_none(), "and it stays off once it has faded");

            let frame = String::from_utf8_lossy(&o2.0);
            for r in [bar_row, bar_row + 1, rows - 2, rows - 1] {
                let wipe = format!("\x1b[{};1H\x1b[48;2;4;7;12m", r + 1);
                assert!(
                    frame.contains(&wipe),
                    "row {r} should be wiped back to the page background"
                );
            }
        }
    }

    /// The bar's rows and the strip's two rows share the bottom of the window,
    /// so they have to land on different lines or the strip paints over the bar.
    #[test]
    fn the_bar_never_lands_on_the_strip() {
        use crate::term::Out;
        for pixel in [false, true] {
            for (c, r) in [(80u16, 24u16), (100, 30), (60, 20), (80, 12)] {
                let mut e = eng(c, r);
                e.pixel = pixel;
                e.hud = 1.0;
                let mut o = Out::new();
                crate::view::draw(&mut e, &mut o);
                if let Some((bar_row, rows)) = e.hud_block {
                    assert!(
                        bar_row + 1 < rows - 2,
                        "bar and ticks clear the strip's rows at {c}x{r}"
                    );
                }
            }
        }
    }

    /// On a window too short to hold the chrome, the bar and strip give way
    /// rather than sitting on top of the words.
    #[test]
    fn a_short_window_drops_the_control_bar_not_the_words() {
        use crate::term::Out;
        // The pixel font needs seven rows for a single line, which leaves no
        // room for four rows of furniture on a nine-row window.
        let mut e = pixel_eng(80, 9);
        e.hud = 1.0;
        let mut o = Out::new();
        crate::view::draw(&mut e, &mut o);
        assert!(e.hud_block.is_none(), "no room for chrome on nine rows");
        assert!(e.block_size().1 <= 9, "the words still fit");
    }

    /// The terminal font only needs one row for a line, so the same nine-row
    /// window *can* hold the chrome -- and when it does, it must not land on
    /// top of the words.
    #[test]
    fn the_terminal_font_frees_room_for_the_chrome() {
        use crate::term::Out;
        let mut e = eng(80, 9);
        e.use_phrases(vec!["EVERYTHING IS FINE".into()]);
        e.hud = 1.0;
        let mut o = Out::new();
        crate::view::draw(&mut e, &mut o);
        let (bar_row, _) = e.hud_block.expect("the bar fits on a nine-row window");
        let (x, y) = e.origin();
        assert!(y + e.block_size().1 <= bar_row, "the words sit above the bar");
        assert!(x + e.block_width() <= 80, "and inside the window");
    }

    /// The cursor advances one cell per keystroke and the terminal never clears,
    /// so the cell it vacates has to be put back or a block character ends up
    /// stranded in the middle of a finished sentence.
    #[test]
    fn the_cursor_never_strands_its_old_cell() {
        use crate::{term::Out, view};

        /// Every run of cells a frame painted, as `(row, first column, width)`.
        /// Both the background-coloured runs and the foreground writes count: a
        /// cell the cursor vacates is either wiped to background or taken over by
        /// the glyph that has just been typed into it.
        fn touched(frame: &str) -> Vec<(usize, usize, usize)> {
            let b = frame.as_bytes();
            let mut out = Vec::new();
            let mut i = 0;
            while i + 2 < b.len() {
                if b[i] != 0x1b || b[i + 1] != b'[' {
                    i += 1;
                    continue;
                }
                let mut j = i + 2;
                while j < b.len() && !(0x40..=0x7e).contains(&b[j]) {
                    j += 1;
                }
                if j >= b.len() {
                    break;
                }
                let final_byte = b[j] as char;
                let params = frame[i + 2..j].to_string();
                i = j + 1;

                if final_byte != 'H' {
                    continue;
                }
                let Some((r, c)) = params
                    .split_once(';')
                    .and_then(|(a, b)| Some((a.parse::<usize>().ok()?, b.parse::<usize>().ok()?)))
                else {
                    continue;
                };
                if r == 0 || c == 0 {
                    continue;
                }
                // Step over any colour changes between the move and the text.
                let mut k = i;
                loop {
                    if k + 2 < b.len() && b[k] == 0x1b && b[k + 1] == b'[' {
                        let mut m = k + 2;
                        while m < b.len() && !(0x40..=0x7e).contains(&b[m]) {
                            m += 1;
                        }
                        if m < b.len() && b[m] as char == 'm' {
                            k = m + 1;
                            continue;
                        }
                    }
                    break;
                }
                let n = frame[k..].chars().take_while(|ch| *ch != '\x1b').count();
                if n > 0 {
                    out.push((r - 1, c - 1, n));
                }
            }
            out
        }

        let mut moves = 0;
        for pixel in [false, true] {
            let mut e = eng(100, 40);
            e.pixel = pixel;
            e.use_phrases(vec!["STAY A LITTLE LONGER".into()]);
            let mut prev = None;

            while e.typed < e.glyphs.len() {
                e.update(1.0 / 60.0);
                let mut o = Out::new();
                view::draw(&mut e, &mut o);

                if let Some((px, py)) = prev {
                    let frame = String::from_utf8_lossy(&o.0);
                    let repainted = touched(&frame)
                        .iter()
                        .any(|&(r, c, n)| r == py && c <= px && px < c + n);
                    assert!(
                        repainted,
                        "pixel={pixel}: cursor left ({px},{py}) lit, nothing repainted it"
                    );
                    moves += 1;
                }
                prev = e.cursor_at.map(|(x, y, _)| (x, y));
            }
        }
        assert!(moves >= 30, "the cursor really did travel: {moves} moves");
    }

    /// The boot overlay is painted before the text layer has a footprint to
    /// erase, so the first frame of words has to wipe it explicitly -- it used
    /// to leave the progress bar and "INITIALISING" sitting under the words.
    #[test]
    fn the_boot_overlay_is_wiped_when_the_words_take_over() {
        use crate::{term::Out, view};

        let mut e = Engine::new(1, 30, 20);
        e.phase = Phase::Boot;
        e.boot = BOOT_TIME;
        let mut o = Out::new();
        view::draw(&mut e, &mut o);
        let (x, y, _w, _h) = e.overlay.expect("boot records what it painted");

        let mut e2 = e;
        e2.phase = Phase::Typing;
        let mut o2 = Out::new();
        view::draw(&mut e2, &mut o2);
        assert!(e2.overlay.is_none(), "and lets go of it");

        let frame = String::from_utf8_lossy(&o2.0);
        let wipe = format!("\x1b[{};{}H\x1b[48;2;4;7;12m", y + 1, x + 1);
        assert!(frame.contains(&wipe), "the boot band is wiped back to page bg");
        assert!(
            !frame.contains("INITIALISING"),
            "and the words are not drawn over a stranded boot bar"
        );
    }
}



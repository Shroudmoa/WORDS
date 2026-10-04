//! Drawing. One frame = one string of escape codes, flushed in a single write.

use crate::engine::{Engine, Glyph, Mode, Phase};
use crate::style::{self, Rgb};
use crate::term::Out;
use crate::style::{AMBER, BG, CYAN, DIM, FAINT, FG, GREEN, GLITCH_COLORS, MAGENTA};

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn draw(e: &mut Engine, o: &mut Out) {
    let (cols, rows) = (e.cols as usize, e.rows as usize);
    if cols < 20 || rows < 8 {
        e.full = false;
        return;
    }
    o.put("\x1b[?25l");

    if e.phase == Phase::Boot {
        e.overlay = Some(boot(e, o, cols, rows));
        e.full = false;
        return;
    }

    // The boot bar and the help panel are painted outside the footprints the
    // text layer and the control bar track, so whichever of them is gone has
    // to be wiped by hand -- otherwise its pixels sit under the words until
    // something else happens to cover them.
    if let Some((x, y, w, h)) = e.overlay.take() {
        erase_box(o, x, y, w, h);
    }

    text_layer(e, o, cols, rows);
    hud_layer(e, o, cols, rows);
    if e.help {
        e.overlay = Some(help(e, o, cols, rows));
    }

    o.put("\x1b[0m");
    e.full = false;
}

// -- window bezel -----------------------------------------------------------

/// A whisper of a frame around the whole screen, so the words sit inside
/// something. It brightens with the control bar but never fully disappears.
pub fn chrome(e: &Engine, o: &mut Out, cols: usize, rows: usize) {
    if cols < 24 || rows < 8 {
        return;
    }
    let a = 0.30 + 0.70 * e.hud;
    let rule = style::mix(BG, FAINT, a);
    let dim = style::mix(BG, DIM, a);

    // top rule
    style::cells(o, 1, 0, cols - 2, rule);
    style::cells(o, 0, 0, 1, dim);
    style::cells(o, cols - 1, 0, 1, dim);
    // side rails, stopping short of the control strip
    for y in 1..rows.saturating_sub(1) {
        style::cells(o, 0, y, 1, rule);
        style::cells(o, cols - 1, y, 1, rule);
    }
    // corner ticks
    write_text(o, cols, 0, 0, "·", dim);
    write_text(o, cols, cols - 1, 0, "·", dim);

    let name = format!(" words {VERSION} ");
    write_text(o, cols, 3, 0, &name, style::mix(BG, CYAN, a * 0.9));
    let s = e.elapsed as u64;
    let tag = format!(" session {:02}:{:02}:{:02} ", s / 3600, (s / 60) % 60, s % 60);
    write_text(o, cols, cols.saturating_sub(tag.len() + 2), 0, &tag, dim);
}

// -- the words --------------------------------------------------------------

/// Slack around the text block that glitch tears, ghosts and noise can reach.
/// Without it, a burst would leave fragments stranded on screen forever.
const BLEED: usize = 8;

fn text_layer(e: &mut Engine, o: &mut Out, cols: usize, rows: usize) {
    let (x0, y0) = e.origin();
    let bw = e.block_width();
    let bh = e.block_size().1;
    let gh = e.glyph_size();
    let row_h = e.line_h();
    let turbulent = e.glitch > 0.0;
    let caption = e.label_text();

    // The alarm owns the colour of the whole clock -- a five-hertz amber and
    // magenta strobe. It also has to force a repaint, because a terminal holds
    // whatever colour it was last handed and would just keep the first one.
    let alarming = e.mode == Mode::Timer && e.timer.alarming();
    if alarming {
        e.full = true;
    }

    // Any repaint starts by wiping the previous frame's footprint. The wipe is
    // padded, and the frame *after* a burst repaints too, so nothing outlives
    // the glitch that drew it.
    if e.full {
        if let Some((x, y, w, h)) = e.block.take() {
            let x0p = x.saturating_sub(BLEED);
            let y0p = y.saturating_sub(BLEED);
            let w = (w + BLEED * 2).min(cols.saturating_sub(x0p));
            let h = (h + BLEED * 2).min(rows.saturating_sub(y0p));
            if w > 0 && h > 0 {
                erase_box(o, x0p, y0p, w, h);
            }
        }
        for g in &mut e.glyphs {
            g.at = None;
        }
        e.painted_upto = 0;
    }
    retire_cursor(e, o, cols, rows, x0, y0, row_h);

    // Horizontal tears, one value per text row, recomputed every frame.
    let nrows = e.glyphs.iter().map(|g| g.row + 1).max().unwrap_or(0);
    let tears: Vec<isize> = (0..nrows)
        .map(|_| {
            if turbulent && e.rand.chance(0.30) {
                let mag = 1 + e.rand.below(6) as isize;
                if e.rand.chance(0.5) {
                    mag
                } else {
                    -mag
                }
            } else {
                0
            }
        })
        .collect();

    let vis = ease(e.vis);
    let mut base = style::mix(BG, FG, vis);
    if alarming {
        base = if (e.elapsed * 5.0).fract() < 0.5 {
            AMBER
        } else {
            MAGENTA
        };
    }
    let visible = e.typed.min(e.glyphs.len());
    // The digit that just ticked keeps repainting while its flash decays. Its
    // pixels are already down, so recolouring them costs a handful of cells --
    // far less than repainting the clock to change one of them.
    let lit = e.flash_glyph;
    let fading = e.flash > 0.0;

    for i in 0..visible {
        if i < e.painted_upto && !(fading && i == lit) {
            continue;
        }
        let (col, row) = {
            let g = &e.glyphs[i];
            (g.col, g.row)
        };
        let (dx, dy, ghost) = {
            let r = &mut e.rand;
            if turbulent && r.chance(0.22) {
                (
                    r.below(5) as isize - 2,
                    r.below(3) as isize - 1,
                    r.chance(0.45),
                )
            } else {
                (0, 0, false)
            }
        };
        let x = shift(x0 as isize + col as isize + tears[row] + dx);
        let y = shift(y0 as isize + (row * row_h) as isize + dy);

        let color = if turbulent && e.rand.chance(0.25) {
            GLITCH_COLORS[e.rand.below(GLITCH_COLORS.len())]
        } else if fading && i == lit {
            style::mix(base, CYAN, e.flash)
        } else {
            base
        };

        if ghost {
            // Chromatic split: a magenta ghost a couple of cells away.
            let gi = e.rand.below(GLITCH_COLORS.len());
            let (gx, gy) = (shift(x as isize + 2), shift(y as isize - 1));
            paint(&e.glyphs[i], o, cols, gx, gy, GLITCH_COLORS[gi]);
        }
        paint(&e.glyphs[i], o, cols, x, y, color);
        e.glyphs[i].at = Some((x, y));
        e.painted_upto = e.painted_upto.max(i + 1);
    }

    // The caption, inside the block's footprint so the wipe above takes it too.
    if let Some(t) = &caption {
        let lx = x0 + bw.saturating_sub(t.chars().count()) / 2;
        let ly = y0 + gh;
        if ly < rows {
            write_text(o, cols, lx, ly, t, style::mix(BG, CYAN, ease(e.vis)));
        }
    }

    e.block = Some((x0, y0, bw, bh));

    if turbulent {
        noise(e, o, x0, y0, bw, bh);
    }

    // A cursor after the last digit of a clock would only read as a typo.
    if e.mode == Mode::Words {
        cursor(e, o, x0, y0, row_h, cols, rows);
    }
}

/// Paint one glyph. The terminal font puts down the character itself; the pixel
/// font blits its bitmap of background-coloured cells. `cols` clips the
/// character so a glyph jittered to the last column can't wrap and scroll.
fn paint(g: &Glyph, o: &mut Out, cols: usize, x: usize, y: usize, c: Rgb) {
    match &g.canvas {
        Some(canvas) => canvas.blit(o, x, y, c),
        None => write_char(o, cols, x, y, g.ch, c),
    }
}

/// Static-soup characters a burst scatters across the block.
const NOISE: &[char] = &[
    '#', '@', '%', '&', '*', '+', '=', '/', '\\', '<', '>', '[', ']', '{', '}', '~', '^', '$', '|',
    '0', '1', '7', 'x', 'X', '?', '!', '.', ':', ';', '_', '-',
];

fn noise(e: &mut Engine, o: &mut Out, x0: usize, y0: usize, bw: usize, bh: usize) {
    let n = 3 + e.rand.below(9);
    for _ in 0..n {
        let c = GLITCH_COLORS[e.rand.below(GLITCH_COLORS.len())];
        let x = x0 + e.rand.below(bw.max(1));
        let y = y0 + e.rand.below(bh.max(1));
        if e.pixel {
            // Pixel font: the glyphs *are* coloured cells, so lighting a few
            // random ones reads as static.
            let w = 1 + e.rand.below(3);
            style::cells(o, x, y, w, c);
        } else {
            // Terminal font: a run of background-coloured cells would be
            // invisible, so overprint a scramble character instead.
            let ch = NOISE[e.rand.below(NOISE.len())];
            write_char(o, e.cols as usize, x, y, ch, c);
        }
    }
}

/// Which glyph cell a screen cell belongs to, as `(column within the block,
/// row within the block)`. None when the cell is off the block entirely.
fn cell_of(x0: usize, y0: usize, row_h: usize, x: usize, y: usize) -> Option<(usize, usize)> {
    if x < x0 || y < y0 {
        return None;
    }
    Some((x - x0, (y - y0) / row_h.max(1)))
}

/// Wipe the cells the cursor has just vacated.
///
/// The cursor sits immediately past the last glyph typed, so the cell it leaves
/// is either bare ground behind the phrase -- which has to be put back to
/// background, or a stray block sits stranded in the middle of a finished
/// sentence -- or a cell the phrase has just grown into, which the glyph loop
/// below repaints anyway.
fn retire_cursor(
    e: &mut Engine,
    o: &mut Out,
    cols: usize,
    rows: usize,
    x0: usize,
    y0: usize,
    row_h: usize,
) {
    let Some((cx, cy, ch)) = e.cursor_at.take() else {
        return;
    };
    for dy in 0..ch {
        let y = cy + dy;
        if y >= rows || cx >= cols {
            continue;
        }
        let covered = match cell_of(x0, y0, row_h, cx, y) {
            Some((col, row)) => e.glyphs.iter().take(e.typed).any(|g| {
                if g.row != row || col < g.col || col >= g.col + g.w {
                    return false;
                }
                match &g.canvas {
                    // Terminal font: the character fills its one cell.
                    None => true,
                    // Pixel font: a glyph only covers the cells it actually
                    // lights, so its unlit columns still show through.
                    Some(cv) => cv
                        .rows
                        .get(y.saturating_sub(y0).saturating_sub(row * row_h))
                        .and_then(|r| r.get(col - g.col))
                        .copied()
                        .unwrap_or(false),
                }
            }),
            None => false,
        };
        if !covered {
            style::cells(o, cx, y, 1, BG);
        }
    }
}

/// The blinking block cursor, sitting just past the last glyph typed.
fn cursor(e: &mut Engine, o: &mut Out, x0: usize, y0: usize, row_h: usize, cols: usize, rows: usize) {
    let (col, row) = match e.typed.checked_sub(1).map(|i| &e.glyphs[i]) {
        Some(g) => (g.col + g.w, g.row),
        None => (0, 0),
    };
    let x = x0 + col;
    let y = y0 + row * row_h;
    e.cursor_at = None;
    if x >= cols || y >= rows {
        return;
    }

    // 1.4 Hz blink, 65% duty — a terminal cursor, not a nightclub.
    if (e.elapsed * 1.4).fract() >= 0.65 {
        return;
    }

    if row_h == 1 {
        // The terminal font has exactly one row to work with, so the cursor is a
        // solid block character rather than a two-cell stack.
        let c = if e.vis > 0.6 {
            CYAN
        } else {
            style::mix(BG, CYAN, e.vis)
        };
        write_char(o, cols, x, y, '█', c);
        e.cursor_at = Some((x, y, 1));
        return;
    }

    let bob = ((e.elapsed * 2.4).sin() * 0.5 + 0.5) * 0.7 + 0.15;
    let h = ((row_h as f64 * bob).round() as usize).max(2).min(row_h);
    if y + h > rows {
        return;
    }
    style::cells(o, x, y, 1, CYAN);
    style::cells(o, x, y + h - 1, 1, FG);
    e.cursor_at = Some((x, y, h));
}

// -- control bar ------------------------------------------------------------

/// Wipe the four rows the control furniture lives on: the bar, its scale, the
/// strip's wash, and the strip itself.
///
/// The terminal keeps whatever the last frame left in a cell, so a painter that
/// simply stops drawing strands its pixels until something else happens to
/// cover them. The bar also *moves* -- the phrase changes, the window resizes --
/// so we always wipe where we were, not where we are about to be.
fn erase_hud(o: &mut Out, cols: usize, bar_row: usize, rows: usize) {
    for r in [bar_row, bar_row + 1, rows.saturating_sub(2), rows.saturating_sub(1)] {
        if r < rows {
            style::cells(o, 0, r, cols, BG);
        }
    }
}

/// Bar and strip, together, because they share a footprint and a lifetime: the
/// three-second idle fade has to take both down and leave nothing behind.
fn hud_layer(e: &mut Engine, o: &mut Out, cols: usize, rows: usize) {
    if let Some((y, r)) = e.hud_block.take() {
        erase_hud(o, cols, y, r.min(rows));
    }
    if e.hud < 0.04 || e.chrome_rows() < 3 {
        return;
    }
    let y = bar(e, o, cols, rows);
    hud(e, o, cols, rows);
    e.hud_block = Some((y, rows));
}

fn bar(e: &Engine, o: &mut Out, cols: usize, rows: usize) -> usize {
    let a = e.hud;
    let bh = e.block_size().1;
    let (_x0, y0) = e.origin();
    let y = (y0 + bh + 2).min(rows.saturating_sub(4));
    let w = cols.saturating_sub(14).max(8);
    let x = (cols - w) / 2;

    // What the bar is claiming. Words are filling up towards the end of a
    // phrase, a countdown is draining towards zero, and a stopwatch has no end
    // to fill towards at all -- so it says nothing and moves instead.
    let mut sweep = false;
    let frac = if e.mode == Mode::Timer {
        if e.timer.down && e.timer.total > 0.0 {
            (1.0 - e.timer.value / e.timer.total).clamp(0.0, 1.0)
        } else {
            sweep = true;
            0.0
        }
    } else if e.glyphs.is_empty() {
        0.0
    } else {
        e.typed as f64 / e.glyphs.len() as f64
    };

    let cap = style::mix(BG, DIM, a);
    let track = style::mix(BG, FAINT, a);
    let fill_c = style::mix(BG, CYAN, a);
    let head = style::mix(BG, FG, a);
    let tick = style::mix(BG, DIM, a);

    style::cells(o, x - 2, y, 1, cap);
    style::cells(o, x - 1, y, 1, track);
    style::cells(o, x, y, w, track);
    if sweep {
        // A ping-pong head rather than a fill, so a stopwatch never implies it
        // is heading somewhere.
        let t = (e.elapsed * 0.28) % 2.0;
        let p = if t > 1.0 { 2.0 - t } else { t };
        let hx = x + (w as f64 * p).round() as usize;
        if hx < w {
            style::cells(o, hx, y, 1, fill_c);
            style::cells(o, (hx + 1).min(w - 1), y, 1, head);
        }
    } else {
        let fill = ((w as f64 * frac).round() as usize).min(w);
        style::cells(o, x, y, fill, fill_c);
        if fill > 0 {
            style::cells(o, x + fill - 1, y, 1, head);
        }
    }
    style::cells(o, x + w, y, 1, track);
    style::cells(o, x + w + 1, y, 1, cap);

    // Scale ticks, every 10%.
    if y + 2 < rows {
        for i in 0..=10 {
            let tx = x + (w * i / 10).min(w - 1);
            style::cells(o, tx, y + 1, 1, tick);
        }
    }
    y
}

// -- control strip ----------------------------------------------------------

/// A field in the control strip: a dim label and a bright value, as real text.
struct Strip {
    x: usize,
    cols: usize,
}

impl Strip {
    fn label(&mut self, o: &mut Out, y: usize, s: &str, c: Rgb) {
        write_text(o, self.cols, self.x, y, s, c);
        self.x += s.chars().count() + 2;
    }
    fn value(&mut self, o: &mut Out, y: usize, s: &str, c: Rgb) {
        write_text(o, self.cols, self.x, y, s, c);
        self.x += s.chars().count() + 2;
    }
}

fn hud(e: &Engine, o: &mut Out, cols: usize, rows: usize) {
    let a = e.hud;
    let y = rows - 1;
    // Wipe the strip before redrawing: field widths change as the numbers
    // change, and a bare cursor move would leave the old digits behind.
    style::cells(o, 0, y, cols, BG);
    style::cells(o, 0, rows - 2, cols, style::mix(BG, DIM, a * 0.8));

    let lbl = style::mix(BG, DIM, a);
    let val = style::mix(BG, FG, a);
    let key = style::mix(BG, FAINT, a.max(0.3));

    let mut s = Strip { x: 2, cols };
    if e.mode == Mode::Timer {
        strip_timer(e, o, &mut s, y, cols, a, lbl, val);
    } else {
        strip_words(e, o, &mut s, y, cols, a, lbl, val);
    }
    if e.pin {
        s.value(o, y, "pinned", style::mix(BG, AMBER, a));
    }
    if let Some((msg, left)) = &e.notice {
        let fade = (left / 0.6).clamp(0.0, 1.0);
        s.value(o, y, msg, style::mix(BG, GREEN, a * fade));
    }

    // `cycle` counts phrases, so in timer mode it has nothing to say; the
    // direction the clock is running is the more useful thing in that spot.
    let right = if e.mode == Mode::Timer {
        format!("{}  {}", e.timer_label(), clock(e.elapsed))
    } else {
        format!("cycle {:02}  {}", e.cycle, clock(e.elapsed))
    };
    let rx = cols.saturating_sub(right.chars().count() + 2);
    if rx > s.x + 2 {
        write_text(o, cols, rx, y, &right, style::mix(BG, CYAN, a));
        // Key hints fill the gap, but only when there's room to read them.
        let kx = s.x + 2;
        let avail = rx.saturating_sub(kx + 2);
        if avail > 16 {
            let hints = if e.mode == Mode::Timer {
                "space run/pause  c clear  u direction  +/- length  t words  q quit"
            } else {
                "w/s speed  space next  f font  r auto  g glitch  h help  q quit"
            };
            let h: String = hints.chars().take(avail).collect();
            write_text(o, cols, kx, y, h.trim_end(), key);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn strip_words(
    e: &Engine,
    o: &mut Out,
    s: &mut Strip,
    y: usize,
    _cols: usize,
    a: f64,
    lbl: Rgb,
    val: Rgb,
) {
    let hot = style::mix(BG, AMBER, a);
    let phase_c = match e.phase {
        Phase::Holding => style::mix(BG, GREEN, a),
        Phase::Gap | Phase::Resting => style::mix(BG, DIM, a),
        _ => style::mix(BG, MAGENTA, a),
    };
    s.value(o, y, e.phase.label(), phase_c);
    s.value(o, y, &format!("{:.1}", e.cps()), val);
    s.label(o, y, "cps", lbl);
    let hold = if e.phase == Phase::Holding { e.hold } else { e.hold_total };
    s.label(o, y, "hold", lbl);
    s.value(o, y, &format!("{hold:.1}s"), val);
    s.label(o, y, "rate", lbl);
    s.value(o, y, &format!("{:.0}%", e.level), val);
    if e.rapid {
        s.value(o, y, "auto", hot);
    }
}

#[allow(clippy::too_many_arguments)]
fn strip_timer(
    e: &Engine,
    o: &mut Out,
    s: &mut Strip,
    y: usize,
    _cols: usize,
    a: f64,
    lbl: Rgb,
    val: Rgb,
) {
    let state_c = match e.phase {
        Phase::Running => style::mix(BG, GREEN, a),
        Phase::Ready | Phase::Paused => style::mix(BG, AMBER, a),
        Phase::Done => style::mix(BG, MAGENTA, a),
        _ => lbl,
    };
    // The number is the point, so it goes first and largest; the state label
    // follows, because it is what you actually act on.
    s.value(o, y, &Engine::hms(e.timer.value), val);
    s.label(o, y, if e.timer.down { "left" } else { "elapsed" }, lbl);
    if e.timer.down {
        s.label(o, y, "set", lbl);
        s.value(o, y, &Engine::hms(e.timer.total), val);
    }
    s.value(o, y, e.phase.label(), state_c);
    s.label(
        o,
        y,
        if e.timer.down { "countdown" } else { "count up" },
        lbl,
    );
}

fn clock(secs: f64) -> String {
    let s = secs as u64;
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

// -- boot -------------------------------------------------------------------

/// Returns the rect it painted, so the next frame can wipe it.
fn boot(e: &Engine, o: &mut Out, cols: usize, rows: usize) -> (usize, usize, usize, usize) {
    let t = (e.boot / crate::engine::BOOT_TIME).clamp(0.0, 1.0);
    let w = (cols as f64 * 0.42).round() as usize;
    let x = (cols - w) / 2;
    let y = rows / 2;

    style::cells(o, x, y, w, style::mix(BG, FAINT, 0.9));
    let fill = (w as f64 * t).round() as usize;
    style::cells(o, x, y, fill, CYAN);
    if fill > 0 {
        style::cells(o, x + fill - 1, y, 2, FG);
    }
    let msg = "INITIALISING";
    let mx = (cols - msg.len()) / 2;
    write_text(o, cols, mx, y + 2, msg, DIM);

    // The bar and the message are both centred, so a full-width band over the
    // three rows they use is correct and simpler than a union of two rects.
    let top = y.min(rows.saturating_sub(1));
    (0, top, cols, 3.min(rows - top))
}

// -- help panel -------------------------------------------------------------

const HELP_SHARED: &[(&str, &str)] = &[
    ("F", "terminal font / pixel font"),
    ("G", "force a glitch"),
    ("B", "pin the control bar"),
    ("H", "close this panel"),
    ("Q", "quit"),
];

const HELP_WORDS: &[(&str, &str)] = &[
    ("W / UP", "faster strokes"),
    ("S / DOWN", "slower strokes"),
    ("SPACE", "skip to next phrase"),
    ("LEFT / RIGHT", "previous / next"),
    ("R", "auto-ramp the speed"),
];

const HELP_TIMER: &[(&str, &str)] = &[
    ("SPACE / P", "run / pause / resume"),
    ("C", "clear back to the start"),
    ("U", "count down / count up"),
    ("+ / -", "lengthen / shorten the run"),
];

/// The rows for the panel, in the order they are drawn: whatever is specific to
/// the current mode first, then the keys that mean the same thing either way.
fn help_rows(e: &Engine) -> Vec<(&'static str, &'static str)> {
    let mut v = Vec::new();
    if e.mode == Mode::Timer {
        v.extend_from_slice(HELP_TIMER);
        v.push(("T", "back to the words"));
    } else {
        v.extend_from_slice(HELP_WORDS);
        v.push(("T", "open the timer"));
    }
    v.extend_from_slice(HELP_SHARED);
    v
}

/// Panel geometry, so the box always fits the longest row on any width.
struct Panel {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    key_col: usize,
}

fn panel_for(rows: &[(&str, &str)], cols: usize, screen_rows: usize) -> Panel {
    let key_w = rows.iter().map(|(k, _)| k.chars().count()).max().unwrap_or(12);
    let val_w = rows.iter().map(|(_, v)| v.chars().count()).max().unwrap_or(20);
    // panel = 1 border + 2 pad + key + 2 gutter + val + 1 border
    let want = 3 + key_w + 2 + val_w + 1;
    let w = want.min(cols.saturating_sub(2));
    let h = (rows.len() + 4).min(screen_rows.saturating_sub(2));
    Panel {
        x: (cols.saturating_sub(w)) / 2,
        y: (screen_rows.saturating_sub(h)) / 2,
        w,
        h,
        key_col: 3,
    }
}

/// Returns the panel rect, so the frame that closes it can wipe it.
fn help(e: &Engine, o: &mut Out, cols: usize, rows: usize) -> (usize, usize, usize, usize) {
    let items = help_rows(e);
    let p = panel_for(&items, cols, rows);
    let a = e.hud.max(0.5);
    let edge = style::mix(BG, CYAN, a);
    let title_c = style::mix(BG, FG, a);
    let keyc = style::mix(BG, FG, a);
    let valc = style::mix(BG, CYAN, a);

    // Solid backdrop so the panel reads over the words behind it.
    for row in 0..p.h {
        style::cells(o, p.x, p.y + row, p.w, BG);
    }
    box_border(o, cols, p.x, p.y, p.w, p.h, edge);

    let title = "CONTROLS";
    write_text(
        o,
        cols,
        p.x + (p.w.saturating_sub(title.chars().count())) / 2,
        p.y + 1,
        title,
        title_c,
    );

    // Draw each row value-first: the value is flush right, so on a window too
    // narrow for the full wording the key is what gives way.
    let key_room = p.w.saturating_sub(p.key_col + 3); // key + gutter + border
    if key_room < 3 {
        return (p.x, p.y, p.w, p.h);
    }
    for (i, (k, v)) in items.iter().enumerate() {
        let ry = p.y + 3 + i;
        if ry >= p.y + p.h - 1 {
            break;
        }
        let k: String = k.chars().take(key_room).collect();
        // The value ends at the border, so it gets whatever the key column
        // doesn't -- the key's last cell sits *inside* `key_col + len`.
        let room = p.w.saturating_sub(p.key_col + k.chars().count() + 3);
        if room < 3 {
            break;
        }
        // Rather than chop a word off mid-syllable, mark it as shortened.
        let full = v.chars().count();
        let v: String = if full > room {
            let mut t: String = v.chars().take(room - 1).collect();
            t.push('\u{2026}');
            t
        } else {
            v.chars().take(room).collect()
        };
        let vx = p.x + p.w - 1 - v.chars().count();
        write_text(o, cols, vx, ry, &v, valc);
        write_text(o, cols, p.x + p.key_col, ry, &k, keyc);
    }
    (p.x, p.y, p.w, p.h)
}

fn box_border(o: &mut Out, cols: usize, x: usize, y: usize, w: usize, h: usize, c: Rgb) {
    let tl: &str = "\u{250c}";
    let tr: &str = "\u{2510}";
    let bl: &str = "\u{2514}";
    let br: &str = "\u{2518}";
    let hz: &str = "\u{2500}";
    let vt: &str = "\u{2502}";
    write_text(o, cols, x, y, &format!("{tl}{}{tr}", hz.repeat(w - 2)), c);
    write_text(o, cols, x, y + h - 1, &format!("{bl}{}{br}", hz.repeat(w - 2)), c);
    for i in 1..h - 1 {
        write_text(o, cols, x, y + i, vt, c);
        write_text(o, cols, x + w - 1, y + i, vt, c);
    }
}

// -- primitives -------------------------------------------------------------

/// Real foreground text. SGR only, no background, so it composites over
/// whatever is already on screen.
///
/// The clip matters: a run that reaches the last column wraps to the next line
/// and scrolls the screen, which on a narrow window would shove the whole
/// layout up a row every frame.
fn write_text(o: &mut Out, cols: usize, x: usize, y: usize, s: &str, c: Rgb) {
    if x >= cols {
        return;
    }
    let s: String = s.chars().take(cols - x).collect();
    o.put(&format!(
        "\x1b[{};{}H\x1b[38;2;{};{};{}m{}\x1b[0m",
        y + 1,
        x + 1,
        c[0],
        c[1],
        c[2],
        s
    ));
}

/// A single foreground character, at the window clip so a glyph jittered to the
/// last column can't wrap and scroll the screen.
fn write_char(o: &mut Out, cols: usize, x: usize, y: usize, ch: char, c: Rgb) {
    if x >= cols {
        return;
    }
    o.put(&format!(
        "\x1b[{};{}H\x1b[38;2;{};{};{}m{}\x1b[0m",
        y + 1,
        x + 1,
        c[0],
        c[1],
        c[2],
        ch
    ));
}

fn erase_box(o: &mut Out, x: usize, y: usize, w: usize, h: usize) {
    for i in 0..h {
        style::cells(o, x, y + i, w, BG);
    }
}

fn shift(v: isize) -> usize {
    v.max(0) as usize
}

fn ease(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

//! Drawing. One frame = one string of escape codes, flushed in a single write.

use crate::engine::{Engine, Glyph, MenuKind, Mode, Phase};
use crate::style::{self, Rgb};
use crate::term::Out;
use crate::time::Wall;
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

    // A panel that is staying up is opaque and repaints its own rect, so the
    // wipe is only for the frame a panel goes away. Wiping a panel that is
    // about to be redrawn is what let a terminal catch the gap between the two
    // and blink the whole thing off, sixty times a second.
    let keep_out = overlay_rect(e, cols, rows);
    if keep_out.is_none() {
        if let Some((x, y, w, h)) = e.overlay.take() {
            erase_box(o, x, y, w, h);
        }
    }

    let workspace = matches!(e.mode, Mode::Todo | Mode::Plan);
    if workspace {
        let r = workspace_layer(e, o, cols, rows);
        e.workspace = Some(r);
    } else {
        // Leaving a workspace has to take its panel with it: the words only
        // wipe their own footprint, and the panel is bigger than that.
        if let Some((x, y, w, h)) = e.workspace.take() {
            erase_box(o, x, y, w, h);
        }
        text_layer(e, o, cols, rows, keep_out);
    }
    hud_layer(e, o, cols, rows, keep_out);
    // The picker, the form and the help panel share the one overlay slot;
    // only one of them can be up at a time. The form wins over the picker
    // (it is opened from it), and the picker over the help panel.
    if e.form.is_some() {
        e.overlay = Some(form(e, o, cols, rows));
    } else if e.plan_form.is_some() {
        e.overlay = Some(plan_form(e, o, cols, rows));
    } else if e.menu_open {
        e.overlay = Some(menu(e, o, cols, rows));
    } else if e.help {
        e.overlay = Some(help(e, o, cols, rows));
    } else {
        e.overlay = None;
    }

    o.put("\x1b[0m");
    e.full = false;
}

/// The rect the coming overlay will occupy, or `None` when nothing is up. The
/// three panels do not know their own geometry until they paint, so the bounds
/// are worked out here the same way they are: centred, with a fixed or
/// list-sized box. A rect a row or two wide is harmless -- it only keeps the
/// layers clear of the panel.
fn overlay_rect(e: &Engine, cols: usize, rows: usize) -> Option<(usize, usize, usize, usize)> {
    if !e.overlay_up() {
        return None;
    }
    // Generous on purpose: an under-estimate would let a tear or the strip
    // paint across the panel, while an over-estimate only leaves a little of
    // the words frozen for as long as the panel is up.
    let w = cols.saturating_sub(2);
    let h = rows.saturating_sub(2);
    let x = (cols.saturating_sub(w)) / 2;
    let y = (rows.saturating_sub(h)) / 2;
    Some((x, y, w, h))
}

/// Whether a row falls inside the rect an overlay is about to paint.
fn masked(r: Option<(usize, usize, usize, usize)>, y: usize) -> bool {
    matches!(r, Some((_, oy, _, oh)) if y >= oy && y < oy + oh)
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

fn text_layer(
    e: &mut Engine,
    o: &mut Out,
    cols: usize,
    rows: usize,
    keep_out: Option<(usize, usize, usize, usize)>,
) {
    let (x0, y0) = e.origin();
    let bw = e.block_width();
    let bh = e.block_size().1;
    let gh = e.glyph_size();
    let row_h = e.line_h();
    // A panel in front of the words freezes them: no tears, no static, no
    // ghosting. The typewriter keeps its place underneath and picks up where it
    // was the moment the panel goes -- but nothing moves while it is being read.
    let turbulent = e.glitch > 0.0 && keep_out.is_none();
    let caption = e.label_text();

    // The alarm owns the colour of the whole clock -- a five-hertz amber and
    // magenta strobe. It also has to force a repaint, because a terminal holds
    // whatever colour it was last handed and would just keep the first one.
    let alarming = e.alarming();
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

    // Under a panel the text is left exactly as it was: not repainted, not
    // torn, not glitched. The panel covers the middle of the screen where the
    // block sits, so what a viewer loses is pixels they could not see anyway --
    // and what they gain is a panel that holds still.
    if keep_out.is_none() {
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
    } else {
        e.full = true;
    }

    // The caption, inside the block's footprint so the wipe above takes it too.
    if let Some(t) = &caption {
        let lx = x0 + bw.saturating_sub(t.chars().count()) / 2;
        let ly = y0 + gh;
        if ly < rows && !masked(keep_out, ly) {
            write_text(o, cols, lx, ly, t, style::mix(BG, CYAN, ease(e.vis)));
        }
    }

    e.block = Some((x0, y0, bw, bh));

    if turbulent {
        noise(e, o, x0, y0, bw, bh);
    }

    // A cursor after the last digit of a clock would only read as a typo, and a
    // cursor blinking through a panel is worse.
    if e.mode == Mode::Words && keep_out.is_none() {
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
fn hud_layer(
    e: &mut Engine,
    o: &mut Out,
    cols: usize,
    rows: usize,
    keep_out: Option<(usize, usize, usize, usize)>,
) {
    if let Some((y, r)) = e.hud_block.take() {
        erase_hud(o, cols, y, r.min(rows));
    }
    // A panel holds the strip still too: it is another thing that would be
    // repainted under the panel and then painted over, and the clock in it only
    // moves once a second anyway.
    if keep_out.is_some() || e.hud < 0.04 || e.chrome_rows() < 3 {
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
    let frac = match e.mode {
        Mode::Timer => {
            if e.timer.down && e.timer.total > 0.0 {
                (1.0 - e.timer.value / e.timer.total).clamp(0.0, 1.0)
            } else {
                sweep = true;
                0.0
            }
        }
        // A wall clock has no start and no finish, and a deadline has a
        // finish but no start anyone chose -- so both get the moving head
        // rather than a fill that would imply they were heading somewhere.
        Mode::Clock | Mode::Deadline | Mode::Todo | Mode::Plan => {
            sweep = true;
            0.0
        }
        Mode::Words if e.glyphs.is_empty() => 0.0,
        Mode::Words => e.typed as f64 / e.glyphs.len() as f64,
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
    match e.mode {
        Mode::Timer => strip_timer(e, o, &mut s, y, cols, a, lbl, val),
        Mode::Clock => strip_clock(e, o, &mut s, y, cols, a, lbl, val),
        Mode::Deadline => strip_deadline(e, o, &mut s, y, cols, a, lbl, val),
        Mode::Words => strip_words(e, o, &mut s, y, cols, a, lbl, val),
        Mode::Todo | Mode::Plan => {}
    }
    if e.pin {
        s.value(o, y, "pinned", style::mix(BG, AMBER, a));
    }
    if let Some((msg, left)) = &e.notice {
        let fade = (left / 0.6).clamp(0.0, 1.0);
        s.value(o, y, msg, style::mix(BG, GREEN, a * fade));
    }

    // `cycle` counts phrases, so on a clock it has nothing to say; the direction
    // the counter is running, or the mode itself, is the useful thing there.
    let right = match e.mode {
        Mode::Timer => format!("{}  {}", e.timer_label(), clock(e.elapsed)),
        Mode::Deadline => format!("{}  {}", e.mode_name(), clock(e.elapsed)),
        Mode::Clock => format!("{}  {}", e.mode_name(), clock(e.elapsed)),
        Mode::Todo | Mode::Plan => format!("{}  {}", e.mode_name(), clock(e.elapsed)),
        Mode::Words => format!("cycle {:02}  {}", e.cycle, clock(e.elapsed)),
    };
    let rx = cols.saturating_sub(right.chars().count() + 2);
    if rx > s.x + 2 {
        write_text(o, cols, rx, y, &right, style::mix(BG, CYAN, a));
        // Key hints fill the gap, but only when there's room to read them.
        let kx = s.x + 2;
        let avail = rx.saturating_sub(kx + 2);
        if avail > 16 {
            let hints = match e.mode {
                Mode::Timer => {
                    "space run/pause  c clear  u direction  +/- length  t mode  m menu  q quit"
                }
                Mode::Clock | Mode::Deadline => {
                    "t mode  m menu  f font  g glitch  b pin  h help  q quit"
                }
                Mode::Todo => {
                    "up/down move  space tick  a add  x delete  c clear  t mode  m menu  q quit"
                }
                Mode::Plan => {
                    "left/right day  a add  e edit  x delete  d today  t mode  m menu  q quit"
                }
                Mode::Words => {
                    "w/s speed  space next  f font  r auto  g glitch  m menu  h help  q quit"
                }
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

#[allow(clippy::too_many_arguments)]
fn strip_clock(
    e: &Engine,
    o: &mut Out,
    s: &mut Strip,
    y: usize,
    _cols: usize,
    _a: f64,
    lbl: Rgb,
    val: Rgb,
) {
    // The time is the point; the date follows because a wall clock without one
    // leaves you guessing which day the numbers belong to. The year gets the
    // same treatment twice over, because it is the deadline everyone has: how
    // much of it has gone, and how long the rest is.
    s.value(o, y, &e.wall.hms(), val);
    s.label(o, y, "now", lbl);
    s.value(o, y, &e.wall.date(), val);
    // The year is the deadline everyone has, so it gets the countdown the
    // rest of the strip gives a deadline by name. (The share of the year
    // gone is already under the digits, on the caption row.)
    let (_pct, left) = crate::time::year_stats(e.local);
    s.value(o, y, &Engine::countdown(left as f64), val);
    s.label(o, y, "to new year", lbl);
}

#[allow(clippy::too_many_arguments)]
fn strip_deadline(
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
        Phase::Done => style::mix(BG, MAGENTA, a),
        _ => lbl,
    };
    // The seconds are the point; the name and the moment follow, because a
    // number alone does not say what it is that is nearly upon you.
    s.value(o, y, &Engine::countdown(e.left), val);
    s.label(o, y, "left", lbl);
    if let Some(t) = &e.target {
        s.value(o, y, &t.name, val);
        s.label(o, y, &t.stamp(), lbl);
    }
    s.value(o, y, e.phase.label(), state_c);
}

fn clock(secs: f64) -> String {
    let s = secs as u64;
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

// -- workspaces: the todo list and the planner ------------------------------

/// The two workspaces paint the whole inside of the bezel rather than the
/// middle of the screen. They share one rect so the frame that leaves one can
/// wipe it clean.
fn workspace_rect(cols: usize, rows: usize) -> (usize, usize, usize, usize) {
    (1, 1, cols.saturating_sub(2), rows.saturating_sub(2))
}

fn workspace_layer(e: &Engine, o: &mut Out, cols: usize, rows: usize) -> (usize, usize, usize, usize) {
    let (x, y, w, h) = workspace_rect(cols, rows);
    let a = e.hud.max(0.5);
    let edge = style::mix(BG, CYAN, a);

    // Opaque: the panel owns its rect for as long as it is up.
    for row in 0..h {
        style::cells(o, x, y + row, w, BG);
    }
    let (bx, by, bw, bh) = (x + 1, y, w.saturating_sub(2), h);
    if bw < 4 || bh < 5 {
        return (x, y, w, h);
    }
    box_border(o, cols, bx, by, bw, bh, edge);
    match e.mode {
        Mode::Todo => todo_screen(e, o, cols, bx, by, bw, bh, a),
        Mode::Plan => plan_screen(e, o, cols, bx, by, bw, bh, a),
        _ => {}
    }
    (x, y, w, h)
}

fn todo_screen(
    e: &Engine,
    o: &mut Out,
    cols: usize,
    bx: usize,
    by: usize,
    bw: usize,
    bh: usize,
    a: f64,
) {
    let inner_x = bx + 2;
    let inner_w = bw.saturating_sub(4);
    let top = by + 3;
    let hint_y = by + bh - 2;
    let list_rows = hint_y.saturating_sub(top).max(1);
    let a = a.max(0.5);
    let dimc = style::mix(BG, DIM, a);
    let itemc = style::mix(BG, FG, a);
    let selc = style::mix(BG, CYAN, a);
    let selbg = style::mix(BG, CYAN, a * 0.22);
    let donec = style::mix(BG, DIM, a);
    let tickc = style::mix(BG, GREEN, a);
    let title_c = style::mix(BG, FG, a);

    let open = e.todos.iter().filter(|t| !t.done).count();
    let title = format!("TODO · {open} OPEN / {} TOTAL", e.todos.len());
    write_text(
        o,
        cols,
        bx + (bw.saturating_sub(title.chars().count())) / 2,
        by + 1,
        &title,
        title_c,
    );

    // The inline new-task line takes the last content row while it is open.
    let input_rows = usize::from(e.todo_input.is_some());
    let rows = list_rows.saturating_sub(input_rows).max(1);

    if e.todos.is_empty() {
        write_text(
            o,
            cols,
            inner_x,
            top,
            &truncate("NOTHING TO DO · PRESS A TO ADD ONE", inner_w),
            dimc,
        );
    }

    let first = if e.todo_sel >= rows {
        e.todo_sel + 1 - rows
    } else {
        0
    };
    for (vi, i) in (first..e.todos.len()).take(rows).enumerate() {
        let t = &e.todos[i];
        let ry = top + vi;
        let sel = i == e.todo_sel;
        if sel {
            style::cells(o, bx + 1, ry, bw.saturating_sub(2), selbg);
        }
        let mark = if sel { ">" } else { " " };
        let boxs = if t.done { "[x]" } else { "[ ]" };
        let text = truncate(&t.text, inner_w.saturating_sub(6));
        write_text(o, cols, inner_x, ry, mark, if sel { selc } else { dimc });
        write_text(
            o,
            cols,
            inner_x + 2,
            ry,
            boxs,
            if t.done { tickc } else { selc },
        );
        write_text(
            o,
            cols,
            inner_x + 6,
            ry,
            &text,
            if sel {
                selc
            } else if t.done {
                donec
            } else {
                itemc
            },
        );
    }

    if let Some(input) = &e.todo_input {
        let ry = top + rows;
        if ry < hint_y {
            // The session clock is parked in a workspace, so the caret blinks
            // on the idle clock instead.
            let caret = (e.idle * 2.0).fract() < 0.55;
            let shown = truncate(input, inner_w.saturating_sub(10));
            write_text(o, cols, inner_x, ry, "+ [", selc);
            write_text(o, cols, inner_x + 3, ry, &shown, itemc);
            if caret {
                write_text(o, cols, inner_x + 3 + shown.chars().count(), ry, "▌", selc);
            }
            write_text(
                o,
                cols,
                inner_x + 3 + shown.chars().count(),
                ry,
                "]",
                selc,
            );
        }
    }

    let hint = if e.todo_input.is_some() {
        "type a task   enter save   esc cancel"
    } else {
        "up/down move   space tick   a add   x delete   c clear done   t mode   m menu"
    };
    write_text(o, cols, bx + 2, hint_y, &truncate(hint, bw.saturating_sub(4)), dimc);
}

fn plan_screen(
    e: &Engine,
    o: &mut Out,
    cols: usize,
    bx: usize,
    by: usize,
    bw: usize,
    bh: usize,
    a: f64,
) {
    let inner_x = bx + 2;
    let inner_w = bw.saturating_sub(4);
    let top = by + 3;
    let hint_y = by + bh - 2;
    let a = a.max(0.5);
    let dimc = style::mix(BG, DIM, a);
    let itemc = style::mix(BG, FG, a);
    let selc = style::mix(BG, CYAN, a);
    let selbg = style::mix(BG, CYAN, a * 0.22);
    let title_c = style::mix(BG, FG, a);
    let bar = style::mix(BG, CYAN, a);

    let day = if e.plan_day == i64::MIN {
        e.today()
    } else {
        e.plan_day
    };
    let w = Wall::from_local(day * 86_400);
    let today = day == e.today();
    let title = format!(
        "PLANNER · {} {:04}-{:02}-{:02}{}",
        weekday(&w),
        w.year,
        w.month,
        w.day,
        if today { " · TODAY" } else { "" }
    );
    write_text(
        o,
        cols,
        bx + (bw.saturating_sub(title.chars().count())) / 2,
        by + 1,
        &title,
        title_c,
    );

    let indices: Vec<usize> = e
        .plan
        .iter()
        .enumerate()
        .filter(|(_, ev)| ev.day() == day)
        .map(|(i, _)| i)
        .collect();

    // The Gantt layout: a time column, a name column, then the day from 00:00
    // to 24:00. On a narrow window the name column gives way first, then the
    // bar shrinks to whatever is left.
    let time_w = if inner_w >= 30 { 11 } else { 8 };
    let gap = 2;
    let name_w = inner_w
        .saturating_sub(time_w + gap + gap + 6)
        .min(24);
    let bar_x = inner_x + time_w + gap + if name_w > 0 { name_w + gap } else { 0 };
    let right = bx + bw - 2;
    let bar_w = right.saturating_sub(bar_x);

    // Axis labels every six hours, with the tick at each label's left edge.
    if bar_w >= 8 {
        for h in [0usize, 6, 12, 18, 24] {
            let mut px = bar_x + bar_w * h / 24;
            if h == 24 {
                px = px.saturating_sub(2);
            }
            write_text(o, cols, px, top, &format!("{h:02}"), dimc);
        }
    }

    let events_top = top + 2;
    let rows = hint_y.saturating_sub(events_top);
    if rows > 0 && indices.is_empty() {
        write_text(
            o,
            cols,
            inner_x,
            events_top,
            &truncate("NO PLANS FOR THIS DAY · PRESS A TO ADD ONE", inner_w),
            dimc,
        );
    }

    let first = if e.plan_sel >= rows {
        e.plan_sel + 1 - rows
    } else {
        0
    };
    for (vi, i) in (first..indices.len()).take(rows).enumerate() {
        let ev = &e.plan[indices[i]];
        let ry = events_top + vi;
        let sel = i == e.plan_sel;
        if sel {
            style::cells(o, bx + 1, ry, bw.saturating_sub(2), selbg);
        }
        let span = truncate(&ev.span(), time_w);
        write_text(o, cols, inner_x, ry, &span, if sel { selc } else { dimc });
        if name_w > 0 {
            let name = truncate(&ev.name, name_w);
            write_text(
                o,
                cols,
                inner_x + time_w + gap,
                ry,
                &name,
                if sel { selc } else { itemc },
            );
        }
        if bar_w > 0 {
            let sx = bar_x + (ev.start as usize * bar_w / 1440).min(bar_w - 1);
            let ex = bar_x + ev.end as usize * bar_w / 1440;
            let len = ex
                .saturating_sub(sx)
                .max(1)
                .min(right.saturating_sub(sx));
            style::cells(o, sx, ry, len, if sel { FG } else { bar });
        }
    }

    // A moving marker for the real time, so "now" is visible on the day it is
    // actually now. It rides over the bars rather than hiding behind them.
    if today && bar_w > 0 {
        let now = Wall::from_local(e.local);
        let mins = (now.hour * 60 + now.min) as usize;
        let mx = bar_x + (mins * bar_w / 1440).min(bar_w - 1);
        style::cells(o, mx, top, 1, MAGENTA);
        for vi in 0..rows.min(indices.len().saturating_sub(first)) {
            style::cells(o, mx, events_top + vi, 1, MAGENTA);
        }
    }

    write_text(
        o,
        cols,
        bx + 2,
        hint_y,
        &truncate(
            "left/right day   up/down select   a add   e edit   x delete   d today   t mode   m menu",
            bw.saturating_sub(4),
        ),
        dimc,
    );
}

fn weekday(w: &Wall) -> &'static str {
    const NAMES: [&str; 7] = ["MON", "TUE", "WED", "THU", "FRI", "SAT", "SUN"];
    NAMES[w.weekday % 7]
}

/// Cut a string to `n` characters, marking the cut rather than dropping words
/// silently.
fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    if n == 0 {
        return String::new();
    }
    let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

// -- the add-an-event form --------------------------------------------------

/// The planner's form: a name, a start and an end, one of them being filled
/// in. Returns the panel rect, so the frame that closes it can wipe it.
fn plan_form(e: &Engine, o: &mut Out, cols: usize, rows: usize) -> (usize, usize, usize, usize) {
    let Some(f) = &e.plan_form else {
        return (0, 0, 0, 0);
    };
    let a = e.hud.max(0.5);
    let title = if f.edit.is_some() { "EDIT EVENT" } else { "NEW EVENT" };
    let hint = "type to fill   tab / left right field   enter save   esc cancel";

    let w = (hint.len() + 6).max(title.len() + 6).min(cols.saturating_sub(2));
    let h = 9.min(rows.saturating_sub(2));
    let x = (cols.saturating_sub(w)) / 2;
    let y = (rows.saturating_sub(h)) / 2;

    let edge = style::mix(BG, CYAN, a);
    let title_c = style::mix(BG, FG, a);
    let dimc = style::mix(BG, DIM, a);
    let valc = style::mix(BG, FG, a);
    let selc = style::mix(BG, CYAN, a);
    let errc = style::mix(BG, MAGENTA, a);

    for row in 0..h {
        style::cells(o, x, y + row, w, BG);
    }
    box_border(o, cols, x, y, w, h, edge);
    write_text(
        o,
        cols,
        x + (w.saturating_sub(title.chars().count())) / 2,
        y + 1,
        title,
        title_c,
    );

    // The session clock is parked in a workspace, so the caret blinks on the
    // idle clock instead.
    let caret = (e.idle * 2.0).fract() < 0.55;
    let fields: [(&str, &String); 3] = [
        ("NAME", &f.name),
        ("START", &f.start),
        ("END", &f.end),
    ];
    for (i, (label, buf)) in fields.iter().enumerate() {
        let ry = y + 2 + i;
        let on = i == f.field;
        write_text(o, cols, x + 3, ry, label, if on { selc } else { dimc });
        let bx = x + 10;
        let room = w.saturating_sub(bx - x + 3);
        let shown: String = if buf.chars().count() > room {
            let mut v: Vec<char> = buf.chars().collect();
            v.split_off(v.len() - room).into_iter().collect()
        } else {
            buf.to_string()
        };
        write_text(o, cols, bx, ry, "[", if on { selc } else { dimc });
        write_text(o, cols, bx + 1, ry, &shown, valc);
        write_text(
            o,
            cols,
            bx + 1 + shown.chars().count(),
            ry,
            "]",
            if on { selc } else { dimc },
        );
        if on && caret {
            write_text(o, cols, bx + 1 + shown.chars().count(), ry, "▌", selc);
        }
    }

    match &f.err {
        Some(msg) => write_text(o, cols, x + 2, y + h - 2, msg, errc),
        None => write_text(o, cols, x + 2, y + h - 2, hint, dimc),
    }
    (x, y, w, h)
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
    ("M", "open the menu"),
    ("T", "next mode"),
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

const HELP_TODO: &[(&str, &str)] = &[
    ("UP / DOWN", "move through the list"),
    ("SPACE", "tick / untick the task"),
    ("A", "add a task"),
    ("X", "delete the task"),
    ("C", "clear every finished task"),
];

const HELP_PLAN: &[(&str, &str)] = &[
    ("LEFT / RIGHT", "previous / next day"),
    ("UP / DOWN", "move through the day"),
    ("A", "add an event"),
    ("E", "edit the event"),
    ("X", "delete the event"),
    ("D", "jump back to today"),
];

/// The rows for the panel, in the order they are drawn: whatever is specific to
/// the current mode first, then the keys that mean the same thing either way.
fn help_rows(e: &Engine) -> Vec<(&'static str, &'static str)> {
    let mut v = Vec::new();
    match e.mode {
        Mode::Timer => v.extend_from_slice(HELP_TIMER),
        Mode::Words => v.extend_from_slice(HELP_WORDS),
        Mode::Todo => v.extend_from_slice(HELP_TODO),
        Mode::Plan => v.extend_from_slice(HELP_PLAN),
        // The wall clock and a deadline have no controls of their own: they
        // run because time does.
        Mode::Clock | Mode::Deadline => {}
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

/// The picker: the modes, the deadlines, and any phrase files found under
/// the config directory, in titled sections. Returns the panel rect, so the
/// frame that closes it can wipe it.
fn menu(e: &Engine, o: &mut Out, cols: usize, rows: usize) -> (usize, usize, usize, usize) {
    let items = &e.menu;
    let a = e.hud.max(0.5);
    let title = "SELECT";
    let hint = "up/down move   enter open   x delete   esc close";
    let empty = "nothing here yet";

    let label_w = items
        .iter()
        .map(|it| it.label.chars().count() + if it.kind == MenuKind::Head { 6 } else { 0 })
        .max()
        .unwrap_or(0);
    let content = label_w.max(if items.is_empty() { empty.len() } else { 0 });
    let w = (content + 8)
        .max(title.len() + 6)
        .max(hint.len() + 4)
        .min(cols.saturating_sub(2));
    let h = (items.len() + 6).min(rows.saturating_sub(2));
    let x = (cols.saturating_sub(w)) / 2;
    let y = (rows.saturating_sub(h)) / 2;

    let edge = style::mix(BG, CYAN, a);
    let title_c = style::mix(BG, FG, a);
    let dimc = style::mix(BG, DIM, a);
    let itemc = style::mix(BG, FG, a);
    let selc = style::mix(BG, CYAN, a);
    let selbg = style::mix(BG, CYAN, a * 0.22);

    // Solid backdrop so the rows read over whatever is behind them.
    for row in 0..h {
        style::cells(o, x, y + row, w, BG);
    }
    box_border(o, cols, x, y, w, h, edge);
    write_text(
        o,
        cols,
        x + (w.saturating_sub(title.chars().count())) / 2,
        y + 1,
        title,
        title_c,
    );

    if items.is_empty() {
        write_text(o, cols, x + 2, y + 3, empty, dimc);
    }

    // Show as many rows as the panel holds, scrolled so the selection stays in
    // view when the list is longer than the window.
    let cap = h.saturating_sub(6).max(1);
    let first = if e.menu_sel >= cap { e.menu_sel + 1 - cap } else { 0 };
    let room = w.saturating_sub(7);
    // Number keys open rows directly, so they count what can actually be
    // opened -- a section heading does not take a number.
    let mut numbered = 0;
    for i in 0..first {
        if items[i].kind != MenuKind::Head {
            numbered += 1;
        }
    }
    for (vi, it) in items.iter().skip(first).take(cap).enumerate() {
        let i = first + vi;
        let ry = y + 3 + vi;

        if it.kind == MenuKind::Head {
            let line = format!("── {} ──", it.label);
            write_text(o, cols, x + 2, ry, &line, dimc);
            continue;
        }

        let sel = i == e.menu_sel;
        if sel {
            style::cells(o, x + 1, ry, w - 2, selbg);
        }
        let label: String = if it.label.chars().count() > room {
            let mut t: String = it.label.chars().take(room.saturating_sub(1)).collect();
            t.push('\u{2026}');
            t
        } else {
            it.label.clone()
        };
        let num = if numbered < 9 {
            char::from(b'1' + numbered as u8)
        } else {
            ' '
        };
        let line = format!("{} {} {}", if sel { '>' } else { ' ' }, num, label);
        write_text(o, cols, x + 2, ry, &line, if sel { selc } else { itemc });
        numbered += 1;
    }

    write_text(o, cols, x + 2, y + h - 2, hint, dimc);
    (x, y, w, h)
}

// -- the add-a-deadline form ------------------------------------------------

/// The form: a name, a date and a time, one of them being filled in. Returns
/// the panel rect, so the frame that closes it can wipe it.
fn form(e: &Engine, o: &mut Out, cols: usize, rows: usize) -> (usize, usize, usize, usize) {
    let Some(f) = &e.form else {
        return (0, 0, 0, 0);
    };
    let a = e.hud.max(0.5);
    let title = "NEW DEADLINE";
    let hint = "type to fill   tab / left right field   enter save   esc cancel";

    let w = (hint.len() + 6).max(title.len() + 6).min(cols.saturating_sub(2));
    let h = 9.min(rows.saturating_sub(2));
    let x = (cols.saturating_sub(w)) / 2;
    let y = (rows.saturating_sub(h)) / 2;

    let edge = style::mix(BG, CYAN, a);
    let title_c = style::mix(BG, FG, a);
    let dimc = style::mix(BG, DIM, a);
    let valc = style::mix(BG, FG, a);
    let selc = style::mix(BG, CYAN, a);
    let errc = style::mix(BG, MAGENTA, a);

    for row in 0..h {
        style::cells(o, x, y + row, w, BG);
    }
    box_border(o, cols, x, y, w, h, edge);
    write_text(
        o,
        cols,
        x + (w.saturating_sub(title.chars().count())) / 2,
        y + 1,
        title,
        title_c,
    );

    // A caret that blinks at a speed a person can follow, rather than the
    // screen's own sixty-a-second. It rides the idle clock, which keeps running
    // even when a workspace has parked the session clock.
    let caret = (e.idle * 2.0).fract() < 0.55;
    let fields: [(&str, &String); 3] = [
        ("NAME", &f.name),
        ("DATE", &f.date),
        ("TIME", &f.time),
    ];
    for (i, (label, buf)) in fields.iter().enumerate() {
        let ry = y + 2 + i;
        let on = i == f.field;
        write_text(o, cols, x + 3, ry, label, if on { selc } else { dimc });
        // The box has room for the widest stamp with a little air; a longer
        // name shows its tail, which is what is being edited anyway.
        let bx = x + 10;
        let room = w.saturating_sub(bx - x + 3);
        let shown: String = if buf.chars().count() > room {
            let mut v: Vec<char> = buf.chars().collect();
            v.split_off(v.len() - room).into_iter().collect()
        } else {
            buf.to_string()
        };
        write_text(o, cols, bx, ry, "[", if on { selc } else { dimc });
        write_text(o, cols, bx + 1, ry, &shown, valc);
        write_text(
            o,
            cols,
            bx + 1 + shown.chars().count(),
            ry,
            "]",
            if on { selc } else { dimc },
        );
        if on && caret {
            write_text(o, cols, bx + 1 + shown.chars().count(), ry, "▌", selc);
        }
    }

    // The refusal, once there is one, takes the hint's row: what went wrong
    // matters more than how the form works for as long as it is on screen.
    match &f.err {
        Some(msg) => write_text(o, cols, x + 2, y + h - 2, msg, errc),
        None => write_text(o, cols, x + 2, y + h - 2, hint, dimc),
    }
    (x, y, w, h)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::Event;
    use crate::term::Out;
    use crate::todo::Todo;

    /// One frame, as text, so a test can look for what should be on screen.
    fn frame(e: &mut Engine) -> String {
        let mut o = Out::new();
        draw(e, &mut o);
        String::from_utf8_lossy(&o.0).to_string()
    }

    #[test]
    fn the_todo_workspace_draws_its_list_and_boxes() {
        let mut e = Engine::new(1, 100, 30);
        e.set_now(0, 0);
        e.hud = 1.0;
        e.todos = vec![
            Todo::new("BUY COFFEE"),
            Todo {
                text: "SHIP IT".into(),
                done: true,
            },
        ];
        e.start_todo();

        let f = frame(&mut e);
        assert!(f.contains("TODO"), "the panel has a title");
        assert!(f.contains("1 OPEN / 2 TOTAL"), "and counts the list");
        assert!(f.contains("BUY COFFEE"), "an open task is drawn");
        assert!(f.contains("SHIP IT"), "a finished one too");
        assert!(f.contains("[ ]"), "with an empty box");
        assert!(f.contains("[x]"), "and a ticked one");
        assert!(e.workspace.is_some(), "the workspace records its rect");
    }

    #[test]
    fn the_todo_workspace_draws_the_new_task_line_while_typing() {
        let mut e = Engine::new(1, 100, 30);
        e.hud = 1.0;
        e.start_todo();
        e.key(b'a');
        for c in b"walk the dog" {
            e.key(*c);
        }

        let f = frame(&mut e);
        assert!(f.contains("+ ["), "the inline line is open");
        assert!(f.contains("walk the dog"), "and shows what was typed");
        assert!(f.contains("enter save"), "with its own hint");
    }

    #[test]
    fn the_planner_draws_a_gantt_row_per_event() {
        let mut e = Engine::new(1, 120, 30);
        e.set_now(0, 0); // 1970-01-01
        e.hud = 1.0;
        e.plan = vec![
            Event::new("STANDUP", 1970, 1, 1, 9 * 60, 10 * 60 + 30),
            Event::new("LUNCH", 1970, 1, 1, 12 * 60 + 30, 13 * 60 + 15),
        ];
        e.start_plan();

        let f = frame(&mut e);
        assert!(f.contains("PLANNER"), "the panel has a title");
        assert!(f.contains("1970-01-01"), "and names the day");
        assert!(f.contains("TODAY"), "which is today");
        assert!(f.contains("STANDUP"), "the first event is drawn");
        assert!(f.contains("09:00-10:30"), "with its span");
        assert!(f.contains("LUNCH"), "and so is the second");
        assert!(f.contains("12:30-13:15"), "with its own span");
        assert!(f.contains("00") && f.contains("24"), "the day axis is labelled");
        assert!(e.workspace.is_some(), "the workspace records its rect");
    }

    #[test]
    fn the_planner_draws_the_new_event_form() {
        let mut e = Engine::new(1, 100, 30);
        e.set_now(0, 0);
        e.hud = 1.0;
        e.start_plan();
        e.key(b'a');

        let f = frame(&mut e);
        assert!(f.contains("NEW EVENT"), "the form has a title");
        for label in ["NAME", "START", "END"] {
            assert!(f.contains(label), "the {label} field is drawn");
        }
        assert!(f.contains("enter save"), "and the hint says how to finish");
    }

    /// The session readout used to tick once a second while a workspace was up,
    /// which read as the whole panel glitching. A workspace frame should be
    /// identical from one second to the next.
    #[test]
    fn a_workspace_frame_does_not_change_with_the_clock() {
        let mut e = Engine::new(1, 100, 30);
        e.set_now(0, 0);
        e.hud = 1.0;
        e.todos = vec![Todo::new("BUY COFFEE")];
        e.start_todo();

        let first = frame(&mut e);
        e.update(1.0);
        let second = frame(&mut e);
        assert_eq!(first, second, "nothing on the todo screen ticks over");
    }

    /// The words only wipe their own footprint, so the frame that leaves a
    /// workspace has to take the whole panel down with it.
    #[test]
    fn leaving_a_workspace_wipes_the_panel_it_painted() {
        let mut e = Engine::new(1, 80, 24);
        e.hud = 1.0;
        e.start_todo();
        let _ = frame(&mut e);
        let (x, y, w, h) = e.workspace.expect("the todo panel records its rect");
        assert_eq!((x, y, w, h), (1, 1, 78, 22), "the panel fills the bezel");

        e.start_timer(true, 60.0);
        let f = frame(&mut e);
        assert!(e.workspace.is_none(), "the workspace is no longer up");
        let wipe = format!("\x1b[{};{}H\x1b[48;2;4;7;12m", y + 1, x + 1);
        assert!(
            f.contains(&wipe),
            "the panel's top-left cell is wiped back to the page background"
        );
    }
}

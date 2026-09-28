//! Palette, glitch primitives, and the pixel -> terminal-cell canvas.

use crate::font::{self, Font};
use crate::term::Out;

pub type Rgb = [u8; 3];

// -- palette ----------------------------------------------------------------

pub const BG: Rgb = [0x04, 0x07, 0x0c];
pub const FG: Rgb = [0xcf, 0xf7, 0xff];
pub const CYAN: Rgb = [0x00, 0xe5, 0xff];
pub const MAGENTA: Rgb = [0xff, 0x2e, 0x97];
pub const GREEN: Rgb = [0x2d, 0xff, 0x9c];
pub const AMBER: Rgb = [0xff, 0xb3, 0x3a];
pub const DIM: Rgb = [0x18, 0x2c, 0x38];
pub const FAINT: Rgb = [0x0b, 0x14, 0x1c];

pub const GLITCH_COLORS: [Rgb; 4] = [MAGENTA, CYAN, FG, GREEN];

// -- colour math ------------------------------------------------------------

pub fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    [
        (a[0] as f64 + (b[0] as f64 - a[0] as f64) * t).round() as u8,
        (a[1] as f64 + (b[1] as f64 - a[1] as f64) * t).round() as u8,
        (a[2] as f64 + (b[2] as f64 - a[2] as f64) * t).round() as u8,
    ]
}

/// Erase a run of cells to the background colour.
pub fn cells(out: &mut Out, x: usize, y: usize, n: usize, c: Rgb) {
    if n == 0 {
        return;
    }
    out.put(&format!(
        "\x1b[{};{}H\x1b[48;2;{};{};{}m{}\x1b[0m",
        y + 1,
        x + 1,
        c[0],
        c[1],
        c[2],
        " ".repeat(n)
    ));
}

// -- tiny prng --------------------------------------------------------------

/// xorshift64*. Deterministic, no deps, plenty good enough for glitch noise.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    /// Uniform in [0, 1).
    pub fn f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.f64() * n as f64) as usize % n
        }
    }
    pub fn chance(&mut self, p: f64) -> bool {
        self.f64() < p
    }
}

// -- canvas -----------------------------------------------------------------

/// Wraps a pixel font in vertical cell rows. Because the pixel font is 7 rows
/// tall and terminal cells are ~2:1, each font row becomes 2 cell rows and the
/// glyph looks exactly as tall as it was designed.
pub struct Canvas {
    pub rows: Vec<[bool; font::COLS]>,
    pub w: usize,
}

impl Canvas {
    /// Lay a glyph out into `cell_h` cell rows.
    pub fn from_font(f: &Font, cell_h: usize) -> Canvas {
        let mut rows = vec![[false; font::COLS]; font::ROWS * cell_h];
        for (fy, bits) in f.iter().enumerate() {
            for sub in 0..cell_h {
                let ry = fy * cell_h + sub;
                if ry >= rows.len() {
                    break;
                }
                for (fx, cell) in rows[ry].iter_mut().enumerate() {
                    *cell = (bits >> (font::COLS - 1 - fx)) & 1 == 1;
                }
            }
        }
        Canvas { rows, w: font::COLS }
    }

    /// Push the canvas into the frame buffer.
    ///
    /// A terminal space shows its *background* colour, so a lit pixel is just
    /// "paint this run of cells with the glyph colour and print spaces". We
    /// emit one cursor-move + one SGR per contiguous run, and leave the gaps
    /// alone because the page behind them is already background-coloured.
    pub fn blit(&self, out: &mut Out, x: usize, y: usize, color: Rgb) {
        for (ry, row) in self.rows.iter().enumerate() {
            let mut fx = 0;
            while fx < self.w {
                if !row[fx] {
                    fx += 1;
                    continue;
                }
                let start = fx;
                while fx < self.w && row[fx] {
                    fx += 1;
                }
                cells(out, x + start, y + ry, fx - start, color);
            }
        }
    }
}

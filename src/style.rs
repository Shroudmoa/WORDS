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

/// The three looks the program ships with. A palette is derived from one of
/// these and then optionally overlaid with the user's own colours.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Theme {
    #[default]
    Cyberpunk,
    Amber,
    Matrix,
}

impl Theme {
    pub const ALL: [Theme; 3] = [Theme::Cyberpunk, Theme::Amber, Theme::Matrix];

    pub fn name(self) -> &'static str {
        match self {
            Theme::Cyberpunk => "CYBERPUNK",
            Theme::Amber => "AMBER",
            Theme::Matrix => "MATRIX GREEN",
        }
    }

    /// The value written in the settings file.
    pub fn key(self) -> &'static str {
        match self {
            Theme::Cyberpunk => "cyberpunk",
            Theme::Amber => "amber",
            Theme::Matrix => "matrix",
        }
    }

    pub fn parse(s: &str) -> Option<Theme> {
        match s
            .trim()
            .to_ascii_lowercase()
            .replace([' ', '-', '_'], "")
            .as_str()
        {
            "cyberpunk" | "cyan" | "default" => Some(Theme::Cyberpunk),
            "amber" | "orange" => Some(Theme::Amber),
            "matrix" | "matrixgreen" | "green" => Some(Theme::Matrix),
            _ => None,
        }
    }

    pub fn palette(self) -> Palette {
        match self {
            Theme::Cyberpunk => Palette::new(BG, FG, CYAN, MAGENTA, AMBER, GREEN),
            Theme::Amber => Palette::new(
                [0x0d, 0x08, 0x03],
                [0xff, 0xdf, 0xa8],
                [0xff, 0xb0, 0x00],
                [0xff, 0x77, 0x1a],
                [0xff, 0x4d, 0x2e],
                [0xff, 0xd1, 0x66],
            ),
            Theme::Matrix => Palette::new(
                [0x01, 0x07, 0x03],
                [0xc8, 0xff, 0xd0],
                [0x00, 0xff, 0x70],
                [0x00, 0xb3, 0x4a],
                [0xc8, 0xff, 0x3a],
                [0x7d, 0xff, 0xa0],
            ),
        }
    }
}

/// Every colour the interface paints with. `dim` and `faint` are derived so a
/// custom background or accent carries through the chrome and control bar too.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Palette {
    pub bg: Rgb,
    pub fg: Rgb,
    pub accent: Rgb,
    pub secondary: Rgb,
    pub alert: Rgb,
    pub success: Rgb,
    pub dim: Rgb,
    pub faint: Rgb,
    pub glitch: [Rgb; 4],
}

impl Palette {
    pub fn new(bg: Rgb, fg: Rgb, accent: Rgb, secondary: Rgb, alert: Rgb, success: Rgb) -> Palette {
        let p = Palette {
            bg,
            fg,
            accent,
            secondary,
            alert,
            success,
            dim: bg,
            faint: bg,
            glitch: [secondary, accent, fg, success],
        };
        p.finish()
    }

    /// Recompute the colours that are derived from the main roles. Called after
    /// a custom colour is applied, so the whole interface moves together.
    pub fn finish(mut self) -> Palette {
        self.dim = mix(self.bg, self.accent, 0.13);
        self.faint = mix(self.bg, self.dim, 0.45);
        self.glitch = [self.secondary, self.accent, self.fg, self.success];
        self
    }
}

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
///
/// Horizontal scaling is independent: `x_scale` repeats each source column,
/// so the glyph gets wider without changing its height.
pub struct Canvas {
    pub rows: Vec<Vec<bool>>,
    pub w: usize,
}

impl Canvas {
    /// Lay a glyph out at its original width. Kept for callers that want the
    /// unstretched bitmap; the screen uses [`Canvas::from_font_scaled`].
    #[allow(dead_code)]
    pub fn from_font(f: &Font, cell_h: usize) -> Canvas {
        Canvas::from_font_scaled(f, cell_h, 1)
    }

    /// Lay a glyph out into `cell_h` cell rows and `x_scale` cells per source
    /// column.
    pub fn from_font_scaled(f: &Font, cell_h: usize, x_scale: usize) -> Canvas {
        let x_scale = x_scale.max(1);
        let w = font::COLS * x_scale;
        let mut rows = vec![vec![false; w]; font::ROWS * cell_h];
        for (fy, bits) in f.iter().enumerate() {
            for sub in 0..cell_h {
                let ry = fy * cell_h + sub;
                if ry >= rows.len() {
                    break;
                }
                for fx in 0..font::COLS {
                    let on = (bits >> (font::COLS - 1 - fx)) & 1 == 1;
                    if !on {
                        continue;
                    }
                    for dx in 0..x_scale {
                        rows[ry][fx * x_scale + dx] = true;
                    }
                }
            }
        }
        Canvas { rows, w }
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

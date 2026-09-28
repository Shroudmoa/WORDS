//! A tiny 5x7 pixel font, rendered with terminal *background* colour.
//!
//! Why backgrounds: every cell we want "on" is a space painted with a bg colour,
//! so one cursor-move + one SGR draws a whole scanline. The glyph is stored as
//! 7 rows of 5 bits (high bit of each byte = leftmost column).

/// Glyph cell dimensions, in font pixels.
pub const COLS: usize = 5;
pub const ROWS: usize = 7;

pub type Bitmap = [u8; ROWS];
pub type Font = Bitmap;

const fn b(rows: [u8; ROWS]) -> Bitmap {
    rows
}

pub const BLANK: Font = b([0b00000; ROWS]);

pub const A: Font = b([
    0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
]);
pub const B: Font = b([
    0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
]);
pub const C: Font = b([
    0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110,
]);
pub const D: Font = b([
    0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
]);
pub const E: Font = b([
    0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
]);
pub const F: Font = b([
    0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
]);
pub const G: Font = b([
    0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111,
]);
pub const H: Font = b([
    0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
]);
pub const I: Font = b([
    0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b11111,
]);
pub const J: Font = b([
    0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100,
]);
pub const K: Font = b([
    0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
]);
pub const L: Font = b([
    0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
]);
pub const M: Font = b([
    0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
]);
pub const N: Font = b([
    0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001,
]);
pub const O: Font = b([
    0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
]);
pub const P: Font = b([
    0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
]);
pub const Q: Font = b([
    0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101,
]);
pub const R: Font = b([
    0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
]);
pub const S: Font = b([
    0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
]);
pub const T: Font = b([
    0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
]);
pub const U: Font = b([
    0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
]);
pub const V: Font = b([
    0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100,
]);
pub const W: Font = b([
    0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001,
]);
pub const X: Font = b([
    0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
]);
pub const Y: Font = b([
    0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100,
]);
pub const Z: Font = b([
    0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111,
]);

pub const N0: Font = b([
    0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
]);
pub const N1: Font = b([
    0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
]);
pub const N2: Font = b([
    0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
]);
pub const N3: Font = b([
    0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110,
]);
pub const N4: Font = b([
    0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
]);
pub const N5: Font = b([
    0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110,
]);
pub const N6: Font = b([
    0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
]);
pub const N7: Font = b([
    0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
]);
pub const N8: Font = b([
    0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
]);
pub const N9: Font = b([
    0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100,
]);

pub const DOT: Font = b([
    0, 0, 0, 0, 0, 0b00110, 0b00110,
]);
pub const COMMA: Font = b([
    0, 0, 0, 0, 0, 0b00110, 0b01100,
]);
pub const DASH: Font = b([
    0, 0, 0, 0b11111, 0, 0, 0,
]);
pub const COLON: Font = b([
    0, 0b00110, 0b00110, 0, 0b00110, 0b00110, 0,
]);
pub const SEMI: Font = b([
    0, 0b00110, 0b00110, 0, 0b00110, 0b00110, 0b01100,
]);
pub const EXCL: Font = b([
    0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0, 0b00100,
]);
pub const QUEST: Font = b([
    0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0, 0b00100,
]);
pub const SLASH: Font = b([
    0b00001, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b10000,
]);
pub const BSLASH: Font = b([
    0b10000, 0b10000, 0b01000, 0b00100, 0b00010, 0b00001, 0b00001,
]);
pub const APOS: Font = b([
    0b00100, 0b00100, 0b00000, 0, 0, 0, 0,
]);
pub const QUOTE: Font = b([
    0b01010, 0b01010, 0b00000, 0, 0, 0, 0,
]);
pub const TILDE: Font = b([
    0, 0, 0b01000, 0b10100, 0b00010, 0, 0,
]);
pub const UNDER: Font = b([
    0, 0, 0, 0, 0, 0, 0b11111,
]);
pub const CARET: Font = b([
    0b00100, 0b01010, 0b10001, 0, 0, 0, 0,
]);
pub const STAR: Font = b([
    0, 0b00100, 0b01110, 0b11111, 0b01110, 0b00100, 0,
]);
pub const PIPE: Font = b([
    0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
]);
pub const LT: Font = b([
    0b00010, 0b00100, 0b01000, 0b10000, 0b01000, 0b00100, 0b00010,
]);
pub const GT: Font = b([
    0b01000, 0b00100, 0b00010, 0b00001, 0b00010, 0b00100, 0b01000,
]);
pub const LPAR: Font = b([
    0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010,
]);
pub const RPAR: Font = b([
    0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000,
]);
pub const LBRACE: Font = b([
    0b00011, 0b00100, 0b00100, 0b01000, 0b00100, 0b00100, 0b00011,
]);
pub const RBRACE: Font = b([
    0b11000, 0b00100, 0b00100, 0b00010, 0b00100, 0b00100, 0b11000,
]);
pub const EQ: Font = b([0, 0, 0b11111, 0, 0b11111, 0, 0]);
pub const HASH: Font = b([
    0b01010, 0b01010, 0b11111, 0b01010, 0b11111, 0b01010, 0b01010,
]);

pub fn of(c: char) -> Font {
    match c.to_ascii_lowercase() {
        'a' => A, 'b' => B, 'c' => C, 'd' => D, 'e' => E, 'f' => F, 'g' => G,
        'h' => H, 'i' => I, 'j' => J, 'k' => K, 'l' => L, 'm' => M, 'n' => N,
        'o' => O, 'p' => P, 'q' => Q, 'r' => R, 's' => S, 't' => T, 'u' => U,
        'v' => V, 'w' => W, 'x' => X, 'y' => Y, 'z' => Z,
        '0' => N0, '1' => N1, '2' => N2, '3' => N3, '4' => N4,
        '5' => N5, '6' => N6, '7' => N7, '8' => N8, '9' => N9,
        '.' => DOT, ',' => COMMA, '-' => DASH, ':' => COLON, ';' => SEMI,
        '!' => EXCL, '?' => QUEST, '/' => SLASH, '\\' => BSLASH,
        '\'' => APOS, '"' => QUOTE, '~' => TILDE, '_' => UNDER, '^' => CARET,
        '*' => STAR, '|' => PIPE, '<' => LT, '>' => GT,
        '(' => LPAR, ')' => RPAR, '[' => LBRACE, ']' => RBRACE,
        '=' => EQ, '#' => HASH,
        _ => BLANK,
    }
}

/// Cell width of a character in the pixel font. One blank column separates
/// glyphs; spaces get a little more so words read apart.
pub fn width(c: char) -> usize {
    match c {
        ' ' => 4,
        _ => COLS + 1,
    }
}

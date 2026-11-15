//! Preferences and custom colours, remembered between runs.
//!
//! Everything lives in one small plain-text file in the config directory:
//!
//! ```text
//! theme = cyberpunk
//! font_width = 2
//! pixel_font = false
//! pin = false
//! level = 34
//! bg = #04070c
//! ```
//!
//! A missing file, an unknown key, or a value that does not parse is ignored
//! rather than being a reason to refuse to start. Colours are written as
//! `#RRGGBB`; the same file can be edited by hand.

use std::io;
use std::path::Path;

use crate::style::{Palette, Rgb, Theme};

pub const DEFAULT_FONT_WIDTH: u8 = 2;
pub const MIN_FONT_WIDTH: u8 = 1;
pub const MAX_FONT_WIDTH: u8 = 3;

/// Custom overrides for the main interface roles. `None` means "use whatever
/// the selected theme says".
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct CustomColors {
    pub bg: Option<Rgb>,
    pub fg: Option<Rgb>,
    pub accent: Option<Rgb>,
    pub secondary: Option<Rgb>,
    pub alert: Option<Rgb>,
    pub success: Option<Rgb>,
}

impl CustomColors {
    pub fn any(self) -> bool {
        self.bg.is_some()
            || self.fg.is_some()
            || self.accent.is_some()
            || self.secondary.is_some()
            || self.alert.is_some()
            || self.success.is_some()
    }

    pub fn clear(&mut self) {
        *self = CustomColors::default();
    }
}

/// One of the colours the custom menu can change.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColorRole {
    Background,
    Text,
    Accent,
    Secondary,
    Alert,
    Success,
}

impl ColorRole {
    pub const ALL: [ColorRole; 6] = [
        ColorRole::Background,
        ColorRole::Text,
        ColorRole::Accent,
        ColorRole::Secondary,
        ColorRole::Alert,
        ColorRole::Success,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ColorRole::Background => "BACKGROUND",
            ColorRole::Text => "TEXT",
            ColorRole::Accent => "ACCENT",
            ColorRole::Secondary => "SECONDARY",
            ColorRole::Alert => "ALERT",
            ColorRole::Success => "SUCCESS",
        }
    }

    pub fn get(self, p: &Palette) -> Rgb {
        match self {
            ColorRole::Background => p.bg,
            ColorRole::Text => p.fg,
            ColorRole::Accent => p.accent,
            ColorRole::Secondary => p.secondary,
            ColorRole::Alert => p.alert,
            ColorRole::Success => p.success,
        }
    }

    pub fn is_custom(self, c: &CustomColors) -> bool {
        match self {
            ColorRole::Background => c.bg.is_some(),
            ColorRole::Text => c.fg.is_some(),
            ColorRole::Accent => c.accent.is_some(),
            ColorRole::Secondary => c.secondary.is_some(),
            ColorRole::Alert => c.alert.is_some(),
            ColorRole::Success => c.success.is_some(),
        }
    }

    pub fn set(self, c: &mut CustomColors, value: Option<Rgb>) {
        match self {
            ColorRole::Background => c.bg = value,
            ColorRole::Text => c.fg = value,
            ColorRole::Accent => c.accent = value,
            ColorRole::Secondary => c.secondary = value,
            ColorRole::Alert => c.alert = value,
            ColorRole::Success => c.success = value,
        }
    }
}

/// The little text field the colour menu opens. The input is empty to begin
/// with, so `enter` on an untouched form means "go back to the theme's colour"
/// rather than quietly pinning whatever it happened to be showing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColorForm {
    pub role: ColorRole,
    pub text: String,
    pub err: Option<String>,
}

impl ColorForm {
    pub fn new(role: ColorRole) -> ColorForm {
        ColorForm {
            role,
            text: String::new(),
            err: None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Settings {
    pub theme: Theme,
    pub custom: CustomColors,
    pub font_width: u8,
    pub pixel_font: bool,
    pub pin: bool,
    pub level: f64,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            theme: Theme::Cyberpunk,
            custom: CustomColors::default(),
            font_width: DEFAULT_FONT_WIDTH,
            pixel_font: false,
            pin: false,
            level: 34.0,
        }
    }
}

impl Settings {
    pub fn palette(&self) -> Palette {
        let base = self.theme.palette();
        let c = self.custom;
        Palette::new(
            c.bg.unwrap_or(base.bg),
            c.fg.unwrap_or(base.fg),
            c.accent.unwrap_or(base.accent),
            c.secondary.unwrap_or(base.secondary),
            c.alert.unwrap_or(base.alert),
            c.success.unwrap_or(base.success),
        )
    }

    pub fn sanitized(mut self) -> Settings {
        self.font_width = self.font_width.clamp(MIN_FONT_WIDTH, MAX_FONT_WIDTH);
        self.level = if self.level.is_finite() {
            self.level.clamp(0.0, 100.0)
        } else {
            34.0
        };
        self
    }
}

/// `#RRGGBB`, `RRGGBB`, `R,G,B`, or `R G B`. Values outside 0..=255 fail rather
/// than being clamped, so a typo does not silently become another colour.
pub fn parse_color(text: &str) -> Option<Rgb> {
    let s = text.trim().trim_matches('"').trim();
    if s.is_empty() {
        return None;
    }
    let t = s.strip_prefix('#').unwrap_or(s);
    if !t.contains(',') && !t.contains(' ') {
        if t.len() != 6 || !t.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let r = u8::from_str_radix(&t[0..2], 16).ok()?;
        let g = u8::from_str_radix(&t[2..4], 16).ok()?;
        let b = u8::from_str_radix(&t[4..6], 16).ok()?;
        return Some([r, g, b]);
    }
    let parts: Vec<&str> = t
        .split(|c: char| c == ',' || c.is_ascii_whitespace())
        .filter(|p| !p.is_empty())
        .collect();
    if parts.len() != 3 {
        return None;
    }
    let mut rgb = [0u8; 3];
    for (i, part) in parts.iter().enumerate() {
        rgb[i] = part.parse::<u8>().ok()?;
    }
    Some(rgb)
}

pub fn hex(c: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

fn parse_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

pub fn parse(text: &str) -> Settings {
    let mut s = Settings::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase().replace(['-', ' '], "_");
        let value = value.trim();
        match key.as_str() {
            "theme" => {
                if let Some(t) = Theme::parse(value) {
                    s.theme = t;
                }
            }
            "font_width" => {
                if let Ok(w) = value.parse::<u8>() {
                    s.font_width = w.clamp(MIN_FONT_WIDTH, MAX_FONT_WIDTH);
                }
            }
            "pixel_font" | "pixel" => {
                if let Some(v) = parse_bool(value) {
                    s.pixel_font = v;
                }
            }
            "pin" | "pinned" => {
                if let Some(v) = parse_bool(value) {
                    s.pin = v;
                }
            }
            "level" | "rate" | "speed" => {
                if let Ok(v) = value.parse::<f64>() {
                    if v.is_finite() {
                        s.level = v.clamp(0.0, 100.0);
                    }
                }
            }
            "bg" | "background" => {
                if let Some(c) = parse_color(value) {
                    s.custom.bg = Some(c);
                }
            }
            "fg" | "text" | "foreground" => {
                if let Some(c) = parse_color(value) {
                    s.custom.fg = Some(c);
                }
            }
            "accent" | "cyan" => {
                if let Some(c) = parse_color(value) {
                    s.custom.accent = Some(c);
                }
            }
            "secondary" | "magenta" => {
                if let Some(c) = parse_color(value) {
                    s.custom.secondary = Some(c);
                }
            }
            "alert" | "amber" => {
                if let Some(c) = parse_color(value) {
                    s.custom.alert = Some(c);
                }
            }
            "success" | "green" => {
                if let Some(c) = parse_color(value) {
                    s.custom.success = Some(c);
                }
            }
            _ => {}
        }
    }
    s.sanitized()
}

/// Read the preferences. A file that is not there, or cannot be read, is the
/// defaults -- the first run has to start somewhere.
pub fn load(path: &Path) -> Settings {
    std::fs::read_to_string(path)
        .map(|text| parse(&text))
        .unwrap_or_default()
        .sanitized()
}

pub fn render(s: &Settings) -> String {
    let s = s.sanitized();
    let mut out = String::from("# words settings\n");
    out.push_str(&format!("theme = {}\n", s.theme.key()));
    out.push_str(&format!("font_width = {}\n", s.font_width));
    out.push_str(&format!("pixel_font = {}\n", s.pixel_font));
    out.push_str(&format!("pin = {}\n", s.pin));
    out.push_str(&format!("level = {:.2}\n", s.level));
    let c = s.custom;
    for (name, value) in [
        ("bg", c.bg),
        ("fg", c.fg),
        ("accent", c.accent),
        ("secondary", c.secondary),
        ("alert", c.alert),
        ("success", c.success),
    ] {
        if let Some(v) = value {
            out.push_str(&format!("{name} = {}\n", hex(v)));
        }
    }
    out
}

/// Write the preferences atomically, so a crash halfway through a save cannot
/// leave a truncated settings file behind.
pub fn save(path: &Path, s: &Settings) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, render(s))?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_and_broken_settings_fall_back_gracefully() {
        let d = std::env::temp_dir().join("words-settings-missing");
        let _ = std::fs::remove_dir_all(&d);
        let missing = d.join("settings");
        let s = load(&missing);
        assert_eq!(s, Settings::default());
        assert_eq!(s.font_width, DEFAULT_FONT_WIDTH);
        assert_eq!(s.theme, Theme::Cyberpunk);
    }

    #[test]
    fn parse_reads_the_plain_text_format() {
        let s = parse(
            "# comment\n\
             theme = matrix\n\
             font_width = 9\n\
             pixel_font = yes\n\
             pin = true\n\
             level = 71.5\n\
             bg = #010203\n\
             fg = 250, 240, 230\n",
        );
        assert_eq!(s.theme, Theme::Matrix);
        assert_eq!(s.font_width, MAX_FONT_WIDTH, "clamped, not rejected");
        assert!(s.pixel_font);
        assert!(s.pin);
        assert!((s.level - 71.5).abs() < 1e-9);
        assert_eq!(s.custom.bg, Some([1, 2, 3]));
        assert_eq!(s.custom.fg, Some([250, 240, 230]));
    }

    #[test]
    fn invalid_values_are_ignored_not_fatal() {
        let s = parse(
            "theme = nonsense\n\
             font_width = wide\n\
             pixel_font = perhaps\n\
             level = NaN\n\
             accent = #zzzzzz\n",
        );
        assert_eq!(
            s,
            Settings::default(),
            "everything unusable kept its default"
        );
    }

    #[test]
    fn colours_accept_hex_and_rgb_but_not_out_of_range_values() {
        assert_eq!(parse_color("#00E5FF"), Some([0, 229, 255]));
        assert_eq!(parse_color("00e5ff"), Some([0, 229, 255]));
        assert_eq!(parse_color("0, 229, 255"), Some([0, 229, 255]));
        assert_eq!(parse_color("0 229 255"), Some([0, 229, 255]));
        for bad in ["", "#12345", "#1234567", "256,0,0", "0,0", "red", "0,0,0,0"] {
            assert_eq!(parse_color(bad), None, "{bad:?} should fail");
        }
    }

    #[test]
    fn custom_colours_override_the_theme_and_rederive_dim() {
        let mut s = Settings::default();
        s.custom.bg = Some([10, 20, 30]);
        s.custom.accent = Some([200, 100, 50]);
        let p = s.palette();
        assert_eq!(p.bg, [10, 20, 30]);
        assert_eq!(p.accent, [200, 100, 50]);
        assert_eq!(p.dim, crate::style::mix(p.bg, p.accent, 0.13));
        assert_eq!(p.glitch[1], p.accent);
    }

    #[test]
    fn settings_round_trip_through_the_file() {
        let d = std::env::temp_dir().join("words-settings-roundtrip");
        let _ = std::fs::remove_dir_all(&d);
        let path = d.join("settings");
        let mut s = Settings::default();
        s.theme = Theme::Amber;
        s.font_width = 3;
        s.pixel_font = true;
        s.pin = true;
        s.level = 42.25;
        s.custom.secondary = Some([1, 2, 3]);
        save(&path, &s).expect("writes");
        let back = load(&path);
        assert_eq!(back, s);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn selecting_a_theme_resets_custom_overrides_in_the_engine() {
        // The menu calls this path; the settings type only has to supply the
        // base palette, which is what the engine uses after clearing them.
        let mut s = Settings::default();
        s.custom.bg = Some([1, 1, 1]);
        let p = s.palette();
        assert_eq!(p.bg, [1, 1, 1]);
        s.custom.clear();
        assert_eq!(s.palette().bg, Theme::Cyberpunk.palette().bg);
    }
}

//! Terminal control: raw mode, alternate screen, size queries.
//!
//! Deliberately dependency-free. `stty` does the termios work for us, which
//! means this compiles and runs anywhere POSIX-ish without pulling in libc.

use std::cell::Cell;
use std::fs::File;
use std::io::{self, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How often we re-read the window size. The render loop asks every frame, and
/// `stty size` is a subprocess -- forking 60 times a second starves the loop
/// (frames stretch, the HUD fade crawls). A fifth of a second is far below the
/// eye's notice for a resize.
const SIZE_POLL: Duration = Duration::from_millis(200);

/// Guards raw-mode + alt-screen. Restores the terminal on drop (and on panic,
/// see `install_panic_guard`).
pub struct Term {
    saved_modes: String,
    restored: bool,
    size: Cell<(u16, u16)>,
    size_at: Cell<Instant>,
}

fn stty(args: &[&str]) -> io::Result<String> {
    let tty = File::open("/dev/tty")?;
    let out = Command::new("stty")
        .args(args)
        .stdin(Stdio::from(tty))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

impl Term {
    pub fn enter() -> io::Result<Term> {
        let saved_modes = stty(&["-g"])?;
        // raw: no line buffering, no echo -> we get bytes as they are pressed.
        // `min 0 time 1` keeps reads snappy for arrow-key CSI sequences.
        stty(&["raw", "-echo", "min", "0", "time", "1"])?;

        let mut out = io::stdout();
        // alt screen, hide cursor, no scrollback wrap artifacts
        out.write_all(b"\x1b[?1049h\x1b[?25l\x1b[2J\x1b[H")?;
        out.flush()?;

        let term = Term {
            saved_modes,
            restored: false,
            size: Cell::new((80, 24)),
            size_at: Cell::new(Instant::now()),
        };
        // Seed the cache so the first frame doesn't pay for the query too.
        term.size.set(term.query_size());
        install_panic_guard();
        Ok(term)
    }

    /// Cached `(cols, rows)`. See `SIZE_POLL` for why this isn't per-frame.
    pub fn size(&self) -> (u16, u16) {
        if self.size_at.get().elapsed() < SIZE_POLL {
            return self.size.get();
        }
        self.size_at.set(Instant::now());
        let size = self.query_size();
        self.size.set(size);
        size
    }

    /// (cols, rows) via `stty size`, which reads winsize straight from the tty.
    fn query_size(&self) -> (u16, u16) {
        if let Ok(s) = stty(&["size"]) {
            let mut it = s.split_whitespace();
            if let (Some(r), Some(c)) = (it.next(), it.next()) {
                if let (Ok(r), Ok(c)) = (r.parse::<u16>(), c.parse::<u16>()) {
                    if r >= 4 && c >= 20 {
                        return (c, r);
                    }
                }
            }
        }
        (80, 24)
    }

    pub fn restore(&mut self) {
        if self.restored {
            return;
        }
        self.restored = true;
        let mut out = io::stdout();
        // reset everything we touched, then leave the alt screen
        let _ = out.write_all(b"\x1b[0m\x1b[?25h\x1b[?1049l");
        let _ = out.flush();
        let _ = stty(&[self.saved_modes.as_str()]);
    }
}

impl Drop for Term {
    fn drop(&mut self) {
        self.restore();
    }
}

/// A panic while in raw mode would leave the user's shell unusable, so we
/// restore first, *then* let the panic message print normally.
fn install_panic_guard() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let mut out = io::stdout();
            let _ = out.write_all(b"\x1b[0m\x1b[?25h\x1b[?1049l");
            let _ = out.flush();
            let _ = stty(&["sane"]);
            default_hook(info);
        }));
    });
}

/// Buffered stdout writer. One big syscall per frame instead of dozens.
pub struct Out(pub Vec<u8>);

impl Out {
    pub fn new() -> Self {
        Out(Vec::with_capacity(64 * 1024))
    }
    pub fn clear(&mut self) {
        self.0.clear();
    }
    pub fn put(&mut self, s: &str) {
        self.0.extend_from_slice(s.as_bytes());
    }
    pub fn flush(&mut self) -> io::Result<()> {
        let mut so = io::stdout();
        so.write_all(&self.0)?;
        so.flush()
    }
}

impl Default for Out {
    fn default() -> Self {
        Self::new()
    }
}

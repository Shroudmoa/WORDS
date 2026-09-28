//! words — a classy cyberpunk terminal typewriter.
//!
//! Words fade in, get typed one glyph at a time with a blinking block cursor,
//! hold, then dissolve. Everything chrome sits at the bottom of the screen and
//! fades away once you stop touching the keys.

mod engine;
mod font;
mod phrases;
mod style;
mod term;
mod view;

use std::io::{self, Read};
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const PAGE_BG: style::Rgb = style::BG;
const FRAME: Duration = Duration::from_micros(16_666); // 60fps ceiling

/// Phrase file picked up from the working directory when `--words` is absent.
const DEFAULT_WORDS: &str = "words.txt";

struct Args {
    help: bool,
    list: bool,
    seed: u64,
    words: Option<String>,
}

fn parse() -> Result<Args, String> {
    let mut a = Args { help: false, list: false, seed: 0x5EED_1337, words: None };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => a.help = true,
            "-l" | "--list" => a.list = true,
            "-s" | "--seed" => {
                a.seed = it
                    .next()
                    .ok_or("--seed needs a number")?
                    .parse()
                    .map_err(|_| "bad seed")?;
            }
            "-w" | "--words" => {
                a.words = Some(it.next().ok_or("--words needs a file")?);
            }
            v if v.starts_with('-') => return Err(format!("unknown flag {v}")),
            v => return Err(format!("unexpected argument {v}")),
        }
    }
    Ok(a)
}

fn usage() {
    println!(
        "words {VERSION} — a classy cyberpunk terminal typewriter

  USAGE
    words [options]

  OPTIONS
    -s, --seed <n>      deterministic glitch seed
    -w, --words <file>  load phrases from a file (one per line)
    -l, --list          print the built-in phrases and exit
    -h, --help          this text

  A words.txt in the working directory is used if --words is not given.

  KEYS (while running)
    W / ↑    faster      S / ↓    slower
    SPACE     next phrase         ← →     previous / next
    F         font size           R       auto-ramp speed
    G         force a glitch      B       pin the control bar
    H  ?      controls            Q       quit
"
    );
}

fn main() {
    let args = match parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("words: {e}\n");
            usage();
            std::process::exit(2);
        }
    };

    if args.help {
        usage();
        return;
    }
    if args.list {
        for p in phrases::pool(&mut style::Rng::new(args.seed)) {
            println!("{p}");
        }
        return;
    }

    let mut tty = match term::Term::enter() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("words: can't take over the terminal ({e})");
            std::process::exit(1);
        }
    };

    let (cols, rows) = tty.size();
    let mut e = engine::Engine::new(args.seed, cols, rows);
    // An explicit --words is a hard requirement; a words.txt lying about is just
    // a convenience, so a missing one falls back to the built-in pool silently.
    let given = args.words.clone().or_else(|| {
        Path::new(DEFAULT_WORDS)
            .is_file()
            .then(|| DEFAULT_WORDS.to_string())
    });
    if let Some(path) = &given {
        match load_file(path) {
            Ok(v) => e.use_phrases(v),
            Err(err) => {
                let mut o = term::Out::new();
                o.put("\x1b[0m\x1b[?25h\x1b[?1049l");
                let _ = o.flush();
                tty.restore();
                eprintln!("words: {path}: {err}");
                std::process::exit(1);
            }
        }
    }

    // --- input ---------------------------------------------------------------
    let (tx, rx) = mpsc::channel::<u8>();
    thread::spawn(move || read_keys(tx));

    // --- frame loop ----------------------------------------------------------
    let mut out = term::Out::new();
    out.put(&paint_page());
    let _ = out.flush();

    let mut last = Instant::now();
    'run: loop {
        while let Ok(key) = rx.try_recv() {
            if let engine::Action::Quit = e.key(key) {
                break 'run;
            }
        }

        let now = Instant::now();
        let dt = now.duration_since(last).as_secs_f64().min(0.1);
        last = now;

        e.hide_hud();
        e.update(dt);
        let (c, r) = tty.size();
        e.resize(c, r);

        out.clear();
        out.put("\x1b[H");
        view::chrome(&e, &mut out, c as usize, r as usize);
        view::draw(&mut e, &mut out);
        if out.flush().is_err() {
            break 'run;
        }

        let spent = last.elapsed();
        if spent < FRAME {
            thread::sleep(FRAME - spent);
        }
    }

    tty.restore();
}

/// Fill the page with our own background so the bezel and rules have something
/// to sit on, instead of whatever colour the user's profile used.
fn paint_page() -> String {
    format!(
        "\x1b[48;2;{};{};{}m\x1b[38;2;{};{};{}m\x1b[2J\x1b[H",
        PAGE_BG[0], PAGE_BG[1], PAGE_BG[2], PAGE_BG[0], PAGE_BG[1], PAGE_BG[2]
    )
}

fn load_file(path: &str) -> std::io::Result<Vec<String>> {
    let text = std::fs::read_to_string(path)?;
    let v: Vec<String> = text
        .lines()
        .map(phrases::ascii)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .take(500)
        .collect();
    if v.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "no usable phrases in that file",
        ));
    }
    Ok(v)
}

/// Blocking key reader. Arrow keys arrive as three bytes (`ESC [ A`); they are
/// folded into single sentinel bytes so the game loop only ever sees one byte.
fn read_keys(tx: mpsc::Sender<u8>) {
    let mut stdin = io::stdin();
    let mut b = [0u8; 1];
    loop {
        match stdin.read(&mut b) {
            Ok(0) => {
                // `stty raw min 0 time 1` means this is just "nothing pending"
                // rather than EOF, so keep waiting.
                continue;
            }
            Ok(_) => {}
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return,
        }

        if b[0] != 0x1b {
            if tx.send(b[0]).is_err() {
                return;
            }
            continue;
        }

        let mut n = [0u8; 1];
        if stdin.read(&mut n).is_err() {
            return;
        }
        if n[0] == b'[' || n[0] == b'O' {
            if stdin.read(&mut n).is_err() {
                return;
            }
            let key = match n[0] {
                b'A' => engine::KEY_UP,
                b'B' => engine::KEY_DOWN,
                b'C' => engine::KEY_RIGHT,
                b'D' => engine::KEY_LEFT,
                other => other,
            };
            if tx.send(key).is_err() {
                return;
            }
        } else {
            let _ = tx.send(n[0]);
        }
    }
}

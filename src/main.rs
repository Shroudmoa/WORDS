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

use std::io::{self, IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const PAGE_BG: style::Rgb = style::BG;
const FRAME: Duration = Duration::from_micros(16_666); // 60fps ceiling

/// Phrase file picked up from the working directory when `--words` is absent.
const DEFAULT_WORDS: &str = "words.txt";

/// How often the phrase file is stat-ed for changes.
const WATCH_POLL: Duration = Duration::from_millis(200);

/// How long a phrase file has to stop changing before it is re-read. A save is
/// usually several writes, and someone editing a line in place would
/// otherwise hand us a half-written phrase.
const SETTLE: Duration = Duration::from_millis(400);

/// Cap on phrases read out of any one source, so a runaway file can't lock up
/// startup on a quarter of a million lines.
const MAX_PHRASES: usize = 5000;

struct Args {
    help: bool,
    list: bool,
    seed: u64,
    words: Option<String>,
    lang: Option<String>,
    stdin: bool,
}

fn parse() -> Result<Args, String> {
    let mut a = Args {
        help: false,
        list: false,
        seed: 0x5EED_1337,
        words: None,
        lang: None,
        stdin: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => a.help = true,
            "-l" | "--list" => a.list = true,
            "-i" | "--stdin" => a.stdin = true,
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
            "-g" | "--lang" => {
                a.lang = Some(it.next().ok_or("--lang needs a code")?);
            }
            v if v.starts_with('-') => return Err(format!("unknown flag {v}")),
            v => return Err(format!("unexpected argument {v}")),
        }
    }
    Ok(a)
}

fn usage() {
    let langs = phrases::langs().collect::<Vec<_>>().join(", ");
    println!(
        "words {VERSION} — a classy cyberpunk terminal typewriter

  USAGE
    words [options]

  OPTIONS
    -s, --seed <n>      deterministic glitch seed
    -w, --words <path>  phrases from a file, or from every .txt in a directory
    -g, --lang <code>   start from a bundled language ({langs})
    -i, --stdin         also read phrases from a pipe
    -l, --list          print the built-in phrases and exit
    -h, --help          this text

  PHRASES
    A words.txt in the working directory is used if --words is not given, and
    is re-read as you edit it. Group phrases with '# name' headers:

        # short        (built-in: fragments, short, thoughts)
        WAKE UP
        # thoughts
        THE CITY NEVER REALLY SLEEPS

    Anything supplied by --stdin or --words is added to a --lang set, or used on
    its own in place of the built-in phrases. A single --words file is watched
    for changes while you run.

  KEYS (while running)
    W / ↑    faster      S / ↓    slower
    SPACE     next phrase         ← →     previous / next
    F         font                R       auto-ramp speed
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
        for (name, lines) in phrases::BUILTIN {
            println!("# {name}");
            for l in *lines {
                println!("{l}");
            }
            println!();
        }
        return;
    }

    // Everything is read before the terminal is taken over: an error can then
    // be reported on a normal stderr, and --stdin can't have its keyboard
    // swallowed by raw mode.
    let sources = match resolve(&args) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("words: {e}\nrun `words --help` for the full list of options");
            std::process::exit(2);
        }
    };

    let mut tty = match term::Term::enter() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("words: can't take over the terminal ({e})");
            std::process::exit(1);
        }
    };

    let (cols, rows) = tty.size();
    let mut e = engine::Engine::new(args.seed, cols, rows);
    if !sources.sections.is_empty() {
        e.use_sections(&sources.sections);
    }
    let mut watch = sources.watch.map(Watch::new);

    // --- input ---------------------------------------------------------------
    let (tx, rx) = mpsc::channel::<u8>();
    thread::spawn(move || read_keys(tx));

    // --- frame loop ----------------------------------------------------------
    let mut out = term::Out::new();
    out.put(&paint_page());
    let _ = out.flush();

    let mut last = Instant::now();
    let mut watched_at = Instant::now();
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

        // Stat-ing the phrase file every frame would be sixty syscalls a second
        // to answer a question measured in hundreds of milliseconds.
        if let Some(w) = watch.as_mut() {
            if watched_at.elapsed() >= WATCH_POLL {
                watched_at = Instant::now();
                if let Some(secs) = w.poll() {
                    e.reload(&secs);
                }
            }
        }

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

/// Where the phrases come from, and the one file worth watching for changes.
struct Sources {
    sections: Vec<phrases::Section>,
    watch: Option<PathBuf>,
}

/// Work out the phrase set before taking the terminal.
///
/// The built-in pools are the default, but any custom source replaces them
/// unless a `--lang` set was asked for -- that one is a deliberate starting
/// point, and `--words` on top of it is an addition rather than a replacement.
fn resolve(args: &Args) -> Result<Sources, String> {
    // `--words -` is the conventional spelling for "it's on stdin".
    let (want_stdin, words) = match args.words.as_deref() {
        Some("-") => (true, None),
        other => (args.stdin, other.map(PathBuf::from)),
    };
    let words = words.or_else(|| {
        Path::new(DEFAULT_WORDS).is_file().then(|| PathBuf::from(DEFAULT_WORDS))
    });

    let custom = want_stdin || words.is_some();
    let mut sections = if custom {
        match &args.lang {
            None => Vec::new(),
            Some(code) => {
                let text = phrases::lang(code)
                    .ok_or_else(|| format!("no built-in phrases for language `{code}`"))?;
                phrases::parse(text)
            }
        }
    } else {
        phrases::builtin_sections()
    };

    if want_stdin {
        if io::stdin().is_terminal() {
            return Err("--stdin expects phrases on a pipe".into());
        }
        let mut buf = String::new();
        io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("reading stdin: {e}"))?;
        phrases::merge(&mut sections, phrases::parse(&buf));
    }

    let mut watch = None;
    if let Some(path) = &words {
        let (secs, is_file) = read_source(path)?;
        phrases::merge(&mut sections, secs);
        if is_file {
            watch = Some(path.clone());
        }
    }

    if sections.is_empty() {
        return Err("no usable phrases".into());
    }
    Ok(Sources { sections, watch })
}

/// Read one phrase file, or every `.txt` in a directory. The flag says whether
/// the source was a single file, which is the only thing live reload can
/// follow -- a directory's contents change without the directory's own mtime.
fn read_source(path: &Path) -> Result<(Vec<phrases::Section>, bool), String> {
    if !path.is_dir() {
        return read_file(path).map(|s| (s, true));
    }
    let entries = std::fs::read_dir(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut names: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "txt"))
        .collect();
    names.sort();
    if names.is_empty() {
        return Err(format!("{}: no .txt files in that directory", path.display()));
    }
    let mut out = Vec::new();
    for p in &names {
        phrases::merge(&mut out, read_file(p)?);
    }
    Ok((out, false))
}

fn read_file(path: &Path) -> Result<Vec<phrases::Section>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(phrases::parse(&text))
}

fn stamp(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

/// Debounces a file that is being written: notices its timestamp move, then
/// waits for it to stop moving before letting the caller re-read.
///
/// Split out from the reading so it can be tested against a synthetic clock
/// instead of real sleeps.
struct Settle {
    /// The stamp we last acted on.
    seen: Option<SystemTime>,
    /// When the stamp first looked different.
    changed_at: Option<Instant>,
}

impl Settle {
    fn new(seen: Option<SystemTime>) -> Settle {
        Settle {
            seen,
            changed_at: None,
        }
    }

    /// True once the stamp has moved *and* stayed moved for `SETTLE`. Consumes
    /// the change when it fires, so it fires once per edit.
    fn due(&mut self, stamp: Option<SystemTime>, now: Instant) -> bool {
        if stamp == self.seen {
            self.changed_at = None;
            return false;
        }
        match self.changed_at {
            None => {
                self.changed_at = Some(now);
                false
            }
            Some(at) if now.saturating_duration_since(at) < SETTLE => false,
            Some(_) => {
                self.changed_at = None;
                self.seen = stamp;
                true
            }
        }
    }
}

/// Re-reads a phrase file while the program runs, so edits in another window
/// show up without a restart.
struct Watch {
    path: PathBuf,
    settle: Settle,
}

impl Watch {
    fn new(path: PathBuf) -> Watch {
        Watch {
            settle: Settle::new(stamp(&path)),
            path,
        }
    }

    /// Fresh sections once the file has settled, or `None` to keep going.
    fn poll(&mut self) -> Option<Vec<phrases::Section>> {
        if !self.settle.due(stamp(&self.path), Instant::now()) {
            return None;
        }
        // An edit that leaves nothing usable behind keeps the phrases we have:
        // someone is most likely halfway through typing a line. The stamp has
        // been recorded either way, so the next save is what retries.
        match read_file(&self.path) {
            Ok(secs) if !secs.is_empty() && total(&secs) <= MAX_PHRASES => Some(secs),
            _ => None,
        }
    }
}

fn total(secs: &[phrases::Section]) -> usize {
    secs.iter().map(|(_, v)| v.len()).sum()
}

/// Fill the page with our own background so the bezel and rules have something
/// to sit on, instead of whatever colour the user's profile used.
fn paint_page() -> String {
    format!(
        "\x1b[48;2;{};{};{}m\x1b[38;2;{};{};{}m\x1b[2J\x1b[H",
        PAGE_BG[0], PAGE_BG[1], PAGE_BG[2], PAGE_BG[0], PAGE_BG[1], PAGE_BG[2]
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: Option<&str>, lang: Option<&str>, stdin: bool) -> Args {
        Args {
            help: false,
            list: false,
            seed: 1,
            words: words.map(String::from),
            lang: lang.map(String::from),
            stdin,
        }
    }

    fn flat(secs: &[phrases::Section]) -> Vec<String> {
        secs.iter().flat_map(|(_, v)| v.iter().cloned()).collect()
    }

    /// A scratch directory that cleans up after itself.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let p = std::env::temp_dir().join(format!("words-test-{name}"));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).expect("scratch dir");
            Scratch(p)
        }
        fn write(&self, name: &str, body: &str) -> PathBuf {
            let p = self.0.join(name);
            std::fs::write(&p, body).expect("scratch file");
            p
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    // -- the debounce --------------------------------------------------------

    #[test]
    fn an_untouched_file_never_fires() {
        let t0 = Instant::now();
        let mut s = Settle::new(Some(SystemTime::UNIX_EPOCH));
        for i in 0..50 {
            assert!(
                !s.due(Some(SystemTime::UNIX_EPOCH), t0 + Duration::from_secs(i)),
                "an unchanged file must not reload"
            );
        }
    }

    #[test]
    fn an_edit_fires_once_it_has_settled() {
        let t0 = Instant::now();
        let old = SystemTime::UNIX_EPOCH;
        let new = old + Duration::from_secs(1);
        let mut s = Settle::new(Some(old));

        // Spotted, but not yet: a save in progress must not be read half-way.
        assert!(!s.due(Some(new), t0));
        assert!(!s.due(Some(new), t0 + SETTLE / 2));
        // Quiet long enough.
        assert!(s.due(Some(new), t0 + SETTLE));
        // And exactly once per edit.
        assert!(!s.due(Some(new), t0 + SETTLE * 2), "one edit, one reload");
    }

    #[test]
    fn a_save_written_in_pieces_reloads_once_when_it_lands() {
        let t0 = Instant::now();
        let a = SystemTime::UNIX_EPOCH;
        let b = a + Duration::from_secs(1);
        let c = a + Duration::from_secs(2);
        let mut s = Settle::new(Some(a));

        assert!(!s.due(Some(b), t0));
        // The editor writes again 200ms later, resetting the settle timer.
        assert!(!s.due(Some(c), t0 + Duration::from_millis(200)));
        assert!(!s.due(Some(c), t0 + Duration::from_millis(200) + SETTLE / 2));
        assert!(s.due(Some(c), t0 + Duration::from_millis(200) + SETTLE));
    }

    #[test]
    fn a_file_that_comes_back_counts_as_an_edit() {
        let t0 = Instant::now();
        let mut s = Settle::new(Some(SystemTime::UNIX_EPOCH));
        assert!(!s.due(None, t0), "deleted: noticed");
        assert!(!s.due(None, t0 + SETTLE / 2));
        assert!(
            s.due(None, t0 + SETTLE),
            "and recorded, so recreating it is a fresh edit"
        );
    }

    // -- sources -------------------------------------------------------------

    #[test]
    fn a_words_file_replaces_the_builtin_phrases() {
        let d = Scratch::new("replace");
        let f = d.write("w.txt", "# short\nONLY THIS ONE");
        let s = resolve(&args(Some(f.to_str().unwrap()), None, false)).expect("resolves");
        assert_eq!(flat(&s.sections), ["ONLY THIS ONE"]);
        assert_eq!(s.watch.as_deref(), Some(f.as_path()));
    }

    #[test]
    fn a_directory_of_files_is_merged_in_name_order() {
        let d = Scratch::new("dir");
        d.write("b.txt", "# short\nSECOND");
        d.write("a.txt", "# short\nFIRST");
        d.write("ignored.md", "NOT A PHRASE FILE");
        let s = resolve(&args(Some(d.0.to_str().unwrap()), None, false)).expect("resolves");
        assert_eq!(flat(&s.sections), ["FIRST", "SECOND"]);
        assert!(s.watch.is_none(), "a directory's own mtime says nothing");
    }

    #[test]
    fn an_empty_directory_is_an_error_not_a_blank_screen() {
        let d = Scratch::new("emptydir");
        let err = resolve(&args(Some(d.0.to_str().unwrap()), None, false)).unwrap_err();
        assert!(err.contains("no .txt files"), "{err}");
    }

    #[test]
    fn a_language_is_a_base_that_words_add_to() {
        let d = Scratch::new("lang");
        let f = d.write("w.txt", "# deadpan\nSECTION 4.2: SLACK");
        let s = resolve(&args(Some(f.to_str().unwrap()), Some("de"), false)).expect("resolves");
        let all = flat(&s.sections);
        assert!(all.iter().any(|p| p.contains("WACH AUF")), "German base");
        assert!(all.iter().any(|p| p == "SECTION 4.2: SLACK"), "and ours");
    }

    #[test]
    fn a_language_alone_is_a_complete_phrase_set() {
        let s = resolve(&args(None, Some("fr"), false)).expect("resolves");
        assert!(flat(&s.sections).len() > 30);
        assert!(s.watch.is_none(), "a bundled set cannot change on disk");
    }

    #[test]
    fn an_unknown_language_is_refused_by_name() {
        let err = resolve(&args(None, Some("klingon"), false)).unwrap_err();
        assert!(err.contains("klingon"), "{err}");
    }

    #[test]
    fn a_missing_words_file_is_refused_with_its_path() {
        let err = resolve(&args(Some("/nonexistent/nope.txt"), None, false)).unwrap_err();
        assert!(err.contains("nope.txt"), "{err}");
    }

    #[test]
    fn a_words_file_of_only_comments_has_nothing_to_say() {
        let d = Scratch::new("comments");
        let f = d.write("w.txt", "# short\n## nothing here\n\n");
        let err = resolve(&args(Some(f.to_str().unwrap()), None, false)).unwrap_err();
        assert!(err.contains("no usable phrases"), "{err}");
    }

    /// A mid-edit file must not blank the screen: the phrases on screen have to
    /// outlast an unreadable save.
    #[test]
    fn a_broken_reload_keeps_the_phrases_you_had() {
        let d = Scratch::new("broken");
        let f = d.write("w.txt", "# short\nGOOD");
        let mut w = Watch::new(f.clone());
        let before = std::fs::read_to_string(&f).unwrap();

        std::fs::write(&f, "# short\n").unwrap();
        assert!(
            w.poll().is_none(),
            "a save with nothing in it is not a phrase set"
        );

        std::fs::write(&f, "# short\nGOOD\nAND ONE MORE").unwrap();
        // Give the watcher a settle window to notice the second save.
        std::thread::sleep(SETTLE + WATCH_POLL * 3);
        let secs = w.poll().expect("the fixed file loads");
        assert_eq!(flat(&secs), ["GOOD", "AND ONE MORE"]);
        assert_ne!(before, "");
    }
}

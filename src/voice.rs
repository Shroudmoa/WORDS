//! Voice output for Reader Mode and the typewriter.
//!
//! WORDS has no audio dependencies of its own. Speech is produced by whatever
//! the machine already has: `espeak-ng` / `espeak` offline on Linux, `say` on
//! macOS, `spd-say` through speech-dispatcher, or the same Python `gTTS` your
//! Telegram bot uses when an online voice is the only one around. A missing
//! engine is reported to the UI rather than crashing the frame loop.
//!
//! Playback runs on a small worker thread so the terminal keeps painting at
//! 60fps while a sentence is being spoken. The worker reports which sentence
//! it has reached, which is what lets the reader highlight its read-along line.

use std::fs;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::config;

const POLL_MS: u64 = 20;
pub const MIN_RATE: i32 = 60;
pub const MAX_RATE: i32 = 400;

/// Settings are a plain `key=value` file in the config directory. Unknown keys
/// are ignored so a newer WORDS can add options without breaking an older file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// Master switch. `v` outside the reader flips this and writes it back.
    pub enabled: bool,
    /// Speak documents in Reader Mode.
    pub reader: bool,
    /// Speak each phrase as it appears in the typewriter.
    pub words: bool,
    /// `auto`, `espeak`, `say`, `spd-say`, `gtts` or `none`.
    pub backend: String,
    /// Voice name or language code passed to the engine.
    pub voice: String,
    /// Words per minute.
    pub rate: i32,
    /// 0..=100.
    pub volume: i32,
    /// Language code for the gTTS fallback.
    pub lang: String,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            enabled: true,
            reader: true,
            words: true,
            backend: "auto".into(),
            voice: String::new(),
            rate: 175,
            volume: 100,
            lang: "en".into(),
        }
    }
}

impl Config {
    pub fn load() -> Config {
        let Some(file) = config::voice_file() else {
            return Config::default();
        };
        let Ok(text) = fs::read_to_string(file) else {
            return Config::default();
        };
        Self::parse(&text)
    }

    fn parse(text: &str) -> Config {
        let mut c = Config::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            let Some((key, val)) = line.split_once('=') else {
                continue;
            };
            let val = val.trim();
            match key.trim().to_ascii_lowercase().as_str() {
                "enabled" => c.enabled = parse_bool(val, c.enabled),
                "reader" => c.reader = parse_bool(val, c.reader),
                "words" => c.words = parse_bool(val, c.words),
                "backend" => c.backend = val.to_ascii_lowercase(),
                "voice" => c.voice = val.to_string(),
                "rate" => {
                    if let Ok(n) = val.parse::<i32>() {
                        c.rate = n.clamp(MIN_RATE, MAX_RATE);
                    }
                }
                "volume" => {
                    if let Ok(n) = val.parse::<i32>() {
                        c.volume = n.clamp(0, 100);
                    }
                }
                "lang" if !val.is_empty() => c.lang = val.to_string(),
                _ => {}
            }
        }
        c
    }

    /// Save the settings the user changed from inside the app. A config dir we
    /// cannot write to is not fatal: the session keeps working.
    pub fn save(&self) {
        // Tests use a deliberately silent backend; never touch the user's real
        // config file just because a unit test changed a rate.
        if cfg!(test) && self.backend == "none" {
            return;
        }
        let Some(dir) = config::ensure_dir() else {
            return;
        };
        let _ = fs::write(dir.join("voice"), self.to_text());
    }

    fn to_text(&self) -> String {
        format!(
            "# words voice settings\n\
             enabled={}\n\
             reader={}\n\
             words={}\n\
             backend={}\n\
             voice={}\n\
             rate={}\n\
             volume={}\n\
             lang={}\n",
            yn(self.enabled),
            yn(self.reader),
            yn(self.words),
            self.backend,
            self.voice,
            self.rate,
            self.volume,
            self.lang
        )
    }
}

fn yn(b: bool) -> &'static str {
    if b { "true" } else { "false" }
}

fn parse_bool(s: &str, fallback: bool) -> bool {
    match s.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => true,
        "0" | "false" | "no" | "off" => false,
        _ => fallback,
    }
}

/// One speakable stretch of the document, with the char positions it covers in
/// the rendered lines. Those positions are what the read-along highlight uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sentence {
    pub text: String,
    /// `(line, column)` of the first word, inclusive.
    pub start: (usize, usize),
    /// `(line, column)` just past the last word, exclusive.
    pub end: (usize, usize),
}

/// Split rendered lines into sentences without touching the document itself.
/// A terminal full stop, question mark or exclamation mark ends one; so does a
/// blank line. Lines inside a paragraph are joined with a space, so a hard wrap
/// in a `.txt` file does not become a pause in the middle of a clause.
pub fn sentences(lines: &[Vec<char>]) -> Vec<Sentence> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut start = (0, 0);
    let mut end = (0, 0);
    let mut have = false;

    for (li, cs) in lines.iter().enumerate() {
        if cs.is_empty() {
            if have {
                flush(&mut out, &mut buf, start, end);
                have = false;
            }
            continue;
        }
        if have && !buf.ends_with(char::is_whitespace) {
            buf.push(' ');
        }
        for (ci, &c) in cs.iter().enumerate() {
            if !have && c.is_whitespace() {
                continue;
            }
            if !have {
                start = (li, ci);
                have = true;
            }
            buf.push(c);
            end = (li, ci + 1);
            let boundary = matches!(c, '.' | '!' | '?')
                && (ci + 1 == cs.len() || cs[ci + 1].is_whitespace());
            if boundary {
                flush(&mut out, &mut buf, start, end);
                have = false;
            }
        }
        let paragraph_end = li + 1 == lines.len() || lines[li + 1].is_empty();
        if have && paragraph_end {
            flush(&mut out, &mut buf, start, end);
            have = false;
        }
    }
    out
}

fn flush(out: &mut Vec<Sentence>, buf: &mut String, start: (usize, usize), end: (usize, usize)) {
    let text = buf.trim().to_string();
    // Dots and rules are not speech: they are the Markdown divider the reader
    // draws as a line, or ellipses with no words around them.
    if text.chars().any(char::is_alphanumeric) {
        out.push(Sentence { text, start, end });
    }
    buf.clear();
}

/// Commands from the UI to the playback worker.
enum Job {
    Speak {
        id: u64,
        texts: Vec<String>,
        from: usize,
    },
    Config(Config),
    Stop,
    Rate(i32),
    #[allow(dead_code)]
    Quit,
}

/// Events from the worker back to the UI.
enum Signal {
    Speaking {
        id: u64,
        index: usize,
        total: usize,
    },
    Done {
        id: u64,
    },
    Error {
        id: u64,
        message: String,
    },
}

enum Flow {
    Done,
    Stopped,
}

/// A handle to the playback worker. Cheap to keep beside either mode: all the
/// process handling lives in the thread.
pub struct Voice {
    cfg: Config,
    jobs: Option<Sender<Job>>,
    signals: Receiver<Signal>,
    /// True while any sentence of the current job is being spoken.
    pub playing: bool,
    /// Index of the sentence currently being spoken in the reader's list.
    pub current: Option<usize>,
    pub total: usize,
    error: Option<String>,
    id: u64,
}

impl Voice {
    pub fn new(cfg: Config) -> Voice {
        let (job_tx, job_rx) = mpsc::channel();
        let (sig_tx, sig_rx) = mpsc::channel();
        let worker_cfg = cfg.clone();
        thread::spawn(move || worker(worker_cfg, job_rx, sig_tx));
        Voice {
            cfg,
            jobs: Some(job_tx),
            signals: sig_rx,
            playing: false,
            current: None,
            total: 0,
            error: None,
            id: 0,
        }
    }

    /// A silent voice for tests: it exercises the same queue and state machine
    /// without trying to start a speech engine.
    #[cfg(test)]
    pub(crate) fn muted() -> Voice {
        let mut cfg = Config::default();
        cfg.backend = "none".into();
        Voice::new(cfg)
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// Pick up edits to the config file made in another mode or by hand.
    pub fn reload_config(&mut self) {
        self.cfg = Config::load();
        // The worker owns its engine settings; send the whole file so an edit
        // to backend, voice or language takes effect without a restart.
        self.send(Job::Config(self.cfg.clone()));
    }

    /// Drain worker events. Called once per frame by whichever mode is up.
    pub fn poll(&mut self) {
        while let Ok(sig) = self.signals.try_recv() {
            match sig {
                Signal::Speaking { id, index, total } if id == self.id => {
                    self.playing = true;
                    self.current = Some(index);
                    self.total = total;
                }
                Signal::Done { id } if id == self.id => {
                    self.playing = false;
                    self.current = None;
                }
                Signal::Error { id, message } if id == self.id => {
                    self.playing = false;
                    self.current = None;
                    self.error = Some(message);
                }
                // A late event from a job we already replaced: ignore it.
                _ => {}
            }
        }
    }

    pub fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }

    pub fn wants_reader(&self) -> bool {
        self.cfg.enabled && self.cfg.reader
    }

    pub fn wants_words(&self) -> bool {
        self.cfg.enabled && self.cfg.words
    }

    /// Speak a list of sentences, starting at `from`. New speech replaces the
    /// old: the worker sees the `Stop` first and drops the current process.
    pub fn speak(&mut self, texts: Vec<String>, from: usize) {
        self.stop();
        if texts.is_empty() || from >= texts.len() {
            return;
        }
        self.id = self.id.wrapping_add(1);
        self.total = texts.len();
        self.playing = true;
        self.current = None;
        self.send(Job::Speak {
            id: self.id,
            texts,
            from,
        });
    }

    /// Speak one typewriter phrase.
    pub fn speak_phrase(&mut self, text: &str) {
        let text = text.trim();
        if !self.wants_words() || text.is_empty() {
            return;
        }
        self.speak(vec![text.to_string()], 0);
    }

    pub fn stop(&mut self) {
        self.send(Job::Stop);
        self.playing = false;
        self.current = None;
    }

    pub fn set_rate(&mut self, rate: i32) {
        self.cfg.rate = rate.clamp(MIN_RATE, MAX_RATE);
        let _ = self.cfg.save();
        self.send(Job::Rate(self.cfg.rate));
    }

    /// Flip the master switch and write it back. Returns the new value.
    pub fn toggle_enabled(&mut self) -> bool {
        self.cfg.enabled = !self.cfg.enabled;
        if !self.cfg.enabled {
            self.stop();
        }
        let _ = self.cfg.save();
        self.cfg.enabled
    }

    fn send(&self, job: Job) {
        if let Some(tx) = &self.jobs {
            let _ = tx.send(job);
        }
    }
}

fn worker(mut cfg: Config, jobs: Receiver<Job>, signals: Sender<Signal>) {
    let mut rate = cfg.rate;
    while let Ok(job) = jobs.recv() {
        match job {
            Job::Speak { id, texts, from } => {
                let mut scoped = cfg.clone();
                scoped.rate = rate;
                for (i, text) in texts.iter().enumerate().skip(from) {
                    if text.trim().is_empty() {
                        continue;
                    }
                    let _ = signals.send(Signal::Speaking {
                        id,
                        index: i,
                        total: texts.len(),
                    });
                    match speak_one(&scoped, text, &jobs, &mut rate) {
                        Ok(Flow::Done) => {}
                        Ok(Flow::Stopped) => break,
                        Err(message) => {
                            let _ = signals.send(Signal::Error { id, message });
                            break;
                        }
                    }
                }
                let _ = signals.send(Signal::Done { id });
            }
            Job::Config(new) => {
                rate = new.rate.clamp(MIN_RATE, MAX_RATE);
                cfg = new;
            }
            Job::Rate(r) => rate = r.clamp(MIN_RATE, MAX_RATE),
            Job::Stop => {}
            Job::Quit => break,
        }
    }
}

#[derive(Clone, Copy)]
enum Backend {
    EspeakNg,
    Espeak,
    Say,
    SpdSay,
    Gtts,
}

impl Backend {
    fn name(self) -> &'static str {
        match self {
            Backend::EspeakNg => "espeak-ng",
            Backend::Espeak => "espeak",
            Backend::Say => "say",
            Backend::SpdSay => "spd-say",
            Backend::Gtts => "gtts",
        }
    }
}

/// Which engines to try, in order. `auto` takes the offline ones first so WORDS
/// still reads without a network; gTTS is the last resort.
fn candidates(backend: &str) -> Vec<Backend> {
    match backend {
        "none" => Vec::new(),
        "espeak" => vec![Backend::EspeakNg, Backend::Espeak],
        "say" => vec![Backend::Say],
        "spd-say" => vec![Backend::SpdSay],
        "gtts" => vec![Backend::Gtts],
        _ => vec![
            Backend::EspeakNg,
            Backend::Espeak,
            Backend::Say,
            Backend::SpdSay,
            Backend::Gtts,
        ],
    }
}

/// A temp directory that cleans itself up when the player exits.
struct TempDir(std::path::PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn speak_one(
    cfg: &Config,
    text: &str,
    jobs: &Receiver<Job>,
    rate: &mut i32,
) -> Result<Flow, String> {
    let options = candidates(&cfg.backend);
    if options.is_empty() {
        return Ok(Flow::Done);
    }
    let mut last = format!("no voice engine for `{}`", cfg.backend);
    for backend in options {
        let mut scoped = cfg.clone();
        scoped.rate = *rate;
        let (mut child, temp) = match spawn_backend(backend, &scoped, text) {
            Ok(pair) => pair,
            Err(e) => {
                last = e;
                continue;
            }
        };
        let status: ExitStatus;
        loop {
            match child.try_wait() {
                Ok(Some(st)) => {
                    status = st;
                    break;
                }
                Ok(None) => {}
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    drop(temp);
                    return Err(format!("{}: {e}", backend.name()));
                }
            }
            match jobs.try_recv() {
                Ok(Job::Stop) | Ok(Job::Quit) | Ok(Job::Speak { .. }) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    drop(temp);
                    return Ok(Flow::Stopped);
                }
                Ok(Job::Rate(r)) => *rate = r.clamp(MIN_RATE, MAX_RATE),
                // A config reload takes effect at the next sentence; changing
                // the engine under a process already speaking is pointless.
                Ok(Job::Config(_)) => {}
                Err(TryRecvError::Empty) => thread::sleep(Duration::from_millis(POLL_MS)),
                Err(TryRecvError::Disconnected) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    drop(temp);
                    return Ok(Flow::Stopped);
                }
            }
        }
        drop(temp);
        if status.success() {
            return Ok(Flow::Done);
        }
        last = format!("{} exited with {status}", backend.name());
    }
    Err(last)
}

fn spawn_backend(backend: Backend, cfg: &Config, text: &str) -> Result<(Child, Option<TempDir>), String> {
    let name = backend.name();
    let mut c = match backend {
        Backend::EspeakNg | Backend::Espeak => {
            let mut c = Command::new(name);
            let voice = if cfg.voice.is_empty() {
                cfg.lang.as_str()
            } else {
                cfg.voice.as_str()
            };
            c.arg("-v")
                .arg(voice)
                .arg("-s")
                .arg(cfg.rate.to_string())
                // espeak's amplitude is 0..=200, ours is a 0..=100 percentage.
                .arg("-a")
                .arg((cfg.volume * 2).clamp(0, 200).to_string())
                .arg(text);
            c
        }
        Backend::Say => {
            let mut c = Command::new(name);
            if !cfg.voice.is_empty() {
                c.arg("-v").arg(&cfg.voice);
            }
            c.arg("-r").arg(cfg.rate.to_string()).arg(text);
            c
        }
        Backend::SpdSay => {
            let mut c = Command::new(name);
            c.arg("-w").arg("-r").arg(cfg.rate.to_string()).arg(text);
            c
        }
        Backend::Gtts => return spawn_gtts(cfg, text),
    };
    c.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|child| (child, None))
        .map_err(|e| format!("{name}: {e}"))
}

const GTTS: &str = "import sys\n\
                    from gtts import gTTS\n\
                    gTTS(text=sys.argv[1], lang=sys.argv[2], slow=False).save(sys.argv[3])\n";

const PLAYERS: &[(&str, &[&str])] = &[
    ("ffplay", &["-nodisp", "-autoexit", "-loglevel", "quiet"]),
    ("mpg123", &["-q"]),
    ("afplay", &[]),
    ("cvlc", &["-I", "dummy", "--play-and-exit"]),
];

fn spawn_gtts(cfg: &Config, text: &str) -> Result<(Child, Option<TempDir>), String> {
    let lang = if cfg.lang.is_empty() { "en" } else { &cfg.lang };
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("words-voice-{}-{stamp}", std::process::id()));
    fs::create_dir_all(&dir).map_err(|e| format!("gtts: {e}"))?;
    let temp = TempDir(dir.clone());
    let mp3 = dir.join("speech.mp3");

    let made = Command::new("python3")
        .arg("-c")
        .arg(GTTS)
        .arg(text)
        .arg(lang)
        .arg(&mp3)
        .output()
        .map_err(|e| format!("gtts: python3: {e}"))?;
    if !made.status.success() {
        let why = String::from_utf8_lossy(&made.stderr)
            .trim()
            .lines()
            .next_back()
            .unwrap_or("could not synthesise speech")
            .to_string();
        return Err(format!("gtts: {why}"));
    }

    let Some(&(player, args)) = PLAYERS.iter().find(|(p, _)| exists(p)) else {
        return Err("gtts needs ffplay, mpg123, afplay or cvlc to play audio".into());
    };
    let child = Command::new(player)
        .args(args)
        .arg(&mp3)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("gtts: {player}: {e}"))?;
    Ok((child, Some(temp)))
}

/// Whether a command is on `PATH`. Looking rather than executing matters here:
/// `cvlc` with no arguments would sit there waiting for input forever.
fn exists(name: &str) -> bool {
    let path = Path::new(name);
    if path.components().count() > 1 {
        return path.is_file();
    }
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| dir.join(name).is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_are_listening_but_reversible() {
        let c = Config::default();
        assert!(c.enabled && c.reader && c.words);
        assert_eq!(c.backend, "auto");
        assert!(c.enabled && c.reader && c.words);
    }

    #[test]
    fn config_reads_comments_booleans_and_clamps_numbers() {
        let c = Config::parse(
            "# words voice settings\n\
             enabled = no\n\
             reader = off\n\
             words = 1\n\
             backend = SAY\n\
             voice = Alex\n\
             rate = 9999\n\
             volume = -4\n\
             lang = en-gb\n\
             nonsense = ignored\n",
        );
        assert!(!c.enabled && !c.reader && c.words);
        assert_eq!(c.backend, "say");
        assert_eq!(c.voice, "Alex");
        assert_eq!(c.rate, MAX_RATE);
        assert_eq!(c.volume, 0);
        assert_eq!(c.lang, "en-gb");
    }

    #[test]
    fn config_round_trips_through_its_own_file_format() {
        let mut c = Config::default();
        c.enabled = false;
        c.backend = "gtts".into();
        c.rate = 210;
        assert_eq!(Config::parse(&c.to_text()), c);
    }

    #[test]
    fn auto_tries_offline_engines_before_the_online_one() {
        let list = candidates("auto");
        assert_eq!(list[0].name(), "espeak-ng");
        assert_eq!(list.last().unwrap().name(), "gtts");
        assert!(candidates("none").is_empty());
        assert_eq!(candidates("say")[0].name(), "say");
    }

    fn lines(text: &str) -> Vec<Vec<char>> {
        text.split('\n').map(|l| l.chars().collect()).collect()
    }

    #[test]
    fn sentences_split_on_punctuation_and_blank_lines() {
        let s = sentences(&lines("One. Two!\nThree? Four"));
        assert_eq!(s.iter().map(|x| x.text.as_str()).collect::<Vec<_>>(), ["One.", "Two!", "Three?", "Four"]);
    }

    #[test]
    fn sentences_join_hard_wrapped_lines_but_not_paragraphs() {
        let s = sentences(&lines("a long sentence\nwrapped over lines.\n\nnew paragraph"));
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].text, "a long sentence wrapped over lines.");
        assert_eq!(s[0].start, (0, 0));
        assert_eq!(s[0].end, (1, 19));
        assert_eq!(s[1].text, "new paragraph");
        assert_eq!(s[1].start, (3, 0));
    }

    #[test]
    fn sentences_keep_unicode_char_positions() {
        let s = sentences(&lines("  Héllo, 世界!"));
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].text, "Héllo, 世界!");
        assert_eq!(s[0].start, (0, 2));
        assert_eq!(s[0].end, (0, 12));
    }

    #[test]
    fn empty_and_punctuation_only_lines_make_no_sentences() {
        assert!(sentences(&[]).is_empty());
        assert!(sentences(&lines("   \n\n...\n")).is_empty());
    }

    #[test]
    fn a_muted_voice_accepts_commands_without_spawning_audio() {
        let mut v = Voice::muted();
        v.speak(vec!["hello".into()], 0);
        assert!(v.playing);
        for _ in 0..50 {
            v.poll();
            if !v.playing {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(!v.playing, "the silent backend finishes at once");
        assert!(v.take_error().is_none());
    }
}

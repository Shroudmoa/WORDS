//! Voice output for Reader Mode and the typewriter.
//!
//! The old version shelled out to whatever speech command happened to exist and
//! called that "voice". It worked on some machines and sounded awful on all of
//! them. This version synthesises with Microsoft Edge neural voices through a
//! tiny Python helper (the same pattern as the Telegram bot, with a better
//! library), caches the audio with word timings, and drives highlighting from
//! a real playback clock.
//!
//! Rust stays dependency-free. The only setup is:
//!
//!     python3 -m pip install edge-tts
//!
//! `words --voice-doctor` checks the whole chain and says exactly what is
//! missing. Local `say` / `espeak` / `spd-say` remain as explicit fallbacks.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::config;

const POLL_MS: u64 = 16;
pub const MIN_RATE: i32 = 60;
pub const MAX_RATE: i32 = 400;
const BASE_RATE: i32 = 175;
const DEFAULT_VOICE: &str = "en-US-AvaNeural";

/// Settings, read from and written to `~/.config/words/voice`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// Master switch. `v` flips this while WORDS is running.
    pub enabled: bool,
    /// Speak documents in Reader Mode.
    pub reader: bool,
    /// Speak each phrase as it appears in the typewriter.
    pub words: bool,
    /// `edge` (neural), `piper`, `say`, `espeak`, `spd-say`, `auto` or `none`.
    pub backend: String,
    /// Edge voice name, or a path to a Piper `.onnx` model.
    pub voice: String,
    /// Words per minute. Mapped onto Edge's relative `--rate`.
    pub rate: i32,
    /// 0..=100. Mapped onto Edge's relative `--volume`.
    pub volume: i32,
    /// Edge pitch string, e.g. `+0Hz` or `-3Hz`.
    pub pitch: String,
    /// Language hint for the typewriter and for setup listings.
    pub lang: String,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            enabled: true,
            reader: true,
            words: true,
            backend: "edge".into(),
            voice: DEFAULT_VOICE.into(),
            rate: BASE_RATE,
            volume: 100,
            pitch: "+0Hz".into(),
            lang: "en-US".into(),
        }
    }
}

impl Config {
    pub fn load() -> Config {
        let Some(file) = config::voice_file() else {
            return Config::default();
        };
        match fs::read_to_string(file) {
            Ok(text) => Self::parse(&text),
            Err(_) => Config::default(),
        }
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
                "backend" => c.backend = normalise_backend(val),
                "voice" if !val.is_empty() => c.voice = val.to_string(),
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
                "pitch" if !val.is_empty() => c.pitch = valid_pitch(val, c.pitch.clone()),
                "lang" if !val.is_empty() => c.lang = val.to_string(),
                _ => {}
            }
        }
        c
    }

    pub fn save(&self) {
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
             pitch={}\n\
             lang={}\n",
            yn(self.enabled),
            yn(self.reader),
            yn(self.words),
            self.backend,
            self.voice,
            self.rate,
            self.volume,
            self.pitch,
            self.lang
        )
    }
}

fn normalise_backend(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        // Old files named the Google TTS that this rewrite replaces. Silently
        // upgrade them to the good voice rather than stranding anyone's config.
        "gtts" | "auto" | "" => "edge".into(),
        other => other.to_string(),
    }
}

fn valid_pitch(raw: &str, fallback: String) -> String {
    let s = raw.trim();
    let digits = s.trim_start_matches(['+', '-']).trim_end_matches("Hz");
    if (s.starts_with('+') || s.starts_with('-'))
        && s.ends_with("Hz")
        && !digits.is_empty()
        && digits.parse::<i32>().is_ok()
    {
        s.to_string()
    } else {
        fallback
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

/// One speakable stretch of a document, with the char positions it covers in
/// the rendered lines. The same shape serves the typewriter with dummy positions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sentence {
    pub text: String,
    pub start: (usize, usize),
    pub end: (usize, usize),
    /// One document position per char of `text`. Word boundaries are char
    /// offsets into `text`, so this makes highlighting exact even across hard
    /// wraps and Unicode.
    pub positions: Vec<(usize, usize)>,
}

impl Sentence {
    /// A standalone phrase with no document coordinates.
    pub fn simple(text: impl Into<String>) -> Sentence {
        let text = text.into();
        let positions = vec![(0, 0); text.chars().count()];
        let end = (0, text.chars().count());
        Sentence {
            text,
            start: (0, 0),
            end,
            positions,
        }
    }
}

/// Split rendered lines into sentences. Punctuation and blank lines end one;
/// hard-wrapped lines inside a paragraph are joined with a space.
pub fn sentences(lines: &[Vec<char>]) -> Vec<Sentence> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut positions = Vec::new();
    let mut start = (0, 0);
    let mut end = (0, 0);
    let mut have = false;

    for (li, cs) in lines.iter().enumerate() {
        if cs.is_empty() {
            if have {
                flush(&mut out, &mut buf, &mut positions, start, end);
                have = false;
            }
            continue;
        }
        if have && !buf.ends_with(char::is_whitespace) {
            // The space is not in the document; point it at the end of the
            // previous line so a word never lands in a pretend column.
            let join = positions.last().copied().unwrap_or((li, 0));
            buf.push(' ');
            positions.push(join);
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
            positions.push((li, ci));
            end = (li, ci + 1);
            let boundary = matches!(c, '.' | '!' | '?')
                && (ci + 1 == cs.len() || cs[ci + 1].is_whitespace());
            if boundary {
                flush(&mut out, &mut buf, &mut positions, start, end);
                have = false;
            }
        }
        if have && (li + 1 == lines.len() || lines[li + 1].is_empty()) {
            flush(&mut out, &mut buf, &mut positions, start, end);
            have = false;
        }
    }
    out
}

fn flush(
    out: &mut Vec<Sentence>,
    buf: &mut String,
    positions: &mut Vec<(usize, usize)>,
    start: (usize, usize),
    end: (usize, usize),
) {
    let chars: Vec<char> = buf.chars().collect();
    let mut first = 0;
    while first < chars.len() && chars[first].is_whitespace() {
        first += 1;
    }
    let mut last = chars.len();
    while last > first && chars[last - 1].is_whitespace() {
        last -= 1;
    }
    if first < last {
        let text = chars[first..last].iter().collect::<String>();
        // Rules and bare ellipses are not speech.
        if text.chars().any(char::is_alphanumeric) {
            out.push(Sentence {
                text,
                start,
                end,
                positions: positions[first..last].to_vec(),
            });
        }
    }
    buf.clear();
    positions.clear();
}

/// A word boundary from the synthesiser, mapped onto chars of the sentence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WordTiming {
    pub start: usize,
    pub end: usize,
    /// .NET ticks (100 ns each) from the start of the utterance.
    pub offset: u64,
    pub duration: u64,
}

fn ticks(t: u64) -> Duration {
    Duration::from_nanos(t.saturating_mul(100))
}

/// The helper writes `offset<TAB>duration<TAB>word` lines. No JSON dependency.
fn parse_timings(text: &str) -> Vec<(u64, u64, String)> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.splitn(3, '\t');
            let offset = fields.next()?.parse().ok()?;
            let duration = fields.next()?.parse().ok()?;
            let word = fields.next()?.to_string();
            Some((offset, duration, word))
        })
        .collect()
}

/// Lowercase, and turn punctuation into a wildcard that still occupies one
/// char, so a char index in the normalised text is an index in the original.
fn norm_char(c: char) -> char {
    let lower = c.to_lowercase().next().unwrap_or(c);
    if lower.is_alphanumeric() { lower } else { '\0' }
}

fn find_chars(hay: &[char], from: usize, needle: &[char]) -> Option<usize> {
    if needle.is_empty() || from >= hay.len() {
        return None;
    }
    (from..=hay.len() - needle.len()).find(|&i| hay[i..i + needle.len()] == needle[..])
}

/// Map Edge's word strings back onto the sentence we asked it to say. Edge
/// adds punctuation and sometimes changes case; matching on a normalised,
/// length-preserving copy keeps the char offsets exact.
fn map_words(text: &str, raw: &[(u64, u64, String)]) -> Vec<WordTiming> {
    let hay: Vec<char> = text.chars().map(norm_char).collect();
    let mut out = Vec::new();
    let mut from = 0;
    for (offset, duration, word) in raw {
        let needle: Vec<char> = word.trim().chars().map(norm_char).collect();
        if let Some(at) = find_chars(&hay, from, &needle) {
            out.push(WordTiming {
                start: at,
                end: at + needle.len(),
                offset: *offset,
                duration: *duration,
            });
            from = at + needle.len();
        }
    }
    out
}

/// Engines without word boundaries still get a moving highlight: spread words
/// evenly across the duration implied by the requested speaking rate.
fn estimate_words(text: &str, rate: i32) -> Vec<WordTiming> {
    let chars: Vec<char> = text.chars().collect();
    let mut ranges = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && !chars[i].is_whitespace() {
            i += 1;
        }
        ranges.push((start, i));
    }
    if ranges.is_empty() {
        return Vec::new();
    }
    let per_word = Duration::from_secs_f64(60.0 / rate.max(MIN_RATE) as f64);
    ranges
        .into_iter()
        .enumerate()
        .map(|(i, (start, end))| WordTiming {
            start,
            end,
            offset: (per_word * i as u32).as_nanos() as u64,
            duration: per_word.as_nanos() as u64,
        })
        .collect()
}

fn speech_duration(words: &[WordTiming], fallback: Duration) -> Duration {
    words
        .iter()
        .map(|w| ticks(w.offset.saturating_add(w.duration)))
        .max()
        .unwrap_or(fallback)
}

/// Where the audio comes from.
enum Source {
    /// Cached MP3 or WAV, with optional seeking.
    File(PathBuf),
    /// A local engine that speaks the text itself.
    Command { program: String, args: Vec<String> },
}

struct Speech {
    source: Source,
    words: Vec<WordTiming>,
}

#[derive(Clone, Copy)]
enum Backend {
    Edge,
    Piper,
    Say,
    EspeakNg,
    Espeak,
    SpdSay,
}

impl Backend {
    fn name(self) -> &'static str {
        match self {
            Backend::Edge => "edge-tts",
            Backend::Piper => "piper",
            Backend::Say => "say",
            Backend::EspeakNg => "espeak-ng",
            Backend::Espeak => "espeak",
            Backend::SpdSay => "spd-say",
        }
    }
}

fn candidates(cfg: &Config) -> Vec<Backend> {
    match cfg.backend.as_str() {
        "none" => Vec::new(),
        "edge" => vec![Backend::Edge],
        "piper" => vec![Backend::Piper],
        "say" => vec![Backend::Say],
        "espeak" => vec![Backend::EspeakNg, Backend::Espeak],
        "spd-say" => vec![Backend::SpdSay],
        _ => vec![
            Backend::Edge,
            Backend::Piper,
            Backend::Say,
            Backend::EspeakNg,
            Backend::Espeak,
            Backend::SpdSay,
        ],
    }
}

/// Edge's flags are percentages rather than WPM. 175 WPM is the neutral point.
fn edge_rate(rate: i32) -> String {
    let pct = (((rate as f64 / BASE_RATE as f64) - 1.0) * 100.0).round() as i32;
    format!("{:+}%", pct.clamp(-50, 100))
}

fn edge_volume(volume: i32) -> String {
    format!("{:+}%", (volume - 100).clamp(-100, 100))
}

fn fnv(text: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for b in text.bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn cache_key(text: &str, cfg: &Config) -> String {
    fnv(&format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
        text, cfg.backend, cfg.voice, cfg.rate, cfg.pitch
    ))
    .to_string()
}

/// Write the embedded helper to a temp file. The installed binary needs no
/// source tree, and a few kilobytes per run is nothing.
fn helper_path() -> Result<PathBuf, String> {
    let path = std::env::temp_dir().join(format!("words-voice-helper-{}.py", std::process::id()));
    fs::write(&path, include_str!("voice_helper.py"))
        .map_err(|e| format!("voice helper: {e}"))?;
    Ok(path)
}

fn synthesise(text: &str, cfg: &Config) -> Result<(PathBuf, Vec<(u64, u64, String)>), String> {
    let dir = config::voice_cache_dir()
        .ok_or("no config directory to cache speech in")?
        .join(cache_key(text, cfg));
    fs::create_dir_all(&dir).map_err(|e| format!("voice cache: {e}"))?;
    let audio = dir.join("audio.mp3");
    let timings_file = dir.join("words.tsv");

    let cached = fs::metadata(&audio).is_ok_and(|m| m.len() > 0) && timings_file.is_file();
    if !cached {
        let helper = helper_path()?;
        let out = Command::new("python3")
            .arg(&helper)
            .arg("synthesize")
            .arg("--text")
            .arg(text)
            .arg("--voice")
            .arg(&cfg.voice)
            .arg("--rate")
            .arg(edge_rate(cfg.rate))
            .arg("--volume")
            .arg(edge_volume(cfg.volume))
            .arg("--pitch")
            .arg(&cfg.pitch)
            .arg("--out")
            .arg(&audio)
            .arg("--timings")
            .arg(&timings_file)
            .output()
            .map_err(|e| format!("python3: {e}"))?;
        if !out.status.success() {
            let why = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(why.lines().last().unwrap_or("edge-tts failed").to_string());
        }
    }
    let raw = fs::read_to_string(&timings_file).map_err(|e| format!("voice timings: {e}"))?;
    Ok((audio, parse_timings(&raw)))
}

fn system_command(backend: Backend, cfg: &Config, text: &str) -> (String, Vec<String>) {
    let name = backend.name().to_string();
    let args = match backend {
        Backend::EspeakNg | Backend::Espeak => vec![
            "-v".into(),
            if cfg.voice.is_empty() {
                cfg.lang.clone()
            } else {
                cfg.voice.clone()
            },
            "-s".into(),
            cfg.rate.to_string(),
            "-a".into(),
            (cfg.volume * 2).clamp(0, 200).to_string(),
            text.into(),
        ],
        Backend::Say => {
            let mut a = Vec::new();
            if !cfg.voice.is_empty() && !cfg.voice.ends_with(".onnx") {
                a.push("-v".into());
                a.push(cfg.voice.clone());
            }
            a.push("-r".into());
            a.push(cfg.rate.to_string());
            a.push(text.into());
            a
        }
        Backend::SpdSay => vec![
            "-w".into(),
            "-r".into(),
            cfg.rate.to_string(),
            text.into(),
        ],
        Backend::Piper => vec![
            "--model".into(),
            cfg.voice.clone(),
            "--output_file".into(),
            "/dev/stdout".into(), // not used: Piper is prepared as a file below
            text.into(),
        ],
        Backend::Edge => Vec::new(),
    };
    (name, args)
}

/// Piper writes a WAV we can seek in a player, so treat it like a cache entry.
fn synthesise_piper(cfg: &Config, text: &str) -> Result<(PathBuf, Vec<(u64, u64, String)>), String> {
    let dir = config::voice_cache_dir()
        .ok_or("no config directory to cache speech in")?
        .join(format!("piper-{}", cache_key(text, cfg)));
    fs::create_dir_all(&dir).map_err(|e| format!("voice cache: {e}"))?;
    let wav = dir.join("audio.wav");
    if !fs::metadata(&wav).is_ok_and(|m| m.len() > 0) {
        let out = Command::new("piper")
            .arg("--model")
            .arg(&cfg.voice)
            .arg("--output_file")
            .arg(&wav)
            .arg(text)
            .output()
            .map_err(|e| format!("piper: {e}"))?;
        if !out.status.success() {
            return Err(format!("piper exited with {}", out.status));
        }
    }
    Ok((wav, Vec::new()))
}

fn prepare(text: &str, cfg: &Config, rate: i32) -> Result<Speech, String> {
    let mut last = "no voice engine configured".to_string();
    for backend in candidates(cfg) {
        match backend {
            Backend::Edge => match synthesise(text, cfg) {
                Ok((audio, raw)) => {
                    let words = map_words(text, &raw);
                    return Ok(Speech {
                        source: Source::File(audio),
                        words,
                    });
                }
                Err(e) => last = e,
            },
            Backend::Piper => match synthesise_piper(cfg, text) {
                Ok((wav, raw)) => {
                    let mut scoped = cfg.clone();
                    scoped.rate = rate;
                    let words = if raw.is_empty() {
                        estimate_words(text, rate)
                    } else {
                        map_words(text, &raw)
                    };
                    return Ok(Speech {
                        source: Source::File(wav),
                        words,
                    });
                }
                Err(e) => last = e,
            },
            other => {
                let mut scoped = cfg.clone();
                scoped.rate = rate;
                let (program, args) = system_command(other, &scoped, text);
                if !exists(&program) {
                    last = format!("{program} is not installed");
                    continue;
                }
                return Ok(Speech {
                    source: Source::Command { program, args },
                    words: estimate_words(text, rate),
                });
            }
        }
    }
    Err(last)
}

/// Whether a command is on `PATH`. Looking rather than executing matters: some
/// players wait for input when called with no arguments.
fn exists(name: &str) -> bool {
    let path = Path::new(name);
    if path.components().count() > 1 {
        return path.is_file();
    }
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| dir.join(name).is_file())
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AudioKind {
    Mp3,
    Wav,
}

fn audio_kind(path: &Path) -> AudioKind {
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("wav")) {
        AudioKind::Wav
    } else {
        AudioKind::Mp3
    }
}

const PLAYERS: &[(&str, &[&str])] = &[
    ("ffplay", &[]),
    ("mpg123", &[]),
    ("afplay", &[]),
    ("cvlc", &["-I", "dummy", "--play-and-exit"]),
];

const WAV_PLAYERS: &[(&str, &[&str])] = &[
    ("ffplay", &[]),
    ("paplay", &[]),
    ("aplay", &[]),
    ("afplay", &[]),
];

/// Build a player command. `ffplay` and `afplay` can start at an offset, which
/// is what makes pause/resume exact; other players restart at the beginning.
fn player_command(path: &Path, seek: Duration) -> Result<(String, Vec<String>), String> {
    let is_wav = audio_kind(path) == AudioKind::Wav;
    let list = if is_wav { WAV_PLAYERS } else { PLAYERS };
    let secs = seek.as_secs_f64();
    let searching = secs > 0.05;

    if searching && exists("ffplay") {
        let mut args = vec![
            "-nodisp".into(),
            "-autoexit".into(),
            "-loglevel".into(),
            "quiet".into(),
        ];
        if is_wav {
            args.push("-f".into());
            args.push("wav".into());
        }
        args.push("-ss".into());
        args.push(secs.to_string());
        args.push(path.display().to_string());
        return Ok(("ffplay".into(), args));
    }
    if searching && is_wav && exists("paplay") {
        // PulseAudio cannot seek in a file; a seeked ffplay is preferable, and
        // this branch is only reached when ffplay is absent.
    }
    if searching && exists("afplay") {
        return Ok((
            "afplay".into(),
            vec!["-time".into(), secs.to_string(), path.display().to_string()],
        ));
    }
    for &(name, base) in list {
        if !exists(name) {
            continue;
        }
        let mut args = base.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        if name == "ffplay" {
            args.extend([
                "-nodisp".into(),
                "-autoexit".into(),
                "-loglevel".into(),
                "quiet".into(),
            ]);
            if is_wav {
                args.push("-f".into());
                args.push("wav".into());
            }
        }
        args.push(path.display().to_string());
        return Ok((name.into(), args));
    }
    Err("no audio player found — install ffplay, mpg123, paplay or afplay".into())
}

fn spawn_speech(source: &Source, seek: Duration) -> Result<Child, String> {
    let (program, args) = match source {
        Source::File(path) => player_command(path, seek)?,
        Source::Command { program, args } => (program.clone(), args.clone()),
    };
    Command::new(&program)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{program}: {e}"))
}

// -- events and shared state ------------------------------------------------

enum Job {
    Speak {
        id: u64,
        pieces: Vec<String>,
        from: usize,
    },
    Pause,
    Resume,
    Stop,
    SeekSentence(usize),
    Rate(i32),
    Config(Config),
}

enum Signal {
    Speaking {
        id: u64,
        index: usize,
        total: usize,
    },
    Word {
        id: u64,
        sentence: usize,
        start: usize,
        end: usize,
    },
    Paused {
        id: u64,
    },
    Resumed {
        id: u64,
    },
    Done {
        id: u64,
    },
    Stopped {
        id: u64,
    },
    Error {
        id: u64,
        message: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct State {
    pub playing: bool,
    pub paused: bool,
    pub current_sentence: Option<usize>,
    pub total: usize,
    /// `(sentence index, char start, char end)` relative to that sentence.
    pub word: Option<(usize, usize, usize)>,
}

struct Inner {
    cfg: Config,
    jobs: Option<Sender<Job>>,
    signals: Receiver<Signal>,
    state: State,
    error: Option<String>,
    id: u64,
}

/// A cloneable handle to one playback worker. Main and Reader share it, so
/// there is no duplicate config state or second audio process.
#[derive(Clone)]
pub struct Voice {
    inner: Arc<Mutex<Inner>>,
}

impl Voice {
    pub fn new(cfg: Config) -> Voice {
        let (job_tx, job_rx) = mpsc::channel();
        let (sig_tx, sig_rx) = mpsc::channel();
        let worker_cfg = cfg.clone();
        thread::spawn(move || {
            let mut worker = Worker::new(worker_cfg, job_rx, sig_tx);
            worker.run();
        });
        Voice {
            inner: Arc::new(Mutex::new(Inner {
                cfg,
                jobs: Some(job_tx),
                signals: sig_rx,
                state: State::default(),
                error: None,
                id: 0,
            })),
        }
    }

    #[cfg(test)]
    pub(crate) fn muted() -> Voice {
        let mut cfg = Config::default();
        cfg.backend = "none".into();
        Voice::new(cfg)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn send(&self, job: Job) {
        if let Some(tx) = &self.lock().jobs {
            let _ = tx.send(job);
        }
    }

    pub fn poll(&self) {
        let mut inner = self.lock();
        while let Ok(sig) = inner.signals.try_recv() {
            let id = match &sig {
                Signal::Speaking { id, .. }
                | Signal::Word { id, .. }
                | Signal::Paused { id }
                | Signal::Resumed { id }
                | Signal::Done { id }
                | Signal::Stopped { id }
                | Signal::Error { id, .. } => *id,
            };
            if id != inner.id {
                continue;
            }
            match sig {
                Signal::Speaking {
                    index, total, ..
                } => {
                    inner.state.playing = true;
                    inner.state.paused = false;
                    inner.state.current_sentence = Some(index);
                    inner.state.total = total;
                    inner.state.word = None;
                }
                Signal::Word {
                    sentence, start, end, ..
                } => {
                    inner.state.current_sentence = Some(sentence);
                    inner.state.word = Some((sentence, start, end));
                }
                Signal::Paused { .. } => {
                    inner.state.playing = false;
                    inner.state.paused = true;
                }
                Signal::Resumed { .. } => {
                    inner.state.playing = true;
                    inner.state.paused = false;
                }
                Signal::Done { .. } | Signal::Stopped { .. } => {
                    inner.state = State::default();
                }
                Signal::Error { message, .. } => {
                    inner.state = State::default();
                    inner.error = Some(message);
                }
            }
        }
    }

    pub fn state(&self) -> State {
        self.lock().state
    }

    pub fn is_playing(&self) -> bool {
        self.state().playing
    }

    pub fn is_paused(&self) -> bool {
        self.state().paused
    }

    pub fn current_sentence(&self) -> Option<usize> {
        self.state().current_sentence
    }

    pub fn current_word(&self) -> Option<(usize, usize, usize)> {
        self.state().word
    }

    pub fn config(&self) -> Config {
        self.lock().cfg.clone()
    }

    pub fn take_error(&self) -> Option<String> {
        self.lock().error.take()
    }

    pub fn wants_reader(&self) -> bool {
        let c = self.config();
        c.enabled && c.reader
    }

    pub fn wants_words(&self) -> bool {
        let c = self.config();
        c.enabled && c.words
    }

    /// Speak a list of sentences starting at `from`.
    pub fn speak(&self, sentences: Vec<Sentence>, from: usize) {
        self.stop();
        if sentences.is_empty() || from >= sentences.len() {
            return;
        }
        let id = {
            let mut inner = self.lock();
            inner.id = inner.id.wrapping_add(1);
            inner.state.total = sentences.len();
            inner.error = None;
            inner.id
        };
        self.send(Job::Speak {
            id,
            pieces: sentences.into_iter().map(|s| s.text).collect(),
            from,
        });
    }

    pub fn speak_phrase(&self, text: &str) {
        let text = text.trim();
        if !self.wants_words() || text.is_empty() {
            return;
        }
        self.speak(vec![Sentence::simple(text)], 0);
    }

    /// Speak a fixed sample regardless of the per-mode switches.
    pub fn preview(&self) {
        let cfg = self.config();
        let sample = format!(
            "Hello. This is {}, reading at {} words per minute.",
            cfg.voice, cfg.rate
        );
        self.speak(vec![Sentence::simple(sample)], 0);
    }

    pub fn pause(&self) {
        self.send(Job::Pause);
    }

    pub fn resume(&self) {
        self.send(Job::Resume);
    }

    pub fn stop(&self) {
        self.send(Job::Stop);
        let mut inner = self.lock();
        inner.state = State::default();
    }

    pub fn seek_sentence(&self, index: usize) {
        self.send(Job::SeekSentence(index));
    }

    pub fn set_rate(&self, rate: i32) {
        let mut inner = self.lock();
        inner.cfg.rate = rate.clamp(MIN_RATE, MAX_RATE);
        let cfg = inner.cfg.clone();
        drop(inner);
        let _ = cfg.save();
        self.send(Job::Rate(cfg.rate));
    }

    pub fn set_voice(&self, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let mut inner = self.lock();
        inner.cfg.voice = name.to_string();
        let cfg = inner.cfg.clone();
        drop(inner);
        let _ = cfg.save();
        self.send(Job::Config(cfg));
    }

    pub fn toggle_enabled(&self) -> bool {
        let enabled = {
            let mut inner = self.lock();
            inner.cfg.enabled = !inner.cfg.enabled;
            let cfg = inner.cfg.clone();
            let _ = cfg.save();
            inner.cfg.enabled
        };
        if !enabled {
            self.stop();
        }
        enabled
    }

    pub fn reload_config(&self) {
        let cfg = Config::load();
        self.lock().cfg = cfg.clone();
        self.send(Job::Config(cfg));
    }
}

// -- the playback worker ----------------------------------------------------

struct Worker {
    cfg: Config,
    jobs: Receiver<Job>,
    signals: Sender<Signal>,
    rate: i32,
    id: u64,
    pieces: Vec<String>,
    index: usize,
    offset: Duration,
    paused: bool,
}

impl Worker {
    fn new(cfg: Config, jobs: Receiver<Job>, signals: Sender<Signal>) -> Worker {
        let rate = cfg.rate;
        Worker {
            cfg,
            jobs,
            signals,
            rate,
            id: 0,
            pieces: Vec::new(),
            index: 0,
            offset: Duration::ZERO,
            paused: false,
        }
    }

    fn emit(&self, signal: Signal) {
        let _ = self.signals.send(signal);
    }

    fn run(&mut self) {
        while let Ok(job) = self.jobs.recv() {
            match job {
                Job::Speak { id, pieces, from } => {
                    self.id = id;
                    self.pieces = pieces;
                    self.index = from;
                    self.offset = Duration::ZERO;
                    self.paused = false;
                    self.play();
                }
                Job::Pause => {
                    // Handled inside play(), where the child process lives.
                }
                Job::Resume => {
                    if self.paused && !self.pieces.is_empty() {
                        self.paused = false;
                        self.emit(Signal::Resumed { id: self.id });
                        self.play();
                    }
                }
                Job::Stop => {
                    self.pieces.clear();
                    self.paused = false;
                    self.emit(Signal::Stopped { id: self.id });
                }
                Job::SeekSentence(index) => {
                    if index < self.pieces.len() {
                        self.index = index;
                        self.offset = Duration::ZERO;
                        self.paused = false;
                        self.emit(Signal::Resumed { id: self.id });
                        self.play();
                    }
                }
                Job::Rate(r) => self.rate = r.clamp(MIN_RATE, MAX_RATE),
                Job::Config(cfg) => {
                    self.rate = cfg.rate;
                    self.cfg = cfg;
                }
            }
        }
    }

    fn play(&mut self) {
        if candidates(&self.cfg).is_empty() {
            if let Some(text) = self.pieces.get(self.index) {
                self.emit(Signal::Speaking {
                    id: self.id,
                    index: self.index,
                    total: self.pieces.len(),
                });
                let words = estimate_words(text, self.rate);
                for w in &words {
                    self.emit(Signal::Word {
                        id: self.id,
                        sentence: self.index,
                        start: w.start,
                        end: w.end,
                    });
                }
            }
            self.emit(Signal::Done { id: self.id });
            self.pieces.clear();
            return;
        }

        while self.index < self.pieces.len() {
            let text = self.pieces[self.index].clone();
            let sentence = self.index;
            let speech = match prepare(&text, &self.cfg, self.rate) {
                Ok(s) => s,
                Err(message) => {
                    self.emit(Signal::Error {
                        id: self.id,
                        message,
                    });
                    self.emit(Signal::Done { id: self.id });
                    return;
                }
            };
            let fallback = speech_duration(&speech.words, Duration::ZERO);
            let mut child = match spawn_speech(&speech.source, self.offset) {
                Ok(c) => c,
                Err(message) => {
                    self.emit(Signal::Error {
                        id: self.id,
                        message,
                    });
                    self.emit(Signal::Done { id: self.id });
                    return;
                }
            };
            self.emit(Signal::Speaking {
                id: self.id,
                index: sentence,
                total: self.pieces.len(),
            });
            let started = Instant::now();
            let mut next_word = 0;
            let mut interrupted = false;

            loop {
                let elapsed = self.offset + started.elapsed();
                while let Some(w) = speech.words.get(next_word) {
                    if ticks(w.offset) > elapsed {
                        break;
                    }
                    self.emit(Signal::Word {
                        id: self.id,
                        sentence,
                        start: w.start,
                        end: w.end,
                    });
                    next_word += 1;
                }
                if elapsed >= fallback && fallback > Duration::ZERO {
                    // Some players outlive the timing estimate; the word stream
                    // has still reached the end, so the child is free to go.
                    let _ = child.try_wait();
                }
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => {}
                    Err(e) => {
                        let _ = child.kill();
                        self.emit(Signal::Error {
                            id: self.id,
                            message: e.to_string(),
                        });
                        interrupted = true;
                        break;
                    }
                }
                match self.jobs.try_recv() {
                    Ok(Job::Pause) => {
                        let at = self.offset + started.elapsed();
                        let _ = child.kill();
                        let _ = child.wait();
                        self.offset = at;
                        self.paused = true;
                        self.emit(Signal::Paused { id: self.id });
                        return;
                    }
                    Ok(Job::Stop) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        self.pieces.clear();
                        self.paused = false;
                        self.emit(Signal::Stopped { id: self.id });
                        return;
                    }
                    Ok(Job::SeekSentence(index)) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        if index < self.pieces.len() {
                            self.index = index;
                            self.offset = Duration::ZERO;
                            self.emit(Signal::Resumed { id: self.id });
                            self.play();
                        }
                        return;
                    }
                    Ok(Job::Rate(r)) => self.rate = r.clamp(MIN_RATE, MAX_RATE),
                    Ok(Job::Config(cfg)) => {
                        self.rate = cfg.rate;
                        self.cfg = cfg;
                    }
                    Ok(Job::Resume) => {}
                    Ok(Job::Speak { .. }) => {
                        // `Voice::speak` always sends Stop first; reaching this
                        // means a stale command. Let the outer loop handle it.
                        let _ = child.kill();
                        let _ = child.wait();
                        interrupted = true;
                        break;
                    }
                    Err(TryRecvError::Empty) => thread::sleep(Duration::from_millis(POLL_MS)),
                    Err(TryRecvError::Disconnected) => {
                        let _ = child.kill();
                        return;
                    }
                }
            }
            if interrupted {
                return;
            }
            self.index += 1;
            self.offset = Duration::ZERO;
        }
        self.emit(Signal::Done { id: self.id });
    }
}

/// Human-readable setup report for `--voice-doctor`.
pub fn doctor() -> String {
    let mut out = Vec::new();
    out.push("VOICE DOCTOR".to_string());
    if exists("python3") {
        out.push("python3      found".into());
        match Command::new("python3")
            .arg(helper_path().unwrap_or_default())
            .arg("check")
            .output()
        {
            Ok(o) if o.status.success() => out.push("edge-tts     ready".into()),
            Ok(o) => {
                let why = String::from_utf8_lossy(&o.stderr).trim().to_string();
                out.push("edge-tts     missing — run: python3 -m pip install edge-tts".into());
                if !why.is_empty() && !why.contains("edge-tts is missing") {
                    out.push(format!("              {why}"));
                }
            }
            Err(e) => out.push(format!("edge-tts     check failed: {e}")),
        }
    } else {
        out.push("python3      missing — install Python 3 first".into());
    }
    let players = PLAYERS
        .iter()
        .chain(WAV_PLAYERS.iter())
        .map(|(n, _)| *n)
        .collect::<std::collections::BTreeSet<_>>();
    let found = players.iter().filter(|n| exists(n)).copied().collect::<Vec<_>>();
    out.push(format!(
        "player       {}",
        if found.is_empty() {
            "none — install ffplay, mpg123, paplay or afplay".into()
        } else {
            found.join(", ")
        }
    ));
    out.push("engines      edge-tts (primary), piper/say/espeak/spd-say (fallbacks)".into());
    out.push("config       ~/.config/words/voice".into());
    out.push("reader keys  :voice NAME · :preview · :rate N · V".into());
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_to_neural_voices() {
        let c = Config::default();
        assert_eq!(c.backend, "edge");
        assert_eq!(c.voice, DEFAULT_VOICE);
        assert!(c.enabled && c.reader && c.words);
    }

    #[test]
    fn config_migrates_old_gtts_and_auto_settings() {
        let c = Config::parse("backend=gtts\nvoice=en-GB-SoniaNeural\npitch=-3Hz\nrate=999\n");
        assert_eq!(c.backend, "edge");
        assert_eq!(c.voice, "en-GB-SoniaNeural");
        assert_eq!(c.pitch, "-3Hz");
        assert_eq!(c.rate, MAX_RATE);
    }

    #[test]
    fn config_round_trips_through_its_own_file_format() {
        let mut c = Config::default();
        c.enabled = false;
        c.voice = "en-GB-SoniaNeural".into();
        c.pitch = "+2Hz".into();
        c.rate = 210;
        assert_eq!(Config::parse(&c.to_text()), c);
    }

    #[test]
    fn an_explicit_backend_survives_a_settings_save() {
        let mut c = Config::default();
        c.backend = "piper".into();
        c.voice = "/models/en_amy.onnx".into();
        assert_eq!(Config::parse(&c.to_text()), c);
    }

    #[test]
    fn edge_flags_are_relative_to_the_neutral_rate() {
        assert_eq!(edge_rate(175), "+0%");
        assert_eq!(edge_rate(210), "+20%");
        assert_eq!(edge_rate(140), "-20%");
        assert_eq!(edge_volume(50), "-50%");
        assert_eq!(edge_volume(100), "+0%");
    }

    #[test]
    fn cache_key_changes_with_text_and_voice() {
        let a = Config::default();
        let mut b = a.clone();
        b.voice = "en-GB-SoniaNeural".into();
        assert_ne!(cache_key("hello", &a), cache_key("hello", &b));
        assert_ne!(cache_key("hello", &a), cache_key("goodbye", &a));
    }

    #[test]
    fn ticks_are_100_nanoseconds() {
        assert_eq!(ticks(10_000_000), Duration::from_secs(1));
        assert_eq!(ticks(1), Duration::from_nanos(100));
    }

    #[test]
    fn timings_map_back_onto_unicode_words() {
        let raw = vec![
            (0, 10_000_000, "Héllo,".to_string()),
            (10_000_000, 10_000_000, "世界!".to_string()),
        ];
        let words = map_words("  Héllo, 世界!", &raw);
        assert_eq!(words.len(), 2);
        assert_eq!((words[0].start, words[0].end), (2, 8));
        assert_eq!((words[1].start, words[1].end), (9, 12));
    }

    #[test]
    fn timings_tsv_skips_bad_lines() {
        let raw = parse_timings("0\t10\tHello\n\njunk\n20\t5\tWorld\n");
        assert_eq!(raw.len(), 2);
        assert_eq!(raw[1].2, "World");
    }

    #[test]
    fn estimated_words_cover_the_phrase() {
        let words = estimate_words("one two three", 180);
        assert_eq!(words.len(), 3);
        assert_eq!(words[0].start, 0);
        assert_eq!(words[2].end, 13);
        assert!(words[2].offset > words[1].offset);
    }

    fn lines(text: &str) -> Vec<Vec<char>> {
        text.split('\n').map(|l| l.chars().collect()).collect()
    }

    #[test]
    fn sentences_split_on_punctuation_and_blank_lines() {
        let s = sentences(&lines("One. Two!\nThree? Four"));
        assert_eq!(
            s.iter().map(|x| x.text.as_str()).collect::<Vec<_>>(),
            ["One.", "Two!", "Three?", "Four"]
        );
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
    fn a_muted_voice_finishes_without_error() {
        let v = Voice::muted();
        v.speak(vec![Sentence::simple("hello")], 0);
        for _ in 0..100 {
            v.poll();
            if !v.is_playing() {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert!(!v.is_playing());
        assert!(v.take_error().is_none());
    }

    #[test]
    fn clone_handles_share_state() {
        let v = Voice::muted();
        let other = v.clone();
        other.set_rate(200);
        assert_eq!(v.config().rate, 200);
    }
}

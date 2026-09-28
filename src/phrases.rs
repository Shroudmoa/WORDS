//! Phrases: the built-in pools, and the section format a phrase file uses.
//!
//! A phrase file is plain text, one phrase per line, grouped into named
//! sections with `# name`:
//!
//! ```text
//! # short
//! WAKE UP
//! STAY AWAKE
//! # thoughts
//! THE CITY NEVER REALLY SLEEPS
//! ```
//!
//! Lines before the first header land in `misc`. Anything the terminal can't
//! show is transliterated to ASCII (see [`ascii`]), so a file written with
//! ordinary prose and smart quotes still reads as words.
//!
//! A `words.txt` in the working directory is picked up automatically, and is
//! re-read as you edit it. `--words` names a different file or a directory of
//! them; `--lang` adds a bundled non-English set.

use crate::style::Rng;

pub const PRELUDE: &[&str] = &["SYSTEM ONLINE", "WELCOME BACK, OPERATIVE"];

/// One word or one full thought, so the machine can breathe between them.
const SHORT: &[&str] = &[
    "WAKE UP",
    "STAY AWAKE",
    "LISTEN CLOSELY",
    "LOOK AGAIN",
    "NO SIGNAL",
    "KEEP GOING",
    "DO NOT SLEEP",
    "IT IS AWAKE",
    "THE LIGHTS ARE ON",
    "SOMETHING MOVED",
    "DO NOT LOOK BACK",
    "THE SIGNAL IS CLEAN",
];

const THOUGHTS: &[&str] = &[
    "EVERY MACHINE DREAMS IN ONES AND ZEROS",
    "THE CITY NEVER REALLY SLEEPS",
    "WE BUILT THE NET AND NOW IT BUILDS US",
    "DATA IS THE NEW DARK MATTER",
    "SOMEWHERE A CLOCK IS WRONG",
    "MEMORY IS THE ONLY ROOM THAT NEVER EMPTIES",
    "THE FUTURE IS ALREADY INSTALLED",
    "WE ARE ALL JUST PATTERNS LEARNING TO DREAM",
    "NIGHT IS THE TIME FOR GOOD DECISIONS",
    "THE PAST IS A FOREIGN LANGUAGE",
    "EVERY SIGNAL LEAVES A GHOST BEHIND",
    "SILENCE IS ALSO A KIND OF DATA",
    "THE CROWD GOES QUIET BEFORE THE RAIN",
    "NOTHING IS EVER TRULY DELETED",
    "THE HUM YOU HEAR IS THE WORLD THINKING",
    "TIME MOVES ONE GLYPH AT A TIME",
];

const FRAGMENTS: &[&str] = &[
    "AWAKE",
    "SLEEP",
    "SYNTHETIC RAIN",
    "THE LAST TRAIN HOME",
    "NEON",
    "STATIC",
    "HALF REMEMBERED",
    "THE MIDNIGHT SHIFT",
    "BROKEN PACKETS",
    "A QUIET MACHINE",
    "SECOND HAND DREAMS",
    "LATE NIGHT CODE",
    "THE WIRE",
    "AFTER HOURS",
    "ONE MORE THOUGHT",
    "LOW POWER",
    "GOODBYE, SLEEP",
    "I AM STILL HERE",
    "THE CLOCK IS LYING",
    "STAY A LITTLE LONGER",
];

/// The built-in pools, in the order they are listed by `--list`.
pub const BUILTIN: &[(&str, &[&str])] = &[
    ("fragments", FRAGMENTS),
    ("short", SHORT),
    ("thoughts", THOUGHTS),
];

/// How much of each *built-in* section goes into one cycle. Without this the
/// machine would spend as long on `"NEON"` as on a full sentence. A section
/// shorter than its weight simply contributes all of it, so nothing is left out
/// of the rotation -- the weights only trim.
const WEIGHTS: &[(&str, usize)] = &[("fragments", 12), ("short", 10), ("thoughts", 12)];

/// Ceiling on a user section, so a five-thousand-line dump doesn't turn into a
/// five-hour cycle before anything else gets a turn.
const MAX_SECTION: usize = 64;

/// One named group of phrases.
pub type Section = (String, Vec<String>);

/// The built-in pools as sections, ready to mix.
pub fn builtin_sections() -> Vec<Section> {
    BUILTIN
        .iter()
        .map(|(name, lines)| {
            (
                (*name).to_string(),
                lines.iter().map(|s| s.to_string()).collect(),
            )
        })
        .collect()
}

/// Parse the section format. `##` on its own is a plain comment.
///
/// A line whose first non-space character is `#` opens a section named after
/// the rest of it; an empty name is a comment. Everything else is a phrase,
/// kept exactly as typed -- [`screen`] and [`ascii`] are applied later, by the
/// font that ends up drawing it.
pub fn parse(text: &str) -> Vec<Section> {
    let mut secs: Vec<Section> = Vec::new();
    let mut current = String::from(MISC);
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('#') {
            let name = rest.trim().to_lowercase().replace(' ', "-");
            if name.is_empty() {
                continue; // a bare `#` is just a comment
            }
            current = name;
            touch(&mut secs, &current);
            continue;
        }
        // Only reject what no font could ever draw, so a CJK-only line doesn't
        // become an empty phrase in the middle of a session.
        if screen(line).is_empty() {
            continue;
        }
        touch(&mut secs, &current).push(line.to_string());
    }
    secs.retain(|(_, lines)| !lines.is_empty());
    secs
}

/// Where phrases go when a file hasn't said otherwise.
pub const MISC: &str = "misc";

/// The section called `name`, creating it if this is the first phrase in it.
fn touch<'a>(secs: &'a mut Vec<Section>, name: &str) -> &'a mut Vec<String> {
    if let Some(i) = secs.iter().position(|(n, _)| n == name) {
        return &mut secs[i].1;
    }
    secs.push((name.to_string(), Vec::new()));
    &mut secs.last_mut().expect("just pushed").1
}

/// Append `other` to `into`, combining sections that share a name.
pub fn merge(into: &mut Vec<Section>, other: Vec<Section>) {
    for (name, lines) in other {
        touch(into, &name).extend(lines);
    }
    into.retain(|(_, lines)| !lines.is_empty());
}

/// How many lines `name` contributes to one cycle.
fn take_count(name: &str, len: usize) -> usize {
    let cap = WEIGHTS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, w)| *w)
        .unwrap_or(MAX_SECTION);
    cap.min(len)
}

/// Build one cycle from a set of sections.
///
/// Every section is shuffled and contributes up to its cap, then the whole
/// cycle is shuffled again -- so a cycle has no repeats, but its *proportions*
/// follow the weights: mostly fragments, some short lines, fewer long thoughts.
/// An empty section set gives an empty cycle, which the caller treats as
/// "nothing to show".
pub fn mix(secs: &[Section], rng: &mut Rng) -> Vec<String> {
    let mut cycle: Vec<String> = Vec::new();
    for (name, lines) in secs {
        let mut part = lines.clone();
        // Shuffle before trimming, so each pass over the pool picks a different
        // subset rather than always the same first N lines.
        shuffle(&mut part, rng);
        part.truncate(take_count(name, part.len()));
        cycle.append(&mut part);
    }
    shuffle(&mut cycle, rng);
    cycle
}

/// Everything, shuffled once at startup, so a session has an unpredictable mix
/// of one-word stabs and longer thoughts.
pub fn pool(rng: &mut Rng) -> Vec<String> {
    mix(&builtin_sections(), rng)
}

/// Bundled non-English sets, compiled in so the binary stays self-contained.
/// Latin script only -- [`ascii`] has no romanisation, so a CJK phrase would
/// fold away to nothing.
const LANGS: &[(&str, &str)] = &[
    ("de", include_str!("../lang/de.txt")),
    ("es", include_str!("../lang/es.txt")),
    ("fr", include_str!("../lang/fr.txt")),
];

/// The phrase file bundled for a language code.
pub fn lang(code: &str) -> Option<&'static str> {
    let want = code.to_lowercase();
    LANGS
        .iter()
        .find(|(c, _)| *c == want)
        .map(|(_, text)| *text)
}

/// The language codes `--lang` accepts.
pub fn langs() -> impl Iterator<Item = &'static str> {
    LANGS.iter().map(|(c, _)| *c)
}

pub fn shuffle<T>(v: &mut [T], rng: &mut Rng) {
    for i in (1..v.len()).rev() {
        let j = rng.below(i + 1);
        v.swap(i, j);
    }
}

/// Fold a phrase down to the printable ASCII the pixel font knows about.
///
/// Accents are stripped rather than dropped, so a line of ordinary prose --
/// "Café naïve", "Zürich" -- survives as words instead of losing letters.
pub fn ascii(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii() {
            out.push(c);
            continue;
        }
        let lower = c.to_lowercase().next().unwrap_or(c);
        let Some(rep) = fold(lower) else { continue };
        if c.is_uppercase() {
            for r in rep.chars() {
                out.extend(r.to_uppercase());
            }
        } else {
            out.push_str(rep);
        }
    }
    tidy(&out)
}

/// Normalise a phrase for the terminal's own font.
///
/// That font can draw anything, so there is no reason to spell "MÜSSEN" as
/// "MUSSEN" -- folding is a workaround for the pixel font's 60-odd glyphs, not
/// something the terminal needs. What is dropped is anything that would lie
/// about how much room it takes: control characters, and scripts a terminal
/// renders double-width, since the layout counts one character as one cell.
pub fn screen(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_control() {
            out.push(' ');
        } else if one_cell(c) {
            out.push(c);
        }
    }
    tidy(&out)
}

/// Whether `c` is safe to assume occupies exactly one terminal cell.
///
/// Latin (including every accented form the fold table knows), Greek and
/// Cyrillic are all single-width; CJK, Hangul and the fullwidth forms are not,
/// and the layout would come out miscentred if they were allowed through.
fn one_cell(c: char) -> bool {
    let u = c as u32;
    // Zero-width marks are the awkward case: they take up no cell, so letting
    // one through would shift the whole block one column off centre.
    if matches!(u, 0x200b..=0x200f | 0x2060..=0x2064 | 0xfeff) {
        return false;
    }
    u <= 0x2ff                      // ASCII, Latin-1, Latin Extended, IPA
        || (0x370..=0x4ff).contains(&u)    // Greek and Cyrillic
        || (0x2010..=0x205f).contains(&u)  // dashes, quotes, ellipsis
}

/// Collapse whitespace runs and trim, for both folds.
fn tidy(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// ASCII stand-in for one *lowercase* non-ASCII character. `None` drops it: the
/// font has no glyph for it, and a blank reads better than a wrong box.
fn fold(c: char) -> Option<&'static str> {
    // Latin-1 Supplement and the Latin Extended-A range -- everything that
    // actually turns up in typed prose -- plus the punctuation a text editor
    // tends to smart-quote into.
    Some(match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'æ' => "ae",
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => "c",
        'ď' | 'đ' | 'ð' => "d",
        'ß' => "ss",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
        'ĥ' | 'ħ' => "h",
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
        'ĵ' => "j",
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
        'ñ' | 'ń' | 'ņ' | 'ň' | 'ŉ' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
        'œ' => "oe",
        'þ' => "th",
        'ŕ' | 'ŗ' | 'ř' => "r",
        'ś' | 'ŝ' | 'ş' | 'š' | 'ſ' => "s",
        'ţ' | 'ť' | 'ŧ' => "t",
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
        'ṽ' => "v",
        'ŵ' => "w",
        'ý' | 'ÿ' | 'ŷ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' => "-",
        '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}' => "'",
        '\u{201c}' | '\u{201d}' | '\u{201e}' | '\u{201f}' => "\"",
        '\u{2026}' => "...",
        '\u{00a0}' => " ",
        '\u{00b7}' | '\u{2022}' => "*",
        '\u{00ab}' => "<",
        '\u{00bb}' => ">",
        '\u{00a9}' => "(c)",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accents_are_folded_not_deleted() {
        // The whole point: a user-supplied file should still read as words.
        assert_eq!(ascii("Café naïve résumé"), "Cafe naive resume");
        assert_eq!(ascii("Zürich München Malmö"), "Zurich Munchen Malmo");
        assert_eq!(ascii("Ångström"), "Angstrom");
        assert_eq!(ascii("Straße"), "Strasse");
        // Ligatures expand; a case-sensitive source keeps its case.
        assert_eq!(ascii("æon œuvre"), "aeon oeuvre");
        assert_eq!(ascii("Œuvre"), "OEuvre");
        assert_eq!(ascii("þing"), "thing");
    }

    #[test]
    fn typographic_punctuation_becomes_ascii() {
        assert_eq!(ascii("“quoted” — dashed…"), "\"quoted\" - dashed...");
        assert_eq!(ascii("it’s fine"), "it's fine");
    }

    #[test]
    fn whitespace_is_normalised_and_gaps_are_dropped() {
        assert_eq!(ascii("  a   b  \n c "), "a b c");
        // Nothing the font can draw, so nothing survives.
        assert_eq!(ascii("你好 世界"), "");
    }

    #[test]
    fn every_builtin_phrase_survives_intact() {
        for (name, lines) in BUILTIN {
            for p in *lines {
                assert_eq!(ascii(p), *p, "{name}: built-in phrase should pass through as-is");
                assert_eq!(screen(p), *p, "{name}: and so should the terminal font");
            }
        }
        for p in PRELUDE {
            assert_eq!(ascii(p), *p);
        }
    }

    // -- sections ------------------------------------------------------------

    #[test]
    fn headers_open_sections_and_the_rest_is_misc() {
        let secs = parse(
            "loose line\n\
             # Short\n\
             WAKE UP\n\
             ## just a comment\n\
             #short\n\
             NEON\n\
             #deadpan and deadpan\n\
             SECTION 4.2\n",
        );
        let names: Vec<&str> = secs.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["misc", "short", "deadpan-and-deadpan"]);
        assert_eq!(secs[0].1, ["loose line"]);
        assert_eq!(secs[1].1, ["WAKE UP", "NEON"], "case and spacing in a header");
        assert_eq!(
            secs[2].1,
            ["SECTION 4.2"],
            "a multi-word header becomes a slug, so it is not the `short` pool"
        );
    }

    #[test]
    fn merging_keeps_sections_together_and_their_order() {
        let mut a = parse("# short\nWAKE UP");
        let b = parse("# short\nSTAY AWAKE\n# thoughts\nTHE CITY SLEEPS");
        merge(&mut a, b);
        let names: Vec<&str> = a.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["short", "thoughts"]);
        assert_eq!(a[0].1, ["WAKE UP", "STAY AWAKE"]);
    }

    #[test]
    fn an_empty_section_does_not_survive_parsing() {
        let secs = parse("# short\n# thoughts\nTHE CITY SLEEPS");
        let names: Vec<&str> = secs.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["thoughts"], "a header with nothing under it is dropped");
    }

    // -- mixing --------------------------------------------------------------

    #[test]
    fn a_cycle_has_no_repeats_and_follows_the_weights() {
        let mut rng = Rng::new(5);
        let secs = builtin_sections();
        let cycle = mix(&secs, &mut rng);

        let mut seen = std::collections::HashSet::new();
        for p in &cycle {
            assert!(seen.insert(p.clone()), "{p:?} appears twice in one cycle");
        }
        for (name, want) in [("fragments", 12), ("short", 10), ("thoughts", 12)] {
            let pool = secs.iter().find(|(n, _)| n == name).expect("built-in pool");
            let got = cycle.iter().filter(|p| pool.1.contains(p)).count();
            assert_eq!(got, want, "{name}: trimmed to its weight");
            assert!(pool.1.len() > want, "{name}: the weight has to actually trim");
        }
    }

    #[test]
    fn a_user_section_contributes_all_of_it() {
        let mut rng = Rng::new(9);
        let secs = parse("# deadpan\none\ntwo\nthree");
        assert_eq!(mix(&secs, &mut rng).len(), 3, "sections we weight aren't trimmed");
    }

    #[test]
    fn a_huge_section_is_capped_so_the_cycle_turns_over() {
        let many: String = (0..500).map(|i| format!("line {i}\n")).collect();
        let secs = parse(&many);
        let mut rng = Rng::new(3);
        let cycle = mix(&secs, &mut rng);
        assert!(cycle.len() <= 64, "capped at {}, got {}", MAX_SECTION, cycle.len());
    }

    #[test]
    fn no_sections_means_no_cycle() {
        assert!(mix(&[], &mut Rng::new(1)).is_empty());
        assert!(pool(&mut Rng::new(1)).len() > 20, "the built-ins are never empty");
    }

    // -- languages -----------------------------------------------------------

    #[test]
    fn every_bundled_language_parses_and_survives_both_folds() {
        let mut any = 0;
        for code in langs() {
            let text = lang(code).expect("listed code has text");
            let secs = parse(text);
            assert!(!secs.is_empty(), "{code}: parsed to nothing");

            for (name, lines) in &secs {
                assert!(!lines.is_empty(), "{code}: {name} is empty");
                for p in lines {
                    assert!(
                        !screen(p).is_empty(),
                        "{code}: {p:?} shows as nothing in the terminal font"
                    );
                    assert!(
                        !ascii(p).is_empty(),
                        "{code}: {p:?} folds away to nothing"
                    );
                }
            }
            any += 1;
        }
        assert!(any >= 3, "expected several languages, got {any}");
    }

    #[test]
    fn an_unknown_language_is_not_silently_empty() {
        assert!(lang("klingon").is_none());
    }

    // -- the terminal font ---------------------------------------------------

    #[test]
    fn the_terminal_font_keeps_accents_the_pixel_font_has_to_fold() {
        // The whole reason there are two functions: the terminal can draw this.
        assert_eq!(screen("DAS BRUMMEN"), "DAS BRUMMEN");
        assert_eq!(screen("MÜSSEN — „leise“"), "MÜSSEN — „leise“");
        // ...and the pixel font cannot, so it substitutes rather than vanishing.
        assert_eq!(ascii("MÜSSEN"), "MUSSEN");
        assert_eq!(ascii("„leise“"), "\"leise\"");
    }

    #[test]
    fn double_width_scripts_are_dropped_rather_than_miscounted() {
        // The layout counts one character as one cell, so anything a terminal
        // renders two cells wide has to go, or the block comes out off-centre.
        assert_eq!(screen("NEON ネオン"), "NEON");
        assert_eq!(screen("a\u{200b}b"), "ab", "zero-width marks go too");
        assert_eq!(screen("two\tlines\nhere"), "two lines here");
    }
}

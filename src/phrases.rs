//! Phrases. Drop a `words.txt` in the working directory (one phrase per line)
//! to use your own, or name a file with `--words`; anything the pixel font
//! can't draw is transliterated to ASCII.

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

/// Everything, shuffled once at startup, so a session has an unpredictable mix
/// of one-word stabs and longer thoughts.
pub fn pool(rng: &mut crate::style::Rng) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    v.extend(SHORT.iter().map(|s| s.to_string()));
    v.extend(THOUGHTS.iter().map(|s| s.to_string()));
    v.extend(FRAGMENTS.iter().map(|s| s.to_string()));
    shuffle(&mut v, rng);
    v
}

pub fn shuffle<T>(v: &mut [T], rng: &mut crate::style::Rng) {
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
    out.split_whitespace().collect::<Vec<_>>().join(" ")
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
        for p in PRELUDE.iter().chain(SHORT).chain(THOUGHTS).chain(FRAGMENTS) {
            assert_eq!(ascii(p), *p, "built-in phrase should pass through as-is");
        }
    }
}

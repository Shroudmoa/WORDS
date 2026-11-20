//! The `words` config directory: a place to leave phrase files to be picked up
//! later, without naming them on the command line.
//!
//! `$XDG_CONFIG_HOME/words` (or `~/.config/words`) is scanned for `.txt` files.
//! Every one of them is offered on the in-app menu, so prose can be dropped
//! there and switched to from inside the running program.

use std::path::{Path, PathBuf};

pub const DIR_NAME: &str = "words";
pub const EXT: &str = "txt";

/// The config directory, honouring `XDG_CONFIG_HOME` the way most tools do.
pub fn dir() -> Option<PathBuf> {
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME") {
        if !x.is_empty() {
            return Some(PathBuf::from(x).join(DIR_NAME));
        }
    }
    let home = std::env::var_os("HOME")?;
    if home.is_empty() {
        return None;
    }
    Some(PathBuf::from(home).join(".config").join(DIR_NAME))
}

/// Make sure the directory exists, and hand it back so it can be named in help.
///
/// Created rather than merely looked for: the whole point is to give the user a
/// place to drop files, and a missing directory they have to guess the name of
/// is a worse invitation than one that is already there.
pub fn ensure_dir() -> Option<PathBuf> {
    let d = dir()?;
    let _ = std::fs::create_dir_all(&d);
    Some(d)
}

/// Every `.txt` file directly inside `dir`, sorted by name.
pub fn files_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == EXT))
        .collect();
    names.sort();
    names
}

/// The phrase files sitting in the config directory right now.
pub fn phrase_files() -> Vec<PathBuf> {
    dir().map(|d| files_in(&d)).unwrap_or_default()
}

/// The file Reader Mode keeps last-reading positions in. Deliberately not a
/// `.txt` or `.md`: the picker offers those, and a list of positions is not
/// something anyone wants to read as prose.
pub fn positions_file() -> Option<PathBuf> {
    dir().map(|d| d.join("positions"))
}

/// Document files sitting in the config directory, in name order. Both `.txt`
/// and `.md` are reader documents; the same `.txt` files are also offered as
/// phrase sources under their own section, which is deliberate -- a file can
/// be both a phrase pool and something to sit down and read.
pub fn reader_files() -> Vec<PathBuf> {
    let Some(d) = dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(d) else {
        return Vec::new();
    };
    let mut names: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension().is_some_and(|x| x == "txt" || x == "md")
                && !is_reserved(p)
        })
        .collect();
    names.sort();
    names
}

/// The three files the other modes own are never documents.
fn is_reserved(p: &Path) -> bool {
    p.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| {
            matches!(
                n,
                "deadlines" | "todos" | "plan" | "positions" | "voice"
            )
        })
}

/// What to call a file on the menu: its stem, upper-cased.
pub fn label(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().to_uppercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| path.display().to_string())
}

/// The file saved deadlines live in. Deliberately not a `.txt`: every `.txt`
/// in the config directory is offered on the phrase menu, and a list of dates
/// read out as phrases would be a strange afternoon.
pub fn deadlines_file() -> Option<PathBuf> {
    dir().map(|d| d.join("deadlines"))
}

/// The file the todo list lives in. Not a `.txt` for the same reason as the
/// deadline file: the phrase picker offers every `.txt` in the directory.
pub fn todos_file() -> Option<PathBuf> {
    dir().map(|d| d.join("todos"))
}

/// The file the day planner lives in.
pub fn plan_file() -> Option<PathBuf> {
    dir().map(|d| d.join("plan"))
}

/// The voice settings file. Not a `.txt`: like the other state files it would
/// otherwise turn up in the phrase picker and be read out as prose.
pub fn voice_file() -> Option<PathBuf> {
    dir().map(|d| d.join("voice"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let p = std::env::temp_dir().join(format!("words-config-test-{name}"));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).expect("scratch dir");
            Scratch(p)
        }
        fn write(&self, name: &str, body: &str) -> PathBuf {
            let p = self.0.join(name);
            fs::write(&p, body).expect("scratch file");
            p
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn only_txt_files_are_offered_and_in_name_order() {
        let d = Scratch::new("files");
        d.write("b.txt", "B");
        d.write("a.txt", "A");
        d.write("notes.md", "not a phrase file");
        d.write("no-extension", "still not");
        fs::create_dir_all(d.0.join("sub.txt")).unwrap();
        let names: Vec<String> = files_in(&d.0)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["a.txt", "b.txt"], "sorted, .txt only, dirs skipped");
    }

    #[test]
    fn a_missing_directory_is_empty_rather_than_an_error() {
        assert!(files_in(Path::new("/nonexistent/words-config")).is_empty());
    }

    #[test]
    fn a_file_is_named_by_its_stem_upper_cased() {
        assert_eq!(label(Path::new("/tmp/house-rules.txt")), "HOUSE-RULES");
        assert_eq!(label(Path::new("lang/de.txt")), "DE");
    }
}

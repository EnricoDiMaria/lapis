use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// English dictionary embedded in the executable (.aff and .dic files).
/// Place the files in `assets/dictionaries/` next to Cargo.toml.
/// The paths are RELATIVE to this source file (src/spellcheck.rs).
const BUNDLED_AFF: &[u8] = include_bytes!("../assets/dictionaries/en_US.aff");
const BUNDLED_DIC: &[u8] = include_bytes!("../assets/dictionaries/en_US.dic");

/// Folder where the app stores user-added dictionaries and personal words.
pub fn dict_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    let dir = base.join("lapis/dictionaries");
    let _ = fs::create_dir_all(&dir);
    dir
}

pub fn user_words_path() -> PathBuf {
    dict_dir().join("user_custom.txt")
}

pub fn read_user_words() -> Vec<String> {
    let mut words: Vec<String> = fs::read_to_string(user_words_path())
        .map(|c| {
            c.lines()
                .map(|l| l.trim().to_lowercase())
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default();
    words.sort();
    words.dedup();
    words
}

pub fn write_user_words(words: &[String]) -> io::Result<()> {
    let mut out = words.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    fs::write(user_words_path(), out)
}

pub struct SpellChecker {
    pub enabled: bool,
    pub loaded_languages: Vec<String>,
    dict: Option<spellbook::Dictionary>,
}

impl SpellChecker {
    pub fn new(extra_dict_paths: &[PathBuf]) -> Self {
        let mut checker = Self {
            enabled: true,
            loaded_languages: Vec::new(),
            dict: None,
        };

        checker.load_default_english();
        for p in extra_dict_paths {
            let _ = checker.load_dictionary_file(p);
        }
        checker.load_user_words();
        checker
    }

    pub fn word_count(&self) -> usize {
        0
    }

    pub fn load_default_english(&mut self) {
        // Converte i byte in &str. Se il file non è UTF-8 valido, questo fallisce.
        let aff = match std::str::from_utf8(BUNDLED_AFF) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Errore: en_US.aff non è UTF-8 valido: {:?}", e);
                return;
            }
        };
        let dic = match std::str::from_utf8(BUNDLED_DIC) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Errore: en_US.dic non è UTF-8 valido: {:?}", e);
                return;
            }
        };

        match spellbook::Dictionary::new(aff, dic) {
            Ok(d) => {
                self.dict = Some(d);
                self.loaded_languages.push("English (built-in)".to_string());
            }
            Err(e) => {
                eprintln!("Errore nel caricamento del dizionario incorporato: {:?}", e);
            }
        }
    }

    pub fn load_dictionary_file(&mut self, path: &Path) -> Result<String, String> {
        // Ci aspettiamo che `path` punti al file .aff; il .dic deve avere lo stesso nome base.
        let aff_path = path;
        let dic_path = path.with_extension("dic");
        if !dic_path.exists() {
            return Err(format!("File .dic non trovato: {:?}", dic_path));
        }
        let aff = fs::read_to_string(aff_path).map_err(|e| e.to_string())?;
        let dic = fs::read_to_string(&dic_path).map_err(|e| e.to_string())?;
        
        match spellbook::Dictionary::new(&aff, &dic) {
            Ok(d) => {
                self.dict = Some(d);
                let name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Custom")
                    .to_string();
                if !self.loaded_languages.contains(&name) {
                    self.loaded_languages.push(name.clone());
                }
                Ok(name)
            }
            Err(e) => Err(format!("Errore di parsing: {:?}", e)),
        }
    }

    fn load_user_words(&mut self) {
        if let Some(dict) = &mut self.dict {
            for w in read_user_words() {
                let _ = dict.add(&w);
            }
        }
    }

    pub fn add_user_word(&mut self, word: &str) {
        let clean = word.trim().to_lowercase();
        if clean.is_empty() {
            return;
        }
        if let Some(dict) = &mut self.dict {
            let _ = dict.add(&clean);
        }
        if let Ok(mut f) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(user_words_path())
        {
            let _ = writeln!(f, "{}", clean);
        }
    }

    /// Checks if a single token is spelled correctly (ignores numbers, acronyms, short tokens).
    pub fn check_word(&self, raw_token: &str) -> bool {
        if !self.enabled {
            return true;
        }
        let word = raw_token.trim_matches(|c: char| !c.is_alphabetic());
        if word.chars().count() <= 2 {
            return true;
        }
        // Ignore ALL-CAPS acronyms (CPU, RAM, PDF...) or tokens with digits
        if word.chars().all(|c| c.is_uppercase()) || raw_token.chars().any(|c| c.is_ascii_digit())
        {
            return true;
        }
        
        match &self.dict {
            Some(d) => d.check(word),
            None => true, // Se il dizionario non è caricato, non segnalare errori
        }
    }

    pub fn suggest(&self, misspelled: &str, max_suggestions: usize) -> Vec<String> {
        if max_suggestions == 0 {
            return Vec::new();
        }
        if let Some(dict) = &self.dict {
            let mut suggestions = Vec::new();
            dict.suggest(misspelled, &mut suggestions);
            suggestions.truncate(max_suggestions);
            suggestions
        } else {
            Vec::new()
        }
    }

    pub fn ignore_word(&mut self, word: &str) {
        let clean = word.trim().to_lowercase();
        if !clean.is_empty() {
            if let Some(dict) = &mut self.dict {
                let _ = dict.add(&clean);
            }
        }
    }
}

pub enum SpellAction {
    Replace(String),
    AddToDict,
    Ignore,
}

/// Word containing (or touching) the position `idx` (index in CHARACTERS, not bytes).
/// Uses the same definition of a word as your layouter: letters + apostrophe.
/// Returns (start, end (exclusive), word).
pub fn word_at_char_index(text: &str, idx: usize) -> Option<(usize, usize, String)> {
    let chars: Vec<char> = text.chars().collect();
    let is_w = |c: char| c.is_alphabetic() || c == '\'';
 
    let mut i = idx.min(chars.len());
    // Il cursore sta TRA due caratteri: scegli quello che appartiene a una parola
    if !(i < chars.len() && is_w(chars[i])) {
        if i > 0 && is_w(chars[i - 1]) {
            i -= 1;
        } else {
            return None;
        }
    }
 
    let mut s = i;
    while s > 0 && is_w(chars[s - 1]) {
        s -= 1;
    }
    let mut e = i;
    while e < chars.len() && is_w(chars[e]) {
        e += 1;
    }
    // Apostrofi usati come virgolette: 'ciao'
    while s < e && chars[s] == '\'' {
        s += 1;
    }
    while e > s && chars[e - 1] == '\'' {
        e -= 1;
    }
    if s >= e {
        return None;
    }
    Some((s, e, chars[s..e].iter().collect()))
}

pub fn replace_char_range(text: &mut String, start: usize, end: usize, with: &str) {
    let b0 = text.char_indices().nth(start).map(|(i, _)| i).unwrap_or(text.len());
    let b1 = text.char_indices().nth(end).map(|(i, _)| i).unwrap_or(text.len());
    text.replace_range(b0..b1, with);
}

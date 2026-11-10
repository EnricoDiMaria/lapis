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
    let dir = crate::paths::data_dir().join("dictionaries");
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
    dictionaries: Vec<spellbook::Dictionary>,
    dictionary_codes: Vec<String>,
}

impl SpellChecker {
    pub fn new(extra_dict_paths: &[PathBuf]) -> Self {
        Self::with_preferences(extra_dict_paths, true, &[])
    }

    pub fn with_preferences(
        extra_dict_paths: &[PathBuf],
        english_enabled: bool,
        disabled_paths: &[PathBuf],
    ) -> Self {
        let mut checker = Self {
            enabled: true,
            loaded_languages: Vec::new(),
            dictionaries: Vec::new(),
            dictionary_codes: Vec::new(),
        };

        if english_enabled {
            checker.load_default_english();
        }
        for p in extra_dict_paths {
            if !disabled_paths.contains(p) {
                let _ = checker.load_dictionary_file(p);
            }
        }
        checker.load_user_words();
        checker
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
                self.dictionaries.push(d);
                self.dictionary_codes.push("EN".to_string());
                self.loaded_languages.push("English (built-in)".to_string());
            }
            Err(e) => {
                eprintln!("Errore nel caricamento del dizionario incorporato: {:?}", e);
            }
        }
    }

    /// Load a full Hunspell .aff + .dic pair. A .dic may be selected in the file dialog;
    /// a plain .txt user word list is retained for backward compatibility.
    pub fn load_dictionary_file(&mut self, path: &Path) -> Result<String, String> {
        let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Custom").to_string();
        if self.loaded_languages.contains(&name) {
            return Ok(name);
        }
        let (aff, dic) = if extension.eq_ignore_ascii_case("txt") {
            let words = fs::read_to_string(path).map_err(|e| e.to_string())?;
            let words: Vec<&str> = words.lines().map(str::trim)
                .filter(|w| !w.is_empty() && !w.starts_with('#')).collect();
            ("SET UTF-8\n".to_string(), format!("{}\n{}", words.len(), words.join("\n")))
        } else if extension.eq_ignore_ascii_case("dic") || extension.eq_ignore_ascii_case("aff") {
            let aff_path = path.with_extension("aff");
            let dic_path = path.with_extension("dic");
            if !aff_path.is_file() || !dic_path.is_file() {
                return Err(format!(
                    "Both {} and {} are required (download the matching Hunspell pair)",
                    aff_path.display(), dic_path.display()
                ));
            }
            (fs::read_to_string(&aff_path).map_err(|e| e.to_string())?,
             fs::read_to_string(&dic_path).map_err(|e| e.to_string())?)
        } else {
            return Err("Select a .dic or .aff Hunspell dictionary (or a legacy .txt word list)".into());
        };
        let mut dictionary = spellbook::Dictionary::new(&aff, &dic)
            .map_err(|e| format!("Hunspell parsing failed: {e:?}"))?;
        for word in read_user_words() {
            let _ = dictionary.add(&word);
        }
        self.dictionaries.push(dictionary);
        self.dictionary_codes.push(language_code(&name));
        self.loaded_languages.push(name.clone());
        Ok(name)
    }

    fn load_user_words(&mut self) {
        let words = read_user_words();
        for dict in &mut self.dictionaries {
            for word in &words {
                let _ = dict.add(word);
            }
        }
    }

    pub fn add_user_word(&mut self, word: &str) {
        let clean = word.trim().to_lowercase();
        if clean.is_empty() {
            return;
        }
        for dict in &mut self.dictionaries {
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
        if word.chars().all(|c| c.is_uppercase()) || raw_token.chars().any(|c| c.is_ascii_digit()) {
            return true;
        }

        // A word is correct if ANY active language accepts it.
        self.dictionaries.is_empty() || self.dictionaries.iter().any(|dict| dict.check(word))
    }

    /// Collect candidates from every enabled language before ranking. Hunspell's own
    /// order is useful within one dictionary, but must not give the first loaded
    /// language priority over suggestions from other languages.
    pub fn suggest_with_languages(&self, misspelled: &str, max_suggestions: usize) -> Vec<(String, String)> {
        if max_suggestions == 0 {
            return Vec::new();
        }
        let mut candidates = Vec::new();
        for (dict, code) in self.dictionaries.iter().zip(&self.dictionary_codes) {
            let mut local = Vec::new();
            dict.suggest(misspelled, &mut local);
            candidates.extend(local.into_iter().map(|word| (word, code.clone())));
        }
        rank_suggestions(misspelled, candidates, max_suggestions)
    }

    pub fn ignore_word(&mut self, word: &str) {
        let clean = word.trim().to_lowercase();
        if !clean.is_empty() {
            for dict in &mut self.dictionaries {
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
    let b0 = text
        .char_indices()
        .nth(start)
        .map(|(i, _)| i)
        .unwrap_or(text.len());
    let b1 = text
        .char_indices()
        .nth(end)
        .map(|(i, _)| i)
        .unwrap_or(text.len());
    text.replace_range(b0..b1, with);
}

/// Normalize a Hunspell filename such as it_IT, en-US, or fr_FR to a compact tag.
fn language_code(stem: &str) -> String {
    let code = stem.split(['_', '-']).next().unwrap_or(stem);
    code.chars().take(2).collect::<String>().to_uppercase()
}

/// Rank across languages using the same case-insensitive spelling distance.
/// An adjacent transposition counts as one edit (e.g. "teh" -> "the").
fn spelling_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.to_lowercase().chars().collect();
    let b: Vec<char> = b.to_lowercase().chars().collect();
    let mut distances = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in distances.iter_mut().enumerate() {
        row[0] = i;
    }
    for j in 0..=b.len() {
        distances[0][j] = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut best = (distances[i - 1][j] + 1)
                .min(distances[i][j - 1] + 1)
                .min(distances[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                best = best.min(distances[i - 2][j - 2] + 1);
            }
            distances[i][j] = best;
        }
    }
    distances[a.len()][b.len()]
}

fn rank_suggestions(
    misspelled: &str,
    candidates: Vec<(String, String)>,
    limit: usize,
) -> Vec<(String, String)> {
    use std::collections::BTreeMap;

    // Keep one entry per case-insensitive spelling, regardless of dictionary.
    let mut unique = BTreeMap::new();
    for (word, language) in candidates {
        unique.entry(word.to_lowercase()).or_insert((word, language));
    }
    let target_len = misspelled.chars().count();
    let source_spaces = misspelled.chars().filter(|c| c.is_whitespace()).count();
    let mut ranked: Vec<_> = unique.into_iter().map(|(key, (word, language))| {
        // Hunspell can propose two valid words separated by a space. That is useful,
        // but a simple letter correction should rank ahead of splitting one word.
        // Charge two extra edits for each new word boundary, including hyphens.
        // Existing hyphens are not penalized when correcting hyphenated words.
        let added_spaces = word.chars().filter(|c| c.is_whitespace()).count()
            .saturating_sub(source_spaces);
        let added_hyphens = word.chars().filter(|&c| c == '-').count()
            .saturating_sub(misspelled.chars().filter(|&c| c == '-').count());
        let distance = spelling_distance(misspelled, &word)
            + 2 * (added_spaces + added_hyphens);
        let length_delta = target_len.abs_diff(word.chars().count());
        (distance, length_delta, key, word, language)
    }).collect();
    // Alphabetical tie-breaking is independent of dictionary load order.
    ranked.sort_by(|a, b| (&a.0, &a.1, &a.2).cmp(&(&b.0, &b.1, &b.2)));
    ranked.into_iter().take(limit).map(|(_, _, _, word, language)| (word, language)).collect()
}

#[cfg(test)]
mod suggestion_ranking_tests {
    use super::{rank_suggestions, spelling_distance};

    #[test]
    fn transpositions_and_case_are_handled() {
        assert_eq!(spelling_distance("teh", "The"), 1);
        assert_eq!(spelling_distance("ciaoo", "ciao"), 1);
        assert_eq!(spelling_distance("casa", "CASA"), 0);
    }

    #[test]
    fn best_candidates_win_regardless_of_dictionary_order() {
        let candidates = vec![
            ("castle".into(), "EN".into()),
            ("costume".into(), "EN".into()),
            ("cattle".into(), "EN".into()),
            ("canister".into(), "EN".into()),
            ("casual".into(), "EN".into()),
            ("casa".into(), "IT".into()),
        ];
        let first = rank_suggestions("casa", candidates.clone(), 5);
        let mut reversed = candidates;
        reversed.reverse();
        assert_eq!(first, rank_suggestions("casa", reversed, 5));
        assert_eq!(first[0], ("casa".into(), "IT".into()));
    }

    #[test]
    fn simple_edits_beat_splits_but_splits_remain_available() {
        let candidates = vec![
            ("in the".into(), "EN".into()),
            ("int he".into(), "EN".into()),
            ("intoe".into(), "EN".into()),
            ("into".into(), "EN".into()),
        ];
        let ranked = rank_suggestions("inthe", candidates, 4);
        assert_eq!(ranked[0].0, "intoe");
        assert!(ranked.iter().any(|(word, _)| word == "in the"));
        assert!(ranked.iter().position(|(word, _)| word == "intoe")
            < ranked.iter().position(|(word, _)| word == "in the"));
    }

    #[test]
    fn simple_edits_beat_new_hyphens_but_hyphenated_suggestions_remain() {
        let candidates = vec![
            ("in-the".into(), "EN".into()),
            ("intoe".into(), "EN".into()),
            ("into".into(), "EN".into()),
        ];
        let ranked = rank_suggestions("inthe", candidates, 3);
        assert_eq!(ranked[0].0, "intoe");
        assert!(ranked.iter().any(|(word, _)| word == "in-the"));
        assert!(ranked.iter().position(|(word, _)| word == "intoe")
            < ranked.iter().position(|(word, _)| word == "in-the"));
    }

    #[test]
    fn existing_hyphen_is_not_penalized() {
        let ranked = rank_suggestions("re-enter", vec![
            ("re-enter".into(), "EN".into()),
            ("reenter".into(), "EN".into()),
        ], 2);
        assert_eq!(ranked[0].0, "re-enter");
    }

    #[test]
    fn duplicates_and_limit() {
        let candidates = vec![
            ("ciao".into(), "IT".into()),
            ("CIAO".into(), "EN".into()),
            ("cia".into(), "IT".into()),
        ];
        let results = rank_suggestions("ciao", candidates, 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0.to_lowercase(), "ciao");
    }
}

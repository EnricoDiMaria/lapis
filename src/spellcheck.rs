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
        let edits = single_edits(misspelled);
        for (dict, code) in self.dictionaries.iter().zip(&self.dictionary_codes) {
            let mut local = Vec::new();
            dict.suggest(misspelled, &mut local);
            let hunspell_count = local.len();
            // Keep Hunspell's own order (phonetic/n-gram aware) as a tie-breaker.
            candidates.extend(
                local
                    .into_iter()
                    .enumerate()
                    .map(|(rank, word)| (word, code.clone(), rank)),
            );
            // Hunspell can miss close words (`goin` -> `going`): add every valid single edit.
            candidates.extend(
                valid_edits(dict, &edits)
                    .into_iter()
                    .enumerate()
                    .map(|(i, word)| (word, code.clone(), hunspell_count + i)),
            );
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

/// Letters tried when generating single-edit candidates: ASCII plus common accented letters.
const EDIT_ALPHABET: &str = "abcdefghijklmnopqrstuvwxyz'-àáâäåæçèéêëìíîïñòóôöøùúûüßœ";

/// Words one deletion, transposition, replacement or insertion away from `word`.
fn single_edits(word: &str) -> Vec<String> {
    let chars: Vec<char> = word.chars().collect();
    let n = chars.len();
    // Long tokens are rarely typos of one letter and would make the search slow.
    if n == 0 || n > 24 {
        return Vec::new();
    }
    let alphabet: Vec<char> = EDIT_ALPHABET.chars().collect();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let mut push = |c: Vec<char>| {
        let s: String = c.into_iter().collect();
        if s != word && seen.insert(s.clone()) {
            out.push(s);
        }
    };
    for i in 0..n {
        let mut c = chars.clone();
        c.remove(i);
        push(c);
    }
    for i in 0..n - 1 {
        let mut c = chars.clone();
        c.swap(i, i + 1);
        push(c);
    }
    for i in 0..n {
        for &a in &alphabet {
            let mut c = chars.clone();
            c[i] = a;
            push(c);
        }
    }
    for i in 0..=n {
        for &a in &alphabet {
            let mut c = chars.clone();
            c.insert(i, a);
            push(c);
        }
    }
    out
}

/// The `edits` that `dict` accepts as correct words.
fn valid_edits(dict: &spellbook::Dictionary, edits: &[String]) -> Vec<String> {
    edits
        .iter()
        .filter(|w| dict.check(w.as_str()))
        .cloned()
        .collect()
}

/// Lowercases and strips common Latin diacritics, so a missing accent is a small error.
fn fold_accents(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' => 'a',
            'ç' | 'ć' | 'č' => 'c',
            'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ę' => 'e',
            'ì' | 'í' | 'î' | 'ï' | 'ī' => 'i',
            'ł' => 'l',
            'ñ' | 'ń' => 'n',
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' => 'o',
            'ś' | 'š' => 's',
            'ù' | 'ú' | 'û' | 'ü' | 'ū' => 'u',
            'ý' | 'ÿ' => 'y',
            'ź' | 'ż' | 'ž' => 'z',
            other => other,
        })
        .collect()
}

/// Ranks candidates `(word, language, hunspell_rank)` gathered from all dictionaries.
fn rank_suggestions(
    misspelled: &str,
    candidates: Vec<(String, String, usize)>,
    limit: usize,
) -> Vec<(String, String)> {
    use std::collections::BTreeMap;

    let first_upper = |s: &str| s.chars().next().is_some_and(char::is_uppercase);
    let case_penalty = |w: &str| usize::from(first_upper(w) != first_upper(misspelled));

    // One entry per case-insensitive spelling: prefer the variant that matches the typed
    // capitalization, then Hunspell's order. A suggestion identical to the input is useless.
    let mut unique: BTreeMap<String, (String, String, usize)> = BTreeMap::new();
    for (word, language, rank) in candidates {
        if word == misspelled {
            continue;
        }
        let key = word.to_lowercase();
        let better = unique
            .get(&key)
            .is_none_or(|(w, _, r)| (case_penalty(&word), rank) < (case_penalty(w), *r));
        if better {
            unique.insert(key, (word, language, rank));
        }
    }

    let folded_input = fold_accents(misspelled);
    let input_first = folded_input.chars().next();
    let input_len = folded_input.chars().count();
    let target_len = misspelled.chars().count();
    let source_spaces = misspelled.chars().filter(|c| c.is_whitespace()).count();
    let source_hyphens = misspelled.chars().filter(|&c| c == '-').count();

    let mut ranked: Vec<_> = unique
        .into_iter()
        .map(|(key, (word, language, rank))| {
            let folded = fold_accents(&word);
            let exact = spelling_distance(misspelled, &word);
            let base = spelling_distance(&folded_input, &folded);
            // Scored in half-edits: a letter edit costs 2, an accent-only difference 1.
            let mut score = 2 * base + exact.saturating_sub(base);
            // A cut-off word (`goin` -> `going`) is likelier than an internal change.
            if exact == 1 && folded.chars().count() == input_len + 1 && folded.starts_with(&folded_input) {
                score = 1;
            }
            // Typos rarely change the first letter.
            score += usize::from(folded.chars().next() != input_first);
            // Prefer the typed capitalization (avoids proper nouns for lowercase input).
            score += case_penalty(&word);
            // A new word boundary (space or hyphen) costs two extra edits. Hunspell proposes
            // splits, but a simple letter fix must rank ahead of them.
            let added_spaces = word
                .chars()
                .filter(|c| c.is_whitespace())
                .count()
                .saturating_sub(source_spaces);
            let added_hyphens = word
                .chars()
                .filter(|&c| c == '-')
                .count()
                .saturating_sub(source_hyphens);
            score += 4 * (added_spaces + added_hyphens);
            let length_delta = target_len.abs_diff(word.chars().count());
            (score, rank, length_delta, key, word, language)
        })
        .collect();
    // Ties fall back to Hunspell's order, then length, then spelling; all independent of
    // dictionary load order.
    ranked.sort_by(|a, b| (a.0, a.1, a.2, &a.3).cmp(&(b.0, b.1, b.2, &b.3)));
    ranked
        .into_iter()
        .take(limit)
        .map(|(_, _, _, _, word, language)| (word, language))
        .collect()
}

#[cfg(test)]
mod suggestion_ranking_tests {
    use super::{
        fold_accents, rank_suggestions, single_edits, spelling_distance, valid_edits,
    };

    /// Builds candidates, using the list position as Hunspell's rank.
    fn cands(items: &[(&str, &str)]) -> Vec<(String, String, usize)> {
        items
            .iter()
            .enumerate()
            .map(|(rank, (w, l))| (w.to_string(), l.to_string(), rank))
            .collect()
    }

    fn words(ranked: &[(String, String)]) -> Vec<&str> {
        ranked.iter().map(|(w, _)| w.as_str()).collect()
    }

    #[test]
    fn transpositions_and_case_are_handled() {
        assert_eq!(spelling_distance("teh", "The"), 1);
        assert_eq!(spelling_distance("ciaoo", "ciao"), 1);
        assert_eq!(spelling_distance("casa", "CASA"), 0);
    }

    #[test]
    fn best_candidates_win_regardless_of_dictionary_order() {
        let candidates = cands(&[
            ("castle", "EN"),
            ("costume", "EN"),
            ("cattle", "EN"),
            ("canister", "EN"),
            ("casual", "EN"),
            ("casa", "IT"),
        ]);
        let first = rank_suggestions("casaa", candidates.clone(), 5);
        let mut reversed = candidates;
        reversed.reverse();
        assert_eq!(first, rank_suggestions("casaa", reversed, 5));
        assert_eq!(first[0], ("casa".into(), "IT".into()));
    }

    #[test]
    fn simple_edits_beat_splits_but_splits_remain_available() {
        let ranked = rank_suggestions(
            "inthe",
            cands(&[("in the", "EN"), ("int he", "EN"), ("intoe", "EN"), ("into", "EN")]),
            4,
        );
        let w = words(&ranked);
        assert_eq!(w[0], "intoe");
        assert!(w.iter().position(|x| *x == "intoe") < w.iter().position(|x| *x == "in the"));
        assert!(w.contains(&"in the"));
    }

    #[test]
    fn simple_edits_beat_new_hyphens_but_hyphenated_suggestions_remain() {
        let ranked = rank_suggestions(
            "inthe",
            cands(&[("in-the", "EN"), ("intoe", "EN"), ("into", "EN")]),
            3,
        );
        let w = words(&ranked);
        assert_eq!(w[0], "intoe");
        assert!(w.contains(&"in-the"));
    }

    #[test]
    fn existing_hyphen_is_not_penalized() {
        let ranked = rank_suggestions("re-entr", cands(&[("reenter", "EN"), ("re-enter", "EN")]), 2);
        assert_eq!(ranked[0].0, "re-enter");
    }

    #[test]
    fn duplicates_and_limit() {
        let ranked = rank_suggestions(
            "ciaoo",
            cands(&[("ciao", "IT"), ("CIAO", "EN"), ("cia", "IT")]),
            2,
        );
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].0, "ciao");
    }

    #[test]
    fn single_edits_cover_all_edit_kinds() {
        let edits = single_edits("goin");
        for expected in ["going", "gon", "gion", "gain", "go-in"] {
            assert!(edits.iter().any(|e| e == expected), "missing {expected}");
        }
        assert!(!edits.iter().any(|e| e == "goin"));
        assert!(single_edits("").is_empty());
        assert!(single_edits(&"a".repeat(30)).is_empty());
    }

    #[test]
    fn bundled_english_accepts_the_edit_going() {
        let aff = std::str::from_utf8(super::BUNDLED_AFF).unwrap();
        let dic = std::str::from_utf8(super::BUNDLED_DIC).unwrap();
        let dict = spellbook::Dictionary::new(aff, dic).unwrap();
        let valid = valid_edits(&dict, &single_edits("goin"));
        assert!(valid.iter().any(|w| w == "going"));
    }

    #[test]
    fn cut_off_word_completion_ranks_first() {
        let ranked = rank_suggestions(
            "goin",
            cands(&[("gin", "EN"), ("gain", "EN"), ("goon", "EN"), ("coin", "EN"), ("going", "EN")]),
            5,
        );
        assert_eq!(ranked[0].0, "going");
        // Only a single appended letter counts as a completion.
        let ranked = rank_suggestions("goin", cands(&[("gin", "EN"), ("goings", "EN")]), 2);
        assert_eq!(ranked[0].0, "gin");
    }

    #[test]
    fn missing_accent_ranks_first() {
        assert_eq!(fold_accents("Perché"), "perche");
        let ranked = rank_suggestions("perche", cands(&[("perché", "IT"), ("perce", "IT")]), 2);
        assert_eq!(ranked[0].0, "perché");
    }

    #[test]
    fn same_first_letter_is_preferred() {
        // Equal edit distance: the one keeping the first letter wins over Hunspell's order.
        let ranked = rank_suggestions("wrod", cands(&[("prod", "EN"), ("word", "EN")]), 2);
        assert_eq!(ranked[0].0, "word");
    }

    #[test]
    fn hunspell_order_breaks_ties() {
        let ranked = rank_suggestions("hte", cands(&[("the", "EN"), ("hot", "EN")]), 2);
        assert_eq!(ranked[0].0, "the");
        let ranked = rank_suggestions("hte", cands(&[("hot", "EN"), ("the", "EN")]), 2);
        assert_eq!(ranked[0].0, "the"); // transposition (1 edit) still beats a substitution+insert
    }

    #[test]
    fn typed_case_is_preferred_and_identical_input_is_dropped() {
        // Lowercase input prefers common words over proper nouns at equal distance.
        let ranked = rank_suggestions("bred", cands(&[("Brad", "EN"), ("bed", "EN")]), 2);
        assert_eq!(ranked[0].0, "bed");
        let ranked = rank_suggestions("teh", cands(&[("teh", "EN"), ("the", "EN")]), 5);
        assert_eq!(words(&ranked), vec!["the"]);
        let ranked = rank_suggestions("Romee", cands(&[("rome", "IT"), ("Rome", "EN")]), 5);
        assert_eq!(ranked, vec![("Rome".to_string(), "EN".to_string())]);
    }
}

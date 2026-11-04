//! Identità stabile dei documenti.
//!
//! Ogni PDF ha un ID (32 caratteri esadecimali) che sostituisce l'hash del percorso come chiave
//! dei dati locali: backup note (`notes/<id>.json`), cartella media (`media/<id>/`) e copia
//! dell'originale (`originals/<id>_<nome>.orig`).
//!
//! * L'ID viene scritto nel blocco note dentro il PDF, quindi segue il file: se lo sposti o lo
//!   rinomini (o lo apri su un'altra macchina/OS) i dati locali restano collegati.
//! * Una copia del PDF sulla stessa macchina (l'originale esiste ancora) riceve un ID nuovo.
//! * L'associazione percorso -> ID è in `<data_dir>/docs.json`.
//! * Niente `DefaultHasher` per i dati persistenti: il vecchio hash serve solo a migrare i dati
//!   creati dalle versioni precedenti (`migrate_legacy`).
//! * Un PDF senza blocco note non ha un ID dentro il file: se lo sposti prima di salvare
//!   delle note, i suoi dati locali non si ricollegano (come prima).

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Default, Serialize, Deserialize)]
struct Index {
    /// percorso canonico normalizzato -> id
    #[serde(default)]
    paths: BTreeMap<String, String>,
    /// id -> ultimo percorso canonico noto (serve a distinguere spostamento da copia)
    #[serde(default)]
    ids: BTreeMap<String, String>,
}

static WRITE_LOCK: Mutex<()> = Mutex::new(());
static CACHE: OnceLock<Mutex<HashMap<PathBuf, String>>> = OnceLock::new();

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn cache() -> &'static Mutex<HashMap<PathBuf, String>> {
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// FNV-1a a 64 bit: algoritmo fisso, identico su ogni piattaforma e versione di Rust.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Percorso normalizzato per confronti/chiavi (Windows: case-insensitive, separatori `/`).
fn norm_path(p: &Path) -> String {
    let s = p.to_string_lossy().into_owned();
    if cfg!(windows) {
        s.replace('\\', "/").to_lowercase()
    } else {
        s
    }
}

/// Chiave di ripiego per percorsi vuoti o non risolvibili (file inesistente): mai scritta nell'indice.
fn fallback_key(p: &Path) -> String {
    format!("p{:016x}", fnv1a64(norm_path(p).as_bytes()))
}

/// Hash usato dalle versioni precedenti (SipHash di `DefaultHasher` sul `Path` canonico).
/// Resta solo per ritrovare e migrare i vecchi dati.
fn legacy_hash(canon: &Path) -> u64 {
    let mut h = DefaultHasher::new();
    canon.hash(&mut h);
    h.finish()
}

fn new_id() -> String {
    use std::hash::{BuildHasher, RandomState};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut out = String::with_capacity(32);
    for salt in 0u64..2 {
        let mut h = RandomState::new().build_hasher();
        h.write_u128(nanos);
        h.write_u32(std::process::id());
        h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
        h.write_u64(salt);
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

fn index_path() -> PathBuf {
    crate::paths::data_dir().join("docs.json")
}

fn load_index() -> Index {
    let path = index_path();
    let Ok(bytes) = fs::read(&path) else {
        return Index::default();
    };
    match serde_json::from_slice::<Index>(&bytes) {
        Ok(idx) => idx,
        Err(_) => {
            // file corrotto: lo conservo invece di sovrascriverlo
            let _ = fs::rename(&path, path.with_extension("json.corrupt"));
            Index::default()
        }
    }
}

fn save_index(idx: &Index) {
    let path = index_path();
    if let Ok(json) = serde_json::to_string_pretty(idx) {
        let tmp = path.with_extension("json.tmp");
        if fs::write(&tmp, json).is_ok() {
            let _ = fs::rename(&tmp, &path);
        }
    }
}

/// Chiave dei dati locali del PDF, creandola (e migrando i dati legacy) se non esiste.
/// Dopo la prima chiamata è una lookup in memoria.
pub fn resolve_key(pdf_path: &Path) -> String {
    if pdf_path.as_os_str().is_empty() {
        return fallback_key(pdf_path);
    }
    if let Some(k) = lock(cache()).get(pdf_path) {
        return k.clone();
    }
    let Ok(canon) = dunce::canonicalize(pdf_path) else {
        return fallback_key(pdf_path);
    };
    let norm = norm_path(&canon);

    let _guard = lock(&WRITE_LOCK);
    // riletto da disco: un'altra istanza di Lapis può averlo aggiornato
    let mut idx = load_index();
    let id = if let Some(id) = idx.paths.get(&norm) {
        id.clone()
    } else {
        let id = match crate::notes::read_trailing_doc_id(&canon) {
            Some(tid) => match idx.ids.get(&tid).cloned() {
                None => tid,
                Some(prev) => {
                    let prev_path = Path::new(&prev);
                    if norm_path(prev_path) == norm {
                        tid
                    } else if !prev_path.exists() {
                        // il vecchio percorso non esiste più: il file è stato spostato/rinominato
                        idx.paths.remove(&norm_path(prev_path));
                        tid
                    } else {
                        // l'originale esiste ancora: questa è una copia
                        new_id()
                    }
                }
            },
            None => new_id(),
        };
        idx.paths.insert(norm, id.clone());
        idx.ids.insert(id.clone(), canon.to_string_lossy().into_owned());
        migrate_legacy(&canon, &id);
        save_index(&idx);
        id
    };
    lock(cache()).insert(pdf_path.to_path_buf(), id.clone());
    id
}

/// Come `resolve_key` ma di sola lettura: non crea ID, non migra, non legge il PDF.
/// Se il documento non è ancora noto restituisce la vecchia chiave legacy, così i dati
/// non ancora migrati restano visibili (gestione archiviazione, elenco recenti).
pub fn lookup_key(pdf_path: &Path) -> String {
    if pdf_path.as_os_str().is_empty() {
        return fallback_key(pdf_path);
    }
    if let Some(k) = lock(cache()).get(pdf_path) {
        return k.clone();
    }
    let Ok(canon) = dunce::canonicalize(pdf_path) else {
        return fallback_key(pdf_path);
    };
    match load_index().paths.get(&norm_path(&canon)) {
        Some(id) => id.clone(),
        None => format!("{:016x}", legacy_hash(&canon)),
    }
}

// ───────────────────────── migrazione dei dati legacy ─────────────────────────

fn is_legacy_name(s: &str) -> bool {
    s.len() == 16 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn move_if_absent(from: &Path, to: &Path) -> bool {
    if !from.exists() || to.exists() {
        return false;
    }
    if let Some(parent) = to.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if fs::rename(from, to).is_ok() {
        return true;
    }
    if from.is_file() && fs::copy(from, to).is_ok() {
        let _ = fs::remove_file(from);
        return true;
    }
    false
}

#[derive(Deserialize)]
struct SourceOnly {
    #[serde(default)]
    source_path: Option<PathBuf>,
}

/// Ritrova un vecchio backup/cartella media dal `source_path` salvato al loro interno.
/// Serve se l'hash legacy calcolato oggi non coincide con quello usato per creare i dati
/// (toolchain Rust diverso).
fn find_legacy_by_source(dir: &Path, canon: &Path, media: bool) -> Option<PathBuf> {
    let want = norm_path(canon);
    for e in fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        let (json_path, stem) = if media {
            if !p.is_dir() {
                continue;
            }
            (p.join("audio_index.json"), p.file_name())
        } else {
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            (p.clone(), p.file_stem())
        };
        if !is_legacy_name(stem.and_then(|s| s.to_str()).unwrap_or("")) {
            continue;
        }
        let Ok(bytes) = fs::read(&json_path) else { continue };
        let Ok(src) = serde_json::from_slice::<SourceOnly>(&bytes) else { continue };
        if src.source_path.map(|sp| norm_path(&sp)).as_deref() == Some(want.as_str()) {
            return Some(p);
        }
    }
    None
}

/// Sposta i dati creati con la vecchia chiave (hash del percorso) sotto il nuovo id.
/// Non sovrascrive mai dati già presenti sotto l'id.
fn migrate_legacy(canon: &Path, id: &str) {
    let legacy = format!("{:016x}", legacy_hash(canon));
    let notes_dir = crate::paths::notes_backup_dir();
    let media_root = crate::audio::AudioDocIndex::media_root();
    let orig_dir = crate::paths::data_dir().join("originals");

    let new_note = notes_dir.join(format!("{id}.json"));
    if !move_if_absent(&notes_dir.join(format!("{legacy}.json")), &new_note) && !new_note.exists() {
        if let Some(src) = find_legacy_by_source(&notes_dir, canon, false) {
            move_if_absent(&src, &new_note);
        }
    }

    let new_media = media_root.join(id);
    if !move_if_absent(&media_root.join(&legacy), &new_media) && !new_media.exists() {
        if let Some(src) = find_legacy_by_source(&media_root, canon, true) {
            move_if_absent(&src, &new_media);
        }
    }

    // `<hash>_<nome>.orig` -> `<id>_<nome>.orig`
    if let Ok(rd) = fs::read_dir(&orig_dir) {
        let prefix = format!("{legacy}_");
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if let Some(rest) = name.strip_prefix(&prefix) {
                move_if_absent(&e.path(), &orig_dir.join(format!("{id}_{rest}")));
            }
        }
    }
}

/// I test cambiano variabili d'ambiente di processo: vanno serializzati.
#[cfg(test)]
pub(crate) fn test_lock() -> MutexGuard<'static, ()> {
    static TL: Mutex<()> = Mutex::new(());
    lock(&TL)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lapis_docid_{}_{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn fnv_is_stable() {
        // valori di riferimento FNV-1a 64
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn ids_are_unique_and_32_hex() {
        let a = new_id();
        let b = new_id();
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    /// Un unico test sequenziale: usa variabili d'ambiente di processo.
    #[test]
    fn identity_move_copy_and_legacy_migration() {
        let _l = test_lock();
        let root = tmp("flow");
        let data_home = root.join("xdg");
        fs::create_dir_all(&data_home).unwrap();
        // SAFETY: test sequenziale, nessun altro thread legge l'ambiente.
        unsafe {
            std::env::set_var("XDG_DATA_HOME", &data_home);
            std::env::set_var("XDG_CONFIG_HOME", root.join("cfg"));
        }

        // PDF "vecchio": nessun id, ma dati legacy sotto l'hash del percorso
        let pdf1 = root.join("a.pdf");
        fs::write(&pdf1, b"%PDF-1.4\n%%EOF\n").unwrap();
        let canon1 = dunce::canonicalize(&pdf1).unwrap();
        let legacy = format!("{:016x}", legacy_hash(&canon1));
        let notes_dir = crate::paths::notes_backup_dir();
        let media_root = crate::audio::AudioDocIndex::media_root();
        let orig_dir = crate::paths::data_dir().join("originals");
        fs::create_dir_all(&orig_dir).unwrap();
        fs::create_dir_all(media_root.join(&legacy)).unwrap();
        fs::write(media_root.join(&legacy).join("lecture.wav"), b"wav").unwrap();
        fs::write(notes_dir.join(format!("{legacy}.json")), b"{}").unwrap();
        fs::write(orig_dir.join(format!("{legacy}_a.pdf.orig")), b"orig").unwrap();

        // prima del primo resolve, la chiave di sola lettura è quella legacy
        assert_eq!(lookup_key(&pdf1), legacy);

        let id1 = resolve_key(&pdf1);
        assert_eq!(id1.len(), 32);
        assert_eq!(resolve_key(&pdf1), id1, "la risoluzione deve essere stabile");
        assert_eq!(lookup_key(&pdf1), id1);
        // migrazione eseguita
        assert!(media_root.join(&id1).join("lecture.wav").exists());
        assert!(!media_root.join(&legacy).exists());
        assert!(notes_dir.join(format!("{id1}.json")).exists());
        assert!(orig_dir.join(format!("{id1}_a.pdf.orig")).exists());

        // un'altra "sessione": cache vuota, indice su disco
        lock(cache()).clear();
        assert_eq!(resolve_key(&pdf1), id1);

        // hash legacy "sbagliato" (toolchain diversa): i dati si ritrovano dal source_path salvato dentro
        let pdf2 = root.join("b.pdf");
        fs::write(&pdf2, b"%PDF-1.4\n%%EOF\n").unwrap();
        let canon2 = dunce::canonicalize(&pdf2).unwrap();
        let body = serde_json::json!({ "source_path": canon2 }).to_string();
        fs::write(notes_dir.join("deadbeefdeadbeef.json"), &body).unwrap();
        fs::create_dir_all(media_root.join("deadbeefdeadbeef")).unwrap();
        fs::write(media_root.join("deadbeefdeadbeef").join("audio_index.json"), &body).unwrap();
        let id2 = resolve_key(&pdf2);
        assert_ne!(id2, id1);
        assert!(notes_dir.join(format!("{id2}.json")).exists());
        assert!(media_root.join(&id2).join("audio_index.json").exists());
        assert!(!notes_dir.join("deadbeefdeadbeef.json").exists());
        // i dati legacy di un altro documento non vengono toccati
        assert!(notes_dir.join(format!("{id1}.json")).exists());

        // percorso inesistente / vuoto: chiave di ripiego, indice invariato
        assert!(resolve_key(&root.join("nope.pdf")).starts_with('p'));
        assert!(resolve_key(Path::new("")).starts_with('p'));

        fs::remove_dir_all(&root).ok();
    }
}

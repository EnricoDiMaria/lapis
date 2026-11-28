//! File inventory and actions for the storage settings page.

use crate::audio::{self, AudioDocIndex, remove_media_file, resolve_media_file};
use crate::{docid, notes, paths};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub(super) struct StorageEntry {
    pub(super) id: String, // Hash
    pub(super) name: String,
    pub(super) pdf_path: Option<PathBuf>,
    pub(super) pdf_exists: bool,
    pub(super) backup_path: Option<PathBuf>,
    pub(super) backup_size: u64,
    pub(super) notes_count: usize,
    pub(super) ann_count: usize,
    pub(super) orig_path: Option<PathBuf>,
    pub(super) orig_size: u64,
    pub(super) media_dir: Option<PathBuf>,
    pub(super) audio_size: u64,
    pub(super) lecture_size: u64,
    pub(super) memos_size: u64,
    pub(super) memo_count: usize,
}

impl StorageEntry {
    pub(super) fn total(&self) -> u64 {
        self.backup_size + self.orig_size + self.audio_size
    }
    pub(super) fn has_notes(&self) -> bool {
        self.notes_count > 0 || self.ann_count > 0 || self.backup_size > 0
    }
}

#[derive(Default)]
struct RawStorage {
    backup_path: Option<PathBuf>,
    backup_size: u64,
    notes_count: usize,
    ann_count: usize,
    src: Option<PathBuf>,
    media_dir: Option<PathBuf>,
    audio_size: u64,
    lecture_size: u64,
    memos_size: u64,
    memo_count: usize,
}

pub(super) enum StorageAction {
    ClearNotes(usize),
    DeleteOrig(usize),
    DeleteLecture(usize),
    DeleteMemos(usize),
    DeleteLeftovers(usize),
    CleanOrphans,
    ClearStretchCache,
    ClearRecent,
}

pub(super) fn human_size(b: u64) -> String {
    const U: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < 3 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} B", b)
    } else {
        format!("{:.1} {}", v, U[i])
    }
}

pub(super) fn dir_size(p: &Path) -> u64 {
    let mut total = 0;
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            if let Ok(m) = e.metadata() {
                //Calculates recursively the dimension of each file and subdir
                total += if m.is_dir() {
                    dir_size(&e.path())
                } else {
                    m.len()
                };
            }
        }
    }
    total
}

fn path_key(p: &Path) -> String {
    // read-only: does not create ids for recent files that were never opened
    docid::lookup_key(p)
}

pub(super) fn stretch_cache_dir() -> PathBuf {
    std::env::temp_dir().join("lapis_stretch")
}

fn read_audio_index(dir: &Path) -> Option<audio::AudioDocIndex> {
    std::fs::read_to_string(dir.join("audio_index.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
}

fn write_audio_index(dir: &Path, idx: &audio::AudioDocIndex) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(idx)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    paths::write_atomic(&dir.join("audio_index.json"), json.as_bytes())
}

fn make_storage_entry(key: String, r: RawStorage, pdf_path: Option<PathBuf>) -> StorageEntry {
    let pdf_exists = pdf_path.as_ref().map(|p| p.exists()).unwrap_or(false);
    let orig_path = pdf_path.as_ref().map(|p| notes::original_backup_lookup(p));
    let orig_size = orig_path
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .unwrap_or(0);

    let name = match &pdf_path {
        Some(p) => p
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| p.display().to_string()),
        None => format!("Unknown file ({})", key),
    };

    StorageEntry {
        id: key,
        name,
        pdf_path,
        pdf_exists,
        backup_path: r.backup_path,
        backup_size: r.backup_size,
        notes_count: r.notes_count,
        ann_count: r.ann_count,
        orig_path,
        orig_size,
        media_dir: r.media_dir,
        audio_size: r.audio_size,
        lecture_size: r.lecture_size,
        memos_size: r.memos_size,
        memo_count: r.memo_count,
    }
}

/// Scan note backups + audio folders + `.orig` copies, consolidating them by document.
/// Results sorted by total size in descending order.
pub(super) fn scan_storage(
    recent: &std::collections::VecDeque<crate::state::RecentFileEntry>,
) -> Vec<StorageEntry> {
    let mut raw: HashMap<String, RawStorage> = HashMap::new();

    //Note backups
    if let Ok(rd) = std::fs::read_dir(notes::get_backup_dir()) {
        for e in rd.flatten() {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let Some(key) = path.file_stem().map(|s| s.to_string_lossy().to_string()) else {
                continue;
            };
            let data = std::fs::read(&path)
                .ok()
                .and_then(|b| notes::parse_doc_data_bytes(&b));

            let r = raw.entry(key).or_default();
            r.backup_size = e.metadata().map(|m| m.len()).unwrap_or(0);
            r.notes_count = data.as_ref().map(|d| d.notes.len()).unwrap_or(0);
            r.ann_count = data
                .as_ref()
                .map(|d| {
                    d.annotations.values().map(|v| v.len()).sum::<usize>()
                        + d.sketches.values().map(|s| s.strokes.len()).sum::<usize>()
                })
                .unwrap_or(0);
            r.src = data.as_ref().and_then(|d| d.source_path.clone());
            r.backup_path = Some(path);
        }
    }

    //Audio folder
    if let Ok(rd) = std::fs::read_dir(audio::AudioDocIndex::media_root()) {
        for e in rd.flatten() {
            let dir = e.path();
            if !dir.is_dir() {
                continue;
            }
            let Some(key) = dir.file_name().map(|s| s.to_string_lossy().to_string()) else {
                continue;
            };
            let idx = read_audio_index(&dir);

            let r = raw.entry(key).or_default();
            r.audio_size = dir_size(&dir);
            if let Some(ix) = &idx {
                if r.src.is_none() {
                    r.src = ix.source_path.clone();
                }
                if let Some(w) = &ix.lecture_wav {
                    r.lecture_size = std::fs::metadata(resolve_media_file(&dir, w))
                        .map(|m| m.len())
                        .unwrap_or(0);
                }
                for l in &ix.other_lectures {
                    if let Some(w) = &l.wav {
                        r.lecture_size += std::fs::metadata(resolve_media_file(&dir, w))
                            .map(|m| m.len())
                            .unwrap_or(0);
                    }
                }
                for m in ix.page_memos.values().flatten() {
                    r.memo_count += 1;
                    r.memos_size += std::fs::metadata(resolve_media_file(&dir, &m.filename))
                        .map(|m| m.len())
                        .unwrap_or(0);
                }
            }
            r.media_dir = Some(dir);
        }
    }

    //Linking hash -> PDF in recent files
    let mut recent_by_key: HashMap<String, PathBuf> = HashMap::new();
    for r in recent {
        recent_by_key.insert(path_key(&r.path), r.path.clone());
    }

    let mut out: Vec<StorageEntry> = raw
        .into_iter()
        .map(|(key, r)| {
            let pdf_path = r.src.clone().or_else(|| recent_by_key.get(&key).cloned());
            make_storage_entry(key, r, pdf_path)
        })
        .collect();

    for rp in recent {
        let key = path_key(&rp.path);
        if out.iter().any(|e| e.id == key) {
            continue;
        }
        if notes::original_backup_lookup(&rp.path).exists() {
            out.push(make_storage_entry(
                key,
                RawStorage::default(),
                Some(rp.path.clone()),
            ));
        }
    }

    out.sort_by(|a, b| b.total().cmp(&a.total()));
    out
}

/// Delete notes and annotations. If the PDF exists, it also removes the block appended within the PDF
pub(super) fn storage_clear_notes(en: &StorageEntry) -> std::io::Result<()> {
    match (&en.pdf_path, en.pdf_exists) {
        (Some(p), true) => notes::save_doc_data(
            p,
            &HashMap::new(),
            &HashMap::new(),
            HashSet::new(),
            &HashMap::new(),
        ),
        _ => match &en.backup_path {
            Some(b) if b.exists() => std::fs::remove_file(b),
            _ => Ok(()),
        },
    }
}

pub(super) fn storage_delete_orig(en: &StorageEntry) -> std::io::Result<()> {
    match &en.orig_path {
        Some(o) if o.exists() => std::fs::remove_file(o),
        _ => Ok(()),
    }
}

/// Delete the recording of the lesson and everything that comes from it (transcript,
/// slide markers, waveform). Voice memos remain.
pub(super) fn storage_delete_lecture(en: &StorageEntry) -> std::io::Result<()> {
    let Some(dir) = &en.media_dir else {
        return Ok(());
    };
    if let Some(pdf) = en.pdf_path.as_deref() {
        AudioDocIndex::recover_page_change(pdf)?;
    }
    let Some(mut idx) = read_audio_index(dir) else {
        return Ok(());
    };
    let removed = idx.lecture_wav.take().into_iter().chain(
        std::mem::take(&mut idx.other_lectures)
            .into_iter()
            .filter_map(|lecture| lecture.wav),
    );
    idx.lecture_title.clear();
    idx.lecture_duration_ms = 0;
    idx.page_range = None;
    idx.timeline.clear();
    idx.timeline_is_live = false;
    idx.segments.clear();
    idx.waveform_peaks.clear();
    write_audio_index(dir, &idx)?;
    for wav in removed {
        if !idx.references_wav(&wav) {
            remove_media_file(dir, &wav)?;
        }
    }
    Ok(())
}

/// Delete all voice memos (files + index entries). The lesson remains.
pub(super) fn storage_delete_memos(en: &StorageEntry) -> std::io::Result<()> {
    let Some(dir) = &en.media_dir else {
        return Ok(());
    };
    if let Some(pdf) = en.pdf_path.as_deref() {
        AudioDocIndex::recover_page_change(pdf)?;
    }
    let Some(mut idx) = read_audio_index(dir) else {
        return Ok(());
    };
    let removed = std::mem::take(&mut idx.page_memos);
    write_audio_index(dir, &idx)?;
    for memo in removed.values().flatten() {
        if !idx.references_wav(&memo.filename) {
            remove_media_file(dir, &memo.filename)?;
        }
    }
    Ok(())
}

pub(super) fn storage_delete_leftovers(en: &StorageEntry) -> std::io::Result<()> {
    if let Some(b) = &en.backup_path
        && b.exists()
    {
        std::fs::remove_file(b)?;
    }
    storage_delete_orig(en)?;
    if let Some(d) = &en.media_dir
        && d.exists()
    {
        std::fs::remove_dir_all(d)?;
    }
    Ok(())
}

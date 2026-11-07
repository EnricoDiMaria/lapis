#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]
use std::collections::{HashMap, HashSet, VecDeque};
use std::env;
use std::path::{PathBuf, Path};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::Arc;
use std::thread;
use eframe::egui;
use eframe::egui::scroll_area::{DragScroll, ScrollSource};
use mupdf::{Colorspace, Document, Matrix};
use egui::InputState;

use crate::ZoomMode::FitWidth;
mod dialogs;
use dialogs::{AsrRetry, DialogKind, DialogSpec, Dialogs};
mod export;
mod notes;
mod sketch;
mod theme;
use theme::{rgb, SuiteTheme};
mod state;
use crate::state::*;
mod audio;
use audio::*;
mod spellcheck;
use crate::spellcheck::*;
mod clipboard;
use crate::clipboard::*;
mod screenshots;
use screenshots::{ShotJobs, ShotTarget};
mod doc_lock;
mod docid;
mod paths;
mod sysproc;

fn setup_nerd_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    // Using include_bytes! to incorporate in the executable binary (in .rodata) the font. It returns a static reference
    fonts.font_data.insert(
        "nerd_font".to_string(),
        egui::FontData::from_static(include_bytes!(
            "../assets/JetBrainsMonoNerdFont-Regular.ttf"
        ))
        .into(),
    );

    for family in [
    egui::FontFamily::Proportional,
    egui::FontFamily::Monospace,
    ] {
    fonts
        .families
        .entry(family)
        .or_default()
        .insert(0, "nerd_font".to_string());
    }   

    ctx.set_fonts(fonts);
}

fn truncate_filename<P: AsRef<Path>>(path_ref: P, max_len: usize) -> String {
    let path = path_ref.as_ref();
    
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let file_stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");

    let truncated_stem: String = {
    // Counting total characters
    let total_chars = file_stem.chars().count();

    if total_chars > max_len {
        let available_chars = max_len.saturating_sub(2);
        let keep_start = (available_chars as f32 * 0.7).round() as usize;
        let keep_end = available_chars.saturating_sub(keep_start);

        // Finding index to where execute cut
        let start_byte_idx = file_stem
            .char_indices()
            .nth(keep_start)
            .map_or(file_stem.len(), |(idx, _)| idx);

        let skip_from_end = total_chars.saturating_sub(keep_end);
        let end_byte_idx = file_stem
            .char_indices()
            .nth(skip_from_end)
            .map_or(file_stem.len(), |(idx, _)| idx);

        let start_part = &file_stem[..start_byte_idx];
        let end_part = &file_stem[end_byte_idx..];

        format!("{}…{}", start_part, end_part)
    } else {
        file_stem.to_string()
    }
    };

    if extension.is_empty() {
        truncated_stem
    } else {
        format!("{}.{}", truncated_stem, extension)
    }
}

// Storage Section
#[derive(Clone)]
struct StorageEntry {
    id: String, // Hash
    name: String,
    pdf_path: Option<PathBuf>,
    pdf_exists: bool,
    backup_path: Option<PathBuf>,
    backup_size: u64,
    notes_count: usize,
    ann_count: usize,
    orig_path: Option<PathBuf>,
    orig_size: u64,
    media_dir: Option<PathBuf>,
    audio_size: u64,
    lecture_size: u64,
    memos_size: u64,
    memo_count: usize,
}
 
impl StorageEntry {
    fn total(&self) -> u64 {
        self.backup_size + self.orig_size + self.audio_size
    }
    fn has_notes(&self) -> bool {
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
 
enum StorageAction {
    ClearNotes(usize),
    DeleteOrig(usize),
    DeleteLecture(usize),
    DeleteMemos(usize),
    DeleteLeftovers(usize),
    CleanOrphans,
    ClearStretchCache,
    ClearRecent,
}
 
fn human_size(b: u64) -> String {
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
 
fn dir_size(p: &Path) -> u64 {
    let mut total = 0;
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            if let Ok(m) = e.metadata() { //Calculates recursively the dimension of each file and subdir
                total += if m.is_dir() { dir_size(&e.path()) } else { m.len() };
            }
        }
    }
    total
}
 
fn path_key(p: &Path) -> String {
    // read-only: does not create ids for recent files that were never opened
    docid::lookup_key(p)
}
 
fn stretch_cache_dir() -> PathBuf {
    std::env::temp_dir().join("lapis_stretch")
}
 
fn resolve_media_file(dir: &Path, name: &str) -> PathBuf {
    let p = PathBuf::from(name);
    if p.is_absolute() {
        p
    } else {
        dir.join(p)
    }
}
 
fn read_audio_index(dir: &Path) -> Option<audio::AudioDocIndex> {
    std::fs::read_to_string(dir.join("audio_index.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
}
 
fn write_audio_index(dir: &Path, idx: &audio::AudioDocIndex) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(idx)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(dir.join("audio_index.json"), json)
}
 
/// Removes an audio file only if in the media folder of the file
fn remove_media_file(dir: &Path, name: &str) -> std::io::Result<()> {
    let full = resolve_media_file(dir, name);
    if !full.starts_with(dir) {
        return Ok(());
    }
    match std::fs::remove_file(&full) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
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
fn scan_storage(
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
fn storage_clear_notes(en: &StorageEntry) -> std::io::Result<()> {
    match (&en.pdf_path, en.pdf_exists) {
        (Some(p), true) => {
            notes::save_doc_data(p, &HashMap::new(), &HashMap::new(), HashSet::new(), &HashMap::new())
        }
        _ => match &en.backup_path {
            Some(b) if b.exists() => std::fs::remove_file(b),
            _ => Ok(()),
        },
    }
}
 
fn storage_delete_orig(en: &StorageEntry) -> std::io::Result<()> {
    match &en.orig_path {
        Some(o) if o.exists() => std::fs::remove_file(o),
        _ => Ok(()),
    }
}
 
/// Delete the recording of the lesson and everything that comes from it (transcript,
/// slide markers, waveform). Voice memos remain.
fn storage_delete_lecture(en: &StorageEntry) -> std::io::Result<()> {
    let Some(dir) = &en.media_dir else {
        return Ok(());
    };
    let Some(mut idx) = read_audio_index(dir) else {
        return Ok(());
    };
    if let Some(w) = idx.lecture_wav.take() {
        remove_media_file(dir, &w)?;
    }
    for l in &idx.other_lectures {
        if let Some(w) = &l.wav {
            remove_media_file(dir, w)?;
        }
    }
    idx.other_lectures.clear();
    remove_media_file(dir, "lecture_raw.wav")?;
    idx.lecture_title.clear();
    idx.lecture_duration_ms = 0;
    idx.page_range = None;
    idx.timeline.clear();
    idx.timeline_is_live = false;
    idx.segments.clear();
    idx.waveform_peaks.clear();
    write_audio_index(dir, &idx)
}
 
/// Delete all voice memos (files + index entries). The lesson remains.
fn storage_delete_memos(en: &StorageEntry) -> std::io::Result<()> {
    let Some(dir) = &en.media_dir else {
        return Ok(());
    };
    let Some(mut idx) = read_audio_index(dir) else {
        return Ok(());
    };
    for memo in idx.page_memos.values().flatten() {
        remove_media_file(dir, &memo.filename)?;
    }
    idx.page_memos.clear();
    write_audio_index(dir, &idx)
}
 
fn storage_delete_leftovers(en: &StorageEntry) -> std::io::Result<()> {
    if let Some(b) = &en.backup_path {
        if b.exists() {
            std::fs::remove_file(b)?;
        }
    }
    storage_delete_orig(en)?;
    if let Some(d) = &en.media_dir {
        if d.exists() {
            std::fs::remove_dir_all(d)?;
        }
    }
    Ok(())
}
 

// End of Storage Section


/// Button with inline confirmation: first click -> "Sure? Yes / No". Returns true upon confirmation.
fn danger_button(
    ui: &mut egui::Ui,
    confirm: &mut String,
    key: String,
    label: &str,
    tip: &str,
    enabled: bool,
) -> bool {
    if *confirm == key {
        let mut fired = false;
        ui.label(
            egui::RichText::new("Sure?")
                .color(egui::Color32::from_rgb(251, 113, 133))
                .strong(),
        );
        if ui.small_button("Yes").clicked() {
            fired = true;
            confirm.clear();
        }
        if ui.small_button("No").clicked() {
            confirm.clear();
        }
        fired
    } else {
        if ui
            .add_enabled(enabled, egui::Button::new(label))
            .on_hover_text(tip)
            .clicked()
        {
            *confirm = key;
        }
        false
    }
}


const THUMB_SCALE: f32 = 0.22;
/// Render scales are quantized so a zoom gesture does not trigger a re-render per frame.
const SCALE_STEP: f32 = 0.25;
const MIN_RENDER_SCALE: f32 = 0.5;
const MAX_RENDER_SCALE: f32 = 4.0;
/// Hard caps on a single page texture. Beyond this the GPU upscales the texture;
/// real tile rendering is the next step for very high zoom levels.
const MAX_PAGE_PIXELS: f32 = 12_000_000.0;
const MAX_TEXTURE_SIDE: f32 = 8192.0;

/// Pages to render (0-based) in priority order. A newer request always REPLACES an older
/// one (latest-wins), so the UI just describes what it needs right now.
struct RenderRequest {
    scale: f32,
    pages: Vec<i32>,
    dark_mode: bool,
}

#[derive(Clone, Debug)]
pub struct PdfLinkZone {
    /// Normalized bounding box `[x0, y0, x1, y1]` in `0.0..=1.0`
    pub rect: [f32; 4],
    /// Target 1-based page number if internal link, or external URL
    pub target_page: Option<u32>,
    pub external_uri: Option<String>,
}

struct RenderResult {
    page_idx: i32,
    /// `scale * 100`, rounded: tells the UI at which resolution this texture was rendered.
    scale_key: u32,
    /// Page size in PDF points (used to pick the render scale for the display size).
    page_pt: (f32, f32),
    /// Already converted to Color32 on the worker thread.
    image: egui::ColorImage,
    is_thumb: bool,
    dark_mode: bool,
    pub links: Vec<PdfLinkZone>,
}

/// Messages from the workers to the UI.
enum WorkerMsg {
    Page(RenderResult),
    /// Selectable text of a page. Sent once per page, right AFTER its first image, so
    /// text extraction never delays the first paint.
    Text {
        page_idx: i32,
        text: Arc<PageText>,
    },
}

#[derive(Clone)]
struct PdfCharItem {
    ch: char,
    /// Normalized bounding box (x0, y0, x1, y1) in 0.0..=1.0 of the page.
    rect: [f32; 4],
}
 
/// A visual line of text: chars `start..end` of `PageText::chars` (lines partition the
/// char array, so a selection is just a pair of indices).
#[derive(Clone)]
struct TextLine {
    start: usize,
    end: usize,
    /// Normalized bounding box of the whole line.
    bbox: [f32; 4],
}
 
#[derive(Clone, Default)]
struct PageText {
    chars: Vec<PdfCharItem>,
    lines: Vec<TextLine>,
}
 
/// A caret sits BETWEEN characters: `idx` in `0..=chars.len()` of the given page.
#[derive(Clone, Copy, PartialEq, Debug)]
struct TextCaret {
    page: u32,
    idx: usize,
}
 
#[derive(Clone, Copy, Debug)]
struct TextSelection {
    anchor: TextCaret,
    focus: TextCaret,
}
 
impl TextSelection {
    /// (start, end) in reading order, whichever way the user dragged.
    fn ordered(&self) -> (TextCaret, TextCaret) {
        let a = (self.anchor.page, self.anchor.idx);
        let f = (self.focus.page, self.focus.idx);
        if a <= f {
            (self.anchor, self.focus)
        } else {
            (self.focus, self.anchor)
        }
    }
 
    /// Char range `lo..hi` of `page` covered by this selection (if any).
    fn range_on_page(&self, page: u32, len: usize) -> Option<(usize, usize)> {
        let (s, e) = self.ordered();
        if page < s.page || page > e.page {
            return None;
        }
        let lo = if page == s.page { s.idx } else { 0 };
        let hi = if page == e.page { e.idx.min(len) } else { len };
        (lo < hi).then_some((lo, hi))
    }
}
 
impl PageText {
    /// Line closest to a point (normalized coords): vertical distance first, then
    /// horizontal distance (so multi-column layouts pick the right column).
    fn line_for_point(&self, nx: f32, ny: f32) -> Option<usize> {
        let mut best: Option<(usize, f32, f32)> = None;
        for (i, l) in self.lines.iter().enumerate() {
            let dy = if ny < l.bbox[1] {
                l.bbox[1] - ny
            } else if ny > l.bbox[3] {
                ny - l.bbox[3]
            } else {
                0.0
            };
            let dx = if nx < l.bbox[0] {
                l.bbox[0] - nx
            } else if nx > l.bbox[2] {
                nx - l.bbox[2]
            } else {
                0.0
            };
            let better = match best {
                None => true,
                Some((_, by, bx)) => dy < by - 1e-6 || ((dy - by).abs() <= 1e-6 && dx < bx),
            };
            if better {
                best = Some((i, dy, dx));
            }
        }
        best.map(|b| b.0)
    }
 
    /// Caret position (between chars) closest to a point: passing the middle of a
    /// character includes it, exactly like in a text editor.
    fn caret_at(&self, nx: f32, ny: f32) -> usize {
        let Some(li) = self.line_for_point(nx, ny) else {
            return 0;
        };
        let l = &self.lines[li];
        for i in l.start..l.end {
            let r = &self.chars[i].rect;
            if nx < (r[0] + r[2]) * 0.5 {
                return i;
            }
        }
        l.end
    }
    fn is_over_text(&self, nx: f32, ny: f32) -> bool {
        const PAD_X: f32 = 0.004;
        const PAD_Y: f32 = 0.003;

        self.chars.iter().any(|ch| {
            nx >= ch.rect[0] - PAD_X
                && nx <= ch.rect[2] + PAD_X
                && ny >= ch.rect[1] - PAD_Y
                && ny <= ch.rect[3] + PAD_Y
        })
    }
    /// Character under (or nearest to) a point, for word selection.
    fn char_at(&self, nx: f32, ny: f32) -> Option<usize> {
        let l = &self.lines[self.line_for_point(nx, ny)?];
        (l.start..l.end)
            .find(|&i| nx <= self.chars[i].rect[2])
            .or(Some(l.end.saturating_sub(1)))
            .filter(|&i| i < self.chars.len())
    }
 
    fn line_range(&self, i: usize) -> (usize, usize) {
        match self.lines.iter().find(|l| i >= l.start && i < l.end) {
            Some(l) => (l.start, l.end),
            None => (i, i),
        }
    }
 
    fn word_range(&self, i: usize) -> (usize, usize) {
        let Some(l) = self.lines.iter().find(|l| i >= l.start && i < l.end) else {
            return (i, i);
        };
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        if !is_word(self.chars[i].ch) {
            return (i, i + 1);
        }
        let mut a = i;
        while a > l.start && is_word(self.chars[a - 1].ch) {
            a -= 1;
        }
        let mut b = i + 1;
        while b < l.end && is_word(self.chars[b].ch) {
            b += 1;
        }
        (a, b)
    }
 
    /// Plain text of `lo..hi`, one '\n' per visual line.
    fn text_in(&self, lo: usize, hi: usize) -> String {
        let mut out = String::new();
        for l in &self.lines {
            let s = l.start.max(lo);
            let e = l.end.min(hi);
            if s >= e {
                continue;
            }
            if !out.is_empty() {
                out.push('\n');
            }
            let seg: String = self.chars[s..e].iter().map(|c| c.ch).collect();
            out.push_str(seg.trim_end());
        }
        out
    }
}

fn scale_key(scale: f32) -> u32 {
    (scale * 100.0).round() as u32
}

fn extract_page_links(
    doc: &Document,
    page: &mupdf::Page,
    pw: f32,
    ph: f32,
) -> Vec<PdfLinkZone> {
    let mut out = Vec::new();
    if pw <= 0.0 || ph <= 0.0 {
        return out;
    }
    if let Ok(links) = page.links() {
        for link in links {
            let b = link.bounds;
            let rect = [
                (b.x0 / pw).clamp(0.0, 1.0),
                (b.y0 / ph).clamp(0.0, 1.0),
                (b.x1 / pw).clamp(0.0, 1.0),
                (b.y1 / ph).clamp(0.0, 1.0),
            ];
            let uri = link.uri.trim();
            if uri.starts_with("http://")
                || uri.starts_with("https://")
                || uri.starts_with("mailto:")
            {
                out.push(PdfLinkZone {
                    rect,
                    target_page: None,
                    external_uri: Some(uri.to_string()),
                });
            } else {
                let parsed_p = uri
                    .strip_prefix("#page=")
                    .or_else(|| uri.strip_prefix('#'))
                    .and_then(|s| s.split(&['&', ','][..]).next())
                    .and_then(|s| s.parse::<u32>().ok())
                    .or_else(|| {
                        doc.resolve_link(uri)
                            .ok()
                            .flatten()
                            .map(|dest| dest.loc.page_number + 1)
                    });

                if let Some(tp) = parsed_p {
                    out.push(PdfLinkZone {
                        rect,
                        target_page: Some(tp),
                        external_uri: None,
                    });
                }
            }
        }
    }
    out
}

fn render_to_image(
    doc: &Document,
    idx: i32,
    scale: f32,
    dark_mode: bool,
) -> Option<((f32, f32), egui::ColorImage, Vec<PdfLinkZone>)> {
    let page = doc.load_page(idx).ok()?;
    let b = page.bounds().ok()?;
    let pw = b.x1 - b.x0;
    let ph = b.y1 - b.y0;

    let pix = page
        .to_pixmap(
            &Matrix::new_scale(scale, scale),
            &Colorspace::device_rgb(),
            false,
            false,
        )
        .ok()?;
    let size = [pix.width() as usize, pix.height() as usize];
    let samples = pix.samples();
    let image = if dark_mode {
        let mut dark_rgb = Vec::with_capacity(samples.len());
        for chunk in samples.chunks_exact(3) {
            let r = 22 + ((255 - chunk[0]) as u16 * 215 / 255) as u8;
            let g = 24 + ((255 - chunk[1]) as u16 * 215 / 255) as u8;
            let b_col = 31 + ((255 - chunk[2]) as u16 * 212 / 255) as u8;
            dark_rgb.extend_from_slice(&[r, g, b_col]);
        }
        egui::ColorImage::from_rgb(size, &dark_rgb)
    } else {
        egui::ColorImage::from_rgb(size, samples)
    };

    let links = extract_page_links(doc, &page, pw, ph);
    Some(((pw, ph), image, links))
}

/// Extracts selectable text of one page (scale independent, so done once per page).
fn extract_text(doc: &Document, idx: i32) -> Option<PageText> {
    let page = doc.load_page(idx).ok()?;
    let b = page.bounds().ok()?;
    let (pw, ph) = (b.x1 - b.x0, b.y1 - b.y0);
    let mut out = PageText::default();
    if pw <= 0.0 || ph <= 0.0 {
        return Some(out);
    }
    let Ok(tp) = page.to_text_page(mupdf::TextPageFlags::empty()) else {
        return Some(out);
    };
    for block in tp.blocks() {
        for line in block.lines() {
            let start = out.chars.len();
            let mut bbox = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
            for ci in line.chars() {
                let Some(c) = ci.char() else { continue };
                if c == '\0' || c.is_control() {
                    continue;
                }
                let q = ci.quad();
                let x0 = q.ul.x.min(q.ll.x).min(q.ur.x).min(q.lr.x) / pw;
                let x1 = q.ul.x.max(q.ll.x).max(q.ur.x).max(q.lr.x) / pw;
                let y0 = q.ul.y.min(q.ur.y).min(q.ll.y).min(q.lr.y) / ph;
                let y1 = q.ul.y.max(q.ur.y).max(q.ll.y).max(q.lr.y) / ph;
                bbox[0] = bbox[0].min(x0);
                bbox[1] = bbox[1].min(y0);
                bbox[2] = bbox[2].max(x1);
                bbox[3] = bbox[3].max(y1);
                out.chars.push(PdfCharItem {
                    ch: c,
                    rect: [x0, y0, x1, y1],
                });
            }
            let end = out.chars.len();
            if end > start {
                out.lines.push(TextLine { start, end, bbox });
            }
        }
    }
    Some(out)
}

/// One MuPDF worker with its own `Document`. Newer requests replace older ones; the check
/// happens between pages, so rapid scrolling never renders pages nobody looks at anymore.
fn spawn_worker(
    path: PathBuf,
    res_tx: Sender<WorkerMsg>,
    ctx: egui::Context,
    is_thumb: bool,
) -> Sender<RenderRequest> {
    let (req_tx, req_rx) = channel::<RenderRequest>();
 
    thread::spawn(move || {
        let Some(path_str) = path.to_str() else {
            eprintln!("Non-UTF-8 PDF path: {:?}", path);
            return;
        };
        let doc = match Document::open(path_str) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("Error opening PDF in worker: {:?}", e);
                return;
            }
        };
 
        let mut pending: Option<RenderRequest> = None;
        let mut text_done: std::collections::HashSet<i32> = std::collections::HashSet::new();
 
        'outer: loop {
            let mut req = match pending.take() {
                Some(r) => r,
                None => match req_rx.recv() {
                    Ok(r) => r,
                    Err(_) => break,
                },
            };
            while let Ok(newer) = req_rx.try_recv() {
                req = newer;
            }
            let key = scale_key(req.scale);
 
            for idx in req.pages.iter().copied() {
                if let Ok(newer) = req_rx.try_recv() {
                    pending = Some(newer);
                    continue 'outer;
                }
                if let Some((page_pt, image, links)) = render_to_image(&doc, idx, req.scale, req.dark_mode) {
                    let res = RenderResult {
                        page_idx: idx,
                        scale_key: key,
                        page_pt,
                        image,
                        is_thumb,
                        dark_mode: req.dark_mode,
                        links,
                    };
                    if res_tx.send(WorkerMsg::Page(res)).is_err() {
                        break 'outer; // UI gone / document closed
                    }
                    ctx.request_repaint();
                }
                // Text comes after the image and only once per page.
                if !is_thumb && text_done.insert(idx) {
                    if let Some(text) = extract_text(&doc, idx) {
                        let msg = WorkerMsg::Text {
                            page_idx: idx,
                            text: Arc::new(text),
                        };
                        if res_tx.send(msg).is_err() {
                            break 'outer;
                        }
                        ctx.request_repaint();
                    }
                }
            }
        }
    });
 
    req_tx
}
 
/// Page worker (low latency) + thumbnail worker (never blocks the main page).
fn spawn_render_workers(
    path: PathBuf,
    ctx: egui::Context,
) -> (Sender<RenderRequest>, Sender<RenderRequest>, Receiver<WorkerMsg>) {
    let (res_tx, res_rx) = channel::<WorkerMsg>();
    let page_tx = spawn_worker(path.clone(), res_tx.clone(), ctx.clone(), false);
    let thumb_tx = spawn_worker(path, res_tx, ctx, true);
    (page_tx, thumb_tx, res_rx)
}

fn crop_slide_region_to_png(
    pdf_path: &Path,
    page_num: u32,
    p1: (f32, f32),
    p2: (f32, f32),
    dark_mode: bool,
) -> Result<PathBuf, String> {
    let min_x = p1.0.min(p2.0).clamp(0.0, 1.0);
    let max_x = p1.0.max(p2.0).clamp(0.0, 1.0);
    let min_y = p1.1.min(p2.1).clamp(0.0, 1.0);
    let max_y = p1.1.max(p2.1).clamp(0.0, 1.0);

    if (max_x - min_x) < 0.015 || (max_y - min_y) < 0.015 {
        return Err("Selection area is too small".to_string());
    }

    let doc = Document::open(pdf_path.to_str().unwrap_or_default()).map_err(|e| e.to_string())?;
    let page = doc
        .load_page((page_num.saturating_sub(1)) as i32)
        .map_err(|e| e.to_string())?;

    // Render at crisp 2.5x resolution for high-DPI diagram clarity
    let scale = 2.5_f32;
    let pix = page
        .to_pixmap(
            &Matrix::new_scale(scale, scale),
            &Colorspace::device_rgb(),
            false,
            false,
        )
        .map_err(|e| e.to_string())?;

    let full_w = pix.width() as usize;
    let full_h = pix.height() as usize;
    let samples = pix.samples();

    let x0 = ((min_x * full_w as f32).floor() as usize).min(full_w.saturating_sub(1));
    let y0 = ((min_y * full_h as f32).floor() as usize).min(full_h.saturating_sub(1));
    let x1 = ((max_x * full_w as f32).ceil() as usize).clamp(x0 + 1, full_w);
    let y1 = ((max_y * full_h as f32).ceil() as usize).clamp(y0 + 1, full_h);

    let crop_w = x1 - x0;
    let crop_h = y1 - y0;
    let mut rgb_buf = Vec::with_capacity(crop_w * crop_h * 3);

    for row in y0..y1 {
        for col in x0..x1 {
            let offset = (row * full_w + col) * 3;
            if offset + 2 < samples.len() {
                if dark_mode {
                    let r = 22 + ((255 - samples[offset]) as u16 * 215 / 255) as u8;
                    let g = 24 + ((255 - samples[offset + 1]) as u16 * 215 / 255) as u8;
                    let b = 31 + ((255 - samples[offset + 2]) as u16 * 212 / 255) as u8;
                    rgb_buf.extend_from_slice(&[r, g, b]);
                } else {
                    rgb_buf.extend_from_slice(&samples[offset..offset + 3]);
                }
            }
        }
    }

    let snips_dir = audio::AudioDocIndex::media_dir_for_pdf(pdf_path).join("snips");
    let _ = std::fs::create_dir_all(&snips_dir);

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let filename = format!("snip_p{}_{}.png", page_num, ts);
    let out_path = snips_dir.join(filename);

    image::save_buffer(
        &out_path,
        &rgb_buf,
        crop_w as u32,
        crop_h as u32,
        image::ColorType::Rgb8,
    )
    .map_err(|e| e.to_string())?;

    Ok(out_path)
}

#[derive(PartialEq, Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
enum ZoomMode {
    Custom,
    FitWidth,
    FitHeight,
    FitPage,
}

/// Keep the content point under `anchor` fixed during zoom.


fn zoom_scroll_offset(
    old_offset: f32,
    anchor: f32,
    old_view: f32,
    new_view: f32,
    old_size: f32,
    new_size: f32,
) -> f32 {
    let pad_old = ((old_view - old_size) * 0.5).max(0.0);
    let pad_new = ((new_view - new_size) * 0.5).max(0.0);
    let u = (old_offset + anchor - pad_old).clamp(0.0, old_size.max(1.0));
    (pad_new + u * (new_size / old_size.max(1.0)) - anchor).max(0.0)
}

/// Clamp the offset to the valid scroll range.
/// egui clamps this value at the end of the frame.
fn clamp_scroll_offset(off: egui::Vec2, content: egui::Vec2, view: egui::Vec2) -> egui::Vec2 {
    let max = (content - view).max(egui::Vec2::ZERO);
    egui::vec2(off.x.clamp(0.0, max.x), off.y.clamp(0.0, max.y))
}

#[derive(Clone)]
struct TocItem {
    title: String,
    page: u32,
}

#[derive(PartialEq, Clone, Copy)]
enum NoteMode {
    Edit,
    Preview,
    /// Per-page scratchpad (see `sketch.rs`).
    Sketch,
}

/// Text of every slide (PDF text + the user's note for that slide), used to align the transcript.
fn extract_slide_texts(
    pdf_path: &Path,
    total_pages: u32,
    notes_map: &HashMap<u32, String>,
) -> Vec<String> {
    let mut slide_texts = Vec::with_capacity(total_pages as usize);
    if let Ok(doc) = Document::open(pdf_path.to_str().unwrap_or_default()) {
        for p_idx in 0..(total_pages as i32) {
            let mut combined = String::new();
            if let Ok(page) = doc.load_page(p_idx) {
                if let Ok(tp) = page.to_text_page(mupdf::TextPageFlags::empty()) {
                    for block in tp.blocks() {
                        for line in block.lines() {
                            for ch in line.chars() {
                                if let Some(c) = ch.char() {
                                    combined.push(c);
                                }
                            }
                            combined.push(' ');
                        }
                    }
                }
            }
            if let Some(user_note) = notes_map.get(&((p_idx + 1) as u32)) {
                for line in user_note.lines() {
                    let t = line.trim_start();
                    if t.starts_with("- **[") || t.contains("Lecture Transcript") { continue; }
                    combined.push(' ');
                    combined.push_str(line);
                }
            }
            slide_texts.push(combined);
        }
    }
    slide_texts
}

/// Annotation tools anchored at the bottom of the Notes panel.
#[derive(PartialEq, Clone, Copy)]
enum AnnotationTool {
    Cursor,
    Highlight,
    Underline,
    Draw,
    Shape,
    TextBox,
    Eraser,
    Tape,
    Snip,
    Laser,
}

#[derive(PartialEq, Clone, Copy)]
enum ShapeKind {
    Rect,
    Circle,
    Line,
    Arrow,
}
#[derive(PartialEq, Clone, Copy)]
enum SidebarTab {
    Thumbnails,
    Outline,
}

const ANNOTATION_PALETTE: [([u8; 3], &str); 8] = [
    ([0, 0, 0], "Black"),
    ([156, 163, 175], "Gray"),
    ([255, 255, 255], "White"),
    ([250, 204, 21], "Yellow"),
    ([248, 113, 113], "Coral Red"),
    ([74, 222, 128], "Mint Green"),
    ([56, 189, 248], "Sky Blue"),
    ([192, 132, 252], "Lavender"),
];

#[derive(Debug, PartialEq, Clone, Copy, serde::Serialize, serde::Deserialize)]
enum ViewMode {
    SinglePage,
    Continuous,
    TwoPage,
    Slideshow,
    SlideshowWithNotes,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SettingsTab {
    StudyAndTools,
    AppearanceAndTheme,
    Shortcuts,
    SystemAndData,
    Storage,
    Dictionaries,
}

#[derive(Clone, Copy)]
enum ImageDragMode {
    /// Move: normalized pointer offset from the top-left corner.
    Move { grab: (f32, f32) },
    /// Resize: keep the opposite corner fixed.
    Resize { anchor: (f32, f32) },
}
 
#[derive(Clone, Copy)]
struct ImageDrag {
    page: u32,
    idx: usize,
    mode: ImageDragMode,
}


struct Lapis {
    current_page: u32,
    target_page: u32,
    total_pages: u32,
    current_note: String,
    notes: HashMap<u32, String>,
    note_is_dirty: bool,
    pdf_path: PathBuf,
    pdf_name: String,
    texture_handle: Option<egui::TextureHandle>,
    page_dims: (u32, u32),
    show_notes: bool,
    focus_note_editor: bool,
    zoom_scale: f32,
    zoom_mode: ZoomMode,
    view_mode: ViewMode,
    prev_view_mode: ViewMode,
    continuous_scroll_to_page: Option<u32>,
    continuous_scroll_delta: f32,
    render_tx: Sender<RenderRequest>,
    thumb_tx: Sender<RenderRequest>,
    render_rx: Receiver<WorkerMsg>,
    is_rendering: bool,
    page_cache: HashMap<u32, (egui::TextureHandle, (u32, u32))>,
    /// scale_key each cached page was rendered at
    page_scale: HashMap<u32, u32>,
    cache_order: VecDeque<u32>,
    max_cache_bytes: usize,
    /// page size in PDF points and on-screen width in points (drive the render scale)
    page_pt: (f32, f32),
    display_w_pt: f32,
    /// first/last visible page (1-based, inclusive) in continuous mode
    visible_pages: (u32, u32),
    last_page_req: Option<(u32, Vec<u32>)>,
    last_thumb_req: Option<(usize, usize)>,
    show_toc: bool,
    outline: Vec<TocItem>,
    note_mode: NoteMode,
    commonmark_cache: egui_commonmark::CommonMarkCache,
    export_rx: Receiver<Result<PathBuf, String>>,
    export_tx: Sender<Result<PathBuf, String>>,
    is_exporting: bool,
    toast_message: Option<(String, std::time::Instant)>,
    clipboard: SystemClipboard,
    paste_handled_on_press: bool,
    theme: SuiteTheme,
    dialogs: Dialogs,
    pending_delete_page: Option<u32>,
    app_state: AppState,
    sidebar_tab: SidebarTab,
    user_bookmarks: Vec<state::BookmarkItem>,
    bookmark_input: String,
    last_synced_sidebar_page: u32,
    thumb_cache: HashMap<u32, (egui::TextureHandle, (u32, u32))>,
    annotation_tool: AnnotationTool,
    annotation_color: [u8; 3],
    annotations: HashMap<u32, Vec<notes::PageAnnotation>>,
    active_stroke: Option<notes::PageAnnotation>,
    stroke_width: f32,
    selected_textbox: Option<usize>,
    textbox_font_size: f32,
    textbox_bold: bool,
    textbox_italic: bool,
    /// Selectable text per page (scale independent, extracted once by the page worker).
    page_text: HashMap<u32, Arc<PageText>>,
    pdf_selection: Option<TextSelection>,
    /// True while the primary button is held during text selection.
    selecting: bool,
    /// page rectangles drawn during the previous frame (to resolve cross-page drags)
    slide_rects: Vec<(u32, egui::Rect)>,
    canvas_rect: egui::Rect,
    show_settings: bool,
    settings_tab: SettingsTab,
    rebinding_action: Option<String>,
    show_search: bool,
    focus_search_input: bool,
    search_query: String,
    pdf_dark_mode: bool,
    hidden_pages: HashSet<u32>,
    shape_kind: ShapeKind,
    trackpad_drawing_active: bool,
    audio_engine: AudioEngine,
    memo_audio_engine: AudioEngine,
    audio_index: AudioDocIndex,
    asr_rx: Receiver<audio::AsrJobResult>,
    asr_tx: Sender<audio::AsrJobResult>,
    asr_cancel: Arc<std::sync::atomic::AtomicBool>,
    /// (done_ms, total_ms) of the running lecture transcription
    asr_progress: Option<(u64, u64)>,
    asr_model_dir: Option<PathBuf>,
    is_transcribing: bool,
    audio_dock_open: bool,
    confirm_remove_lecture: bool,
    transcript_all: bool,
    transcript_last_seg: Option<usize>,
    show_align_panel: bool,
    align_from: u32,
    align_to: u32,
    note_page: u32,
    lock_note_page: bool,
    show_audio_trimmer: bool,
    trim_start_ms: u64,
    trim_end_ms: u64,
    /// `None` = trimming the main Lecture Track; `Some((page, memo_idx))` = trimming a specific Voice Memo
    trim_memo_target: Option<(u32, usize)>,
    pdf_full_text_index: HashMap<u32, String>,
    spellchecker: spellcheck::SpellChecker,
    blank_pages: std::collections::HashSet<u32>,
    /// Per-page sketch sheets shown in the notes panel "Sketch" tab.
    sketches: HashMap<u32, notes::SketchPage>,
    sketch: sketch::SketchState,
    split_view_enabled: bool,
    split_secondary_page: u32,
    split_secondary_zoom: f32,
    undo_stack: HashMap<u32, Vec<Vec<notes::PageAnnotation>>>,
    redo_stack: HashMap<u32, Vec<Vec<notes::PageAnnotation>>>,
    /// Set of `(page_num, annotation_idx)` occlusion tapes currently peeked/revealed
    revealed_tapes: std::collections::HashSet<(u32, usize)>,
    /// Temporary laser pointer points: `(normalized_pos, timestamp)`. 
    laser_trail: Vec<((f32, f32), std::time::Instant)>,
    page_links: HashMap<u32, Vec<PdfLinkZone>>,
    /// Active rectangle `[start_norm, cur_norm]` while dragging with the Snip tool
    active_snip_rect: Option<(u32, (f32, f32), (f32, f32))>,
    /// Navigation history stacks for internal PDF link jumps
    nav_back_stack: Vec<u32>,
    nav_forward_stack: Vec<u32>,
    show_tools_rail: bool,
    radial_menu_pos: Option<egui::Pos2>,
    show_flashcards_modal: bool,
    flashcards_deck: Vec<notes::Flashcard>,
    flashcard_idx: usize,
    flashcard_revealed: bool,
    flashcard_sync_slide: bool,
    /// Session score counters: `(again_hard, good, easy)`
    flashcard_stats: (u32, u32, u32),
    show_command_palette: bool,
    command_palette_query: String,
    command_palette_selected: usize,
    book_cover_mode: bool,
    zoom_anchor: Option<egui::Pos2>,
    zoom_track: (ZoomMode, f32),
    last_layout: (f32, f32, f32, f32),
    pdf_scroll_offset: egui::Vec2, 
    pdf_panning: bool,
    show_audio_handle: bool,
    show_memo_fab: bool,
    rename_memo_target: Option<(u32, usize)>,
    rename_memo_text: String,
    shots: ShotJobs,
    last_slide_rects: Vec<(u32, egui::Rect)>,
    selected_image: Option<(u32, usize)>,
    image_drag: Option<ImageDrag>,
    home_thumb_tx: Sender<(PathBuf, egui::ColorImage)>,
    home_thumb_rx: Receiver<(PathBuf, egui::ColorImage)>,
    home_thumb_cache: HashMap<PathBuf, (egui::TextureHandle, (usize, usize))>,
    home_thumb_pending: HashSet<PathBuf>,
    home_logo: Option<egui::TextureHandle>,
    audio_dock_height: f32,
    jump_lock_y: Option<f32>,
    continuous_anchor_page: u32,
    continuous_anchor_frac: f32,
    continuous_prev_layout: (f32, f32, f32),
    jump_lock_frame: u64,
    reanchor_left: u16,
    last_effective_zoom: f32,
    reanchor_page: u32, 
    reanchor_frac: f32, 
    reanchor_ref_y: f32,
    scroll_epoch: u32,
    doc_lock: Option<doc_lock::DocLock>,
    /// Fullscreen requested by the user (F11 / command palette), independent from slideshow.
    user_fullscreen: bool,
    /// Last fullscreen state sent to the window system. The command is sent only when the wanted
    /// state changes, so a compositor that is slow (or refuses) is never fought every frame.
    fullscreen_sent: Option<bool>,
    fullscreen_cmd_at: Option<std::time::Instant>,
}

impl Lapis {
    fn new(cc: &eframe::CreationContext<'_>, initial_path: Option<PathBuf>) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        cc.egui_ctx.options_mut(|opt| {
            opt.zoom_with_keyboard = false;
        });

        setup_nerd_fonts(&cc.egui_ctx);
        let theme = SuiteTheme::load_or_init();
        theme.apply(&cc.egui_ctx);
        let app_state = AppState::load();

        let (req_tx, _req_rx) = channel::<RenderRequest>();
        let (thumb_tx, _thumb_rx) = channel::<RenderRequest>();
        let (_res_tx, res_rx) = channel::<WorkerMsg>();
        let (export_tx, export_rx) = channel::<Result<PathBuf, String>>();
        let (asr_tx, asr_rx) = channel::<audio::AsrJobResult>();
        let (home_thumb_tx, home_thumb_rx) = channel::<(PathBuf, egui::ColorImage)>();
        let home_logo = image::load_from_memory(include_bytes!("../assets/icon.png"))
            .ok()
            .map(|img| {
                let rgba = img.to_rgba8();
                let size = [rgba.width() as usize, rgba.height() as usize];
                let pixels = rgba.as_flat_samples();
                cc.egui_ctx.load_texture(
                    "lapis_home_logo",
                    egui::ColorImage::from_rgba_unmultiplied(size, pixels.as_slice()),
                    egui::TextureOptions::LINEAR,
                )
            });
        let spellchecker = spellcheck::SpellChecker::new(&app_state.settings.extra_dictionaries);

        let mut app = Self {
            current_page: 1,
            target_page: 1,
            total_pages: 1,
            current_note: String::new(),
            notes: HashMap::new(),
            note_is_dirty: false,
            pdf_path: PathBuf::new(),
            pdf_name: String::new(),
            texture_handle: None,
            page_dims: (0, 0),
            show_notes: false,
            focus_note_editor: false,
            zoom_scale: 1.0,
            zoom_mode: FitWidth,
            view_mode: ViewMode::SinglePage,
            prev_view_mode: ViewMode::SinglePage,
            continuous_scroll_to_page: None,
            continuous_scroll_delta: 0.0,
            render_tx: req_tx,
            thumb_tx,
            render_rx: res_rx,
            is_rendering: false,
            page_cache: HashMap::new(),
            page_scale: HashMap::new(),
            cache_order: VecDeque::new(),
            max_cache_bytes: 384 * 1024 * 1024,
            page_pt: (0.0, 0.0),
            display_w_pt: 0.0,
            visible_pages: (0, 0),
            last_page_req: None,
            last_thumb_req: None,
            show_toc: false,
            outline: Vec::new(),
            note_mode: NoteMode::Edit,
            commonmark_cache: egui_commonmark::CommonMarkCache::default(),
            export_rx,
            export_tx,
            is_exporting: false,
            toast_message: None,
            clipboard: SystemClipboard::new(),
            paste_handled_on_press: false,
            theme,
            dialogs: Dialogs::default(),
            pending_delete_page: None,
            app_state,
            sidebar_tab: SidebarTab::Thumbnails,
            user_bookmarks: Vec::new(),
            bookmark_input: String::new(),
            last_synced_sidebar_page: 1,
            thumb_cache: HashMap::new(),
            annotation_tool: AnnotationTool::Cursor,
            annotation_color: ANNOTATION_PALETTE[0].0,
            annotations: HashMap::new(),
            active_stroke: None,
            stroke_width: 2.5,
            selected_textbox: None,
            textbox_font_size: 18.0,
            textbox_bold: false,
            textbox_italic: false,
            page_text: HashMap::new(),
            pdf_selection: None,
            selecting: false,
            slide_rects: Vec::new(),
            canvas_rect: egui::Rect::NOTHING,
            show_settings: false,
            settings_tab: SettingsTab::StudyAndTools,
            rebinding_action: None,
            show_search: false,
            focus_search_input: false,
            search_query: String::new(),
            pdf_dark_mode: false,
            hidden_pages: HashSet::new(),
            shape_kind: ShapeKind::Rect,
            trackpad_drawing_active: false,
            audio_engine: AudioEngine::new(),
            memo_audio_engine: AudioEngine::new(),
            audio_index: AudioDocIndex::default(),
            asr_rx,
            asr_tx,
            asr_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            asr_progress: None,
            asr_model_dir: None,
            is_transcribing: false,
            audio_dock_open: false,
            confirm_remove_lecture: false,
            transcript_all: false,
            transcript_last_seg: None,
            show_align_panel: false,
            align_from: 1,
            align_to: 1,
            note_page: 1,
            lock_note_page: true,
            show_audio_trimmer: false,
            trim_start_ms: 0,
            trim_end_ms: 0,
            trim_memo_target: None,
            pdf_full_text_index: HashMap::new(),
            spellchecker,
            blank_pages: HashSet::new(),
            sketches: HashMap::new(),
            sketch: sketch::SketchState::default(),
            split_view_enabled: false,
            split_secondary_page: 1,
            split_secondary_zoom: 1.0,
            undo_stack: HashMap::new(),
            redo_stack: HashMap::new(),
            revealed_tapes: HashSet::new(),
            laser_trail: Vec::new(),
            page_links: HashMap::new(),
            active_snip_rect: None,
            nav_back_stack: Vec::new(),
            nav_forward_stack: Vec::new(),
            show_tools_rail: false,
            radial_menu_pos: None,
            show_flashcards_modal: false,
            flashcards_deck: Vec::new(),
            flashcard_idx: 0,
            flashcard_revealed: false,
            flashcard_sync_slide: false,
            flashcard_stats: (0, 0, 0),
            show_command_palette: false,
            command_palette_query: String::new(),
            command_palette_selected: 0,
            book_cover_mode: true,
            zoom_anchor: None,
            zoom_track: (ZoomMode::FitWidth, 1.0),
            last_layout: (0.0, 0.0, 0.0, 0.0),
            pdf_scroll_offset: egui::Vec2::ZERO,
            pdf_panning: false,
            show_audio_handle: true,
            show_memo_fab: true,
            rename_memo_target: None,
            rename_memo_text: String::new(),
            shots: ShotJobs::default(),
            last_slide_rects: Vec::new(),
            selected_image: None,
            image_drag: None,
            home_thumb_tx,
            home_thumb_rx,
            home_thumb_cache: HashMap::new(),
            home_thumb_pending: HashSet::new(),
            home_logo,
            audio_dock_height: 180.0,
            jump_lock_y: None,
            continuous_anchor_page: 1,
            continuous_anchor_frac: 0.0,
            continuous_prev_layout: (0.0, 0.0, 0.0),
            jump_lock_frame: 0,
            reanchor_left: 0,
            last_effective_zoom: 1.0,
            reanchor_page: 1, 
            reanchor_frac: 0.0, 
            reanchor_ref_y: 0.0,
            scroll_epoch: 0,
            doc_lock: None,
            user_fullscreen: false,
            fullscreen_sent: None,
            fullscreen_cmd_at: None,
        };

        if let Some(path) = initial_path {
            app.open_pdf(path, &cc.egui_ctx);
        }

        app.apply_custom_font(&cc.egui_ctx);

        app
    }

    #[inline]
    fn has_pdf(&self) -> bool {
        !self.pdf_path.as_os_str().is_empty()
    }

    fn save_session_state(&mut self) {
        if !self.has_pdf() {
            return;
        }
        self.app_state.upsert_recent(RecentFileEntry {
            path: self.pdf_path.clone(),
            name: self.pdf_name.clone(),
            last_page: self.current_page,
            total_pages: self.total_pages,
            zoom_mode: self.zoom_mode,
            zoom_scale: self.zoom_scale,
            show_notes: self.show_notes,
            show_toc: self.show_toc,
            bookmarks: self.user_bookmarks.clone(),
            hidden_pages: self.hidden_pages.clone(),
            view_mode: self.view_mode,
            pdf_dark_mode: self.pdf_dark_mode,
        });
    }
    fn enter_custom_zoom(&mut self) {
    if self.zoom_mode != ZoomMode::Custom {
        self.zoom_scale = self.last_effective_zoom.clamp(0.3, 3.0);
        self.zoom_mode = ZoomMode::Custom;
    }
    }

    fn go_to_page(&mut self, page: u32, _ctx: &egui::Context) {
        if page < 1 || page > self.total_pages || page == self.current_page {
            self.target_page = self.current_page;
            return;
        }
        self.sync_and_save();

        if self.audio_engine.rec_target == Some(RecordingTarget::Lecture) {
            let ts = self.audio_engine.recording_elapsed_ms();
            self.audio_index.timeline.push((ts, page));
            self.audio_index.save(&self.pdf_path);
        }

        self.current_page = page;
        if !self.audio_engine.is_recording() && !self.audio_engine.is_playing() && !self.is_transcribing {
            if let Some(i) = self.audio_index.other_covering(page) {
                self.audio_engine.stop_and_release();
                self.audio_index.switch_to_other(i);
                self.transcript_last_seg = None;
                self.audio_index.save(&self.pdf_path);
            }
        }
        self.target_page = page;
        if !self.lock_note_page {
            self.note_page = page;
            self.current_note = self.notes.get(&page).cloned().unwrap_or_default();
            self.note_is_dirty = false;
        }
        
        self.selected_textbox = None;
        self.continuous_scroll_to_page = Some(page);

        // Rendering requests are issued once per frame by `pump_render_requests`.
        if let Some((handle, dims)) = self.page_cache.get(&page) {
            self.texture_handle = Some(handle.clone());
            self.page_dims = *dims;
            self.is_rendering = false;
            self.touch_cache(page);
        } else {
            self.is_rendering = true;
            // Show the low-res thumbnail instead of the previous page while rendering.
            if let Some((h, dims)) = self.thumb_cache.get(&page) {
                self.texture_handle = Some(h.clone());
                self.page_dims = *dims;
            }
        }
    }

    fn touch_cache(&mut self, page: u32) {
        self.cache_order.retain(|&p| p != page);
        self.cache_order.push_back(page);
    }

    /// Evicts least-recently-used page textures until the cache fits the byte budget.
    /// Pages the UI currently wants are never evicted.
    fn evict_cache(&mut self) {
        let protected = self.wanted_pages();
        let mut total: usize = self
            .page_cache
            .values()
            .map(|(_, (w, h))| *w as usize * *h as usize * 4)
            .sum();
        let mut i = 0;
        while total > self.max_cache_bytes && i < self.cache_order.len() {
            let p = self.cache_order[i];
            if protected.contains(&p) {
                i += 1;
                continue;
            }
            let _ = self.cache_order.remove(i);
            if let Some((_, (w, h))) = self.page_cache.remove(&p) {
                total = total.saturating_sub(w as usize * h as usize * 4);
            }
            self.page_scale.remove(&p);
        }
    }

    /// MuPDF scale needed for the current on-screen size (HiDPI aware), quantized upwards
    /// and capped by texture limits.
    fn needed_scale(&self, ppp: f32) -> f32 {
        let (pw, ph) = self.page_pt;
        if pw <= 0.0 || ph <= 0.0 || self.display_w_pt <= 0.0 {
            return 1.5;
        }
        let ideal = self.display_w_pt * ppp / pw;
        let cap = (MAX_TEXTURE_SIDE / pw.max(ph))
            .min((MAX_PAGE_PIXELS / (pw * ph)).sqrt())
            .min(MAX_RENDER_SCALE);
        let quantized = (ideal / SCALE_STEP).ceil() * SCALE_STEP;
        quantized.max(MIN_RENDER_SCALE).min(cap)
    }

    /// Pages (1-based) the UI wants, in priority order.
    fn wanted_pages(&self) -> Vec<u32> {
        fn push(v: &mut Vec<u32>, total: u32, p: i64) {
            if p >= 1 && p <= total as i64 && !v.contains(&(p as u32)) {
                v.push(p as u32);
            }
        }
        let total = self.total_pages;
        let mut v = Vec::with_capacity(12);
        let (a, b) = self.visible_pages;
        if self.view_mode == ViewMode::Continuous && a >= 1 && b >= a {
            push(&mut v, total, self.current_page as i64);
            for p in a..=b {
                push(&mut v, total, p as i64);
            }
            push(&mut v, total, b as i64 + 1);
            push(&mut v, total, a as i64 - 1);
            push(&mut v, total, b as i64 + 2);
        } else {
            let c = self.current_page as i64;
            for d in [0, 1, -1, 2] {
                push(&mut v, total, c + d);
            }
        }
        if self.split_view_enabled {
            push(&mut v, total, self.split_secondary_page as i64);
            push(&mut v, total, self.split_secondary_page as i64 + 1);
        }
        v
    }

    /// A cached texture is good enough if it is not blurrier than needed, and not so large
    /// that the GPU would have to downscale it by more than 2x (LINEAR has no mipmaps).
    fn page_is_adequate(&self, p: u32, key: u32) -> bool {
        match self.page_scale.get(&p) {
            Some(&k) if self.page_cache.contains_key(&p) => k >= key && k <= key * 2,
            _ => false,
        }
    }

    /// Called once per frame. Sends a request only when the *desired* state changed
    /// (page set or resolution), never every frame.
    fn pump_render_requests(&mut self, ctx: &egui::Context) {
        if !self.has_pdf() {
            return;
        }
        let scale = self.needed_scale(ctx.pixels_per_point());
        let key = scale_key(scale) * 2 + (self.pdf_dark_mode as u32);
        let wanted = self.wanted_pages();
        let sig = (key, wanted.clone());
        if self.last_page_req.as_ref() == Some(&sig) {
            return;
        }
        self.last_page_req = Some(sig);
        let pages: Vec<i32> = wanted
            .iter()
            .copied()
            .filter(|&p| !self.page_is_adequate(p, key))
            .map(|p| (p - 1) as i32)
            .collect();
        if !pages.is_empty() {
            let _ = self.render_tx.send(RenderRequest { scale, pages, dark_mode: self.pdf_dark_mode});
        }
    }

    /// Thumbnails for the visible sidebar rows (+ margin), closest to the centre first.
    fn request_thumbs(&mut self, first_row: usize, end_row: usize) {
        let total = self.total_pages as usize;
        let lo = first_row.saturating_sub(3);
        let hi = (end_row + 3).min(total);
        if self.last_thumb_req == Some((lo, hi)) {
            return;
        }
        self.last_thumb_req = Some((lo, hi));
        let mid = ((first_row + end_row) / 2) as i32;
        let mut pages: Vec<i32> = (lo..hi)
            .filter(|i| !self.thumb_cache.contains_key(&((*i + 1) as u32)))
            .map(|i| i as i32)
            .collect();
        if pages.is_empty() {
            return;
        }
        pages.sort_by_key(|&i| (i - mid).abs());
        let _ = self.thumb_tx.send(RenderRequest {
            scale: THUMB_SCALE,
            pages,
            dark_mode: self.pdf_dark_mode,
        });
    }

    fn current_book_spread(&self) -> (Option<u32>, Option<u32>) {
        let vis: Vec<u32> = (1..=self.total_pages)
            .filter(|p| !self.hidden_pages.contains(p))
            .collect();
        if vis.is_empty() {
            return (None, None);
        }

        let pos = vis
            .iter()
            .position(|&p| p >= self.current_page)
            .unwrap_or(vis.len() - 1);

        if self.book_cover_mode {
            // (p.1) then (1,2)->(p.2,p.3), (3,4)->(p.4,p.5)
            if pos == 0 {
                return (None, Some(vis[0]));
            }
            let left_idx = if pos % 2 == 1 { pos } else { pos - 1 };
            let left_p = vis.get(left_idx).copied();
            let right_p = vis.get(left_idx + 1).copied();
            (left_p, right_p)
        } else {
            // (1, 2)
            let left_idx = (pos / 2) * 2;
            let left_p = vis.get(left_idx).copied();
            let right_p = vis.get(left_idx + 1).copied();
            (left_p, right_p)
        }
    }

    fn prev_page(&mut self, ctx: &egui::Context) {
        if self.view_mode == ViewMode::TwoPage {
            let (left_opt, right_opt) = self.current_book_spread();
            let first_in_spread = left_opt.or(right_opt).unwrap_or(self.current_page);
            if let Some(prev_p) = (1..first_in_spread)
                .rev()
                .find(|p| !self.hidden_pages.contains(p))
            {
                self.go_to_page(prev_p, ctx);
            }
            return;
        }
        let skip_unannotated = self.view_mode == ViewMode::SlideshowWithNotes
            && self.app_state.settings.slideshow_empty_mode == SlideshowEmptyMode::SkipEmpty;

        let mut p = self.current_page.saturating_sub(1);
        while p >= 1 {
            let is_hidden = self.hidden_pages.contains(&p);
            let is_empty_skip = skip_unannotated && !self.page_has_study_content(p);

            if !is_hidden && !is_empty_skip {
                self.go_to_page(p, ctx);
                return;
            }
            p -= 1;
        }
    }

    fn next_page(&mut self, ctx: &egui::Context) {
        if self.view_mode == ViewMode::TwoPage {
            let (_, right_opt) = self.current_book_spread();
            let base = right_opt.unwrap_or(self.current_page);
            if let Some(next_p) = ((base + 1)..=self.total_pages)
                .find(|p| !self.hidden_pages.contains(p))
            {
                self.go_to_page(next_p, ctx);
            }
            return;
        }
        let skip_unannotated = self.view_mode == ViewMode::SlideshowWithNotes
            && self.app_state.settings.slideshow_empty_mode == SlideshowEmptyMode::SkipEmpty;

        let mut p = self.current_page + 1;
        while p <= self.total_pages {
            let is_hidden = self.hidden_pages.contains(&p);
            let is_empty_skip = skip_unannotated && !self.page_has_study_content(p);

            if !is_hidden && !is_empty_skip {
                self.go_to_page(p, ctx);
                return;
            }
            p += 1;
        }
    }

    fn toggle_hide_page(&mut self, page: u32, ctx: &egui::Context) {
        if self.hidden_pages.contains(&page) {
            self.hidden_pages.remove(&page);
            self.save_session_state();
        } else {
            // Prevent hiding every single page in the document
            if self.hidden_pages.len() + 1 >= self.total_pages as usize {
                self.toast_message = Some((
                    "󰅚 Cannot hide all pages".to_string(),
                    std::time::Instant::now(),
                ));
                return;
            }
            self.hidden_pages.insert(page);
            self.save_session_state();

            // If we just hid the currently viewed page, jump to the nearest visible page
            if page == self.current_page {
                let next_vis = ((page + 1)..=self.total_pages)
                    .find(|p| !self.hidden_pages.contains(p))
                    .or_else(|| (1..page).rev().find(|p| !self.hidden_pages.contains(p)));
                if let Some(target) = next_vis {
                    self.go_to_page(target, ctx);
                }
            }
        }
    }

    fn sync_and_save(&mut self) {
    if !self.has_pdf() {
        return;
    }

    if self.current_note.trim().is_empty() {
        self.notes.remove(&self.note_page);
    } else {
        self.notes
            .insert(self.note_page, self.current_note.clone());
    }

    let _ = notes::save_doc_data(&self.pdf_path, &self.notes, &self.annotations, self.blank_pages.clone(), &self.sketches);
    self.note_is_dirty = false;
}

    /// Persist notes, annotations and sketches (after a change made in the Sketch tab).
    fn save_sketch_data(&self) {
        let _ = notes::save_doc_data(
            &self.pdf_path,
            &self.notes,
            &self.annotations,
            self.blank_pages.clone(),
            &self.sketches,
        );
    }

    /// Finish a text box that is still being typed in the Sketch tab (before the page
    /// structure changes, the document is closed, ...).
    fn finish_sketch_edit(&mut self) {
        if self.sketch.commit_pending(&mut self.sketches) {
            self.save_sketch_data();
        }
    }

    /// The Sketch tab can be turned off in the advanced settings.
    fn sketch_enabled(&self) -> bool {
        self.app_state.settings.sketch_enabled
    }

    /// Show the Sketch tab (opening the notes panel if needed), or go back to Edit.
    fn toggle_sketch_tab(&mut self) {
        if !self.sketch_enabled() {
            return;
        }
        if self.show_notes && self.note_mode == NoteMode::Sketch {
            self.note_mode = NoteMode::Edit;
            self.focus_note_editor = true;
        } else {
            self.show_notes = true;
            self.note_mode = NoteMode::Sketch;
        }
    }

    /// Ctrl+Z / Ctrl+Y act on the sketch when it was the last thing drawn on.
    fn sketch_receives_undo(&self) -> bool {
        self.sketch_enabled() && self.show_notes && self.note_mode == NoteMode::Sketch && self.sketch.focus
    }

    fn start_export(&mut self, ctx: &egui::Context) {
        if self.is_exporting {
            return;
        }

        self.sync_and_save();
        self.is_exporting = true;
        self.toast_message = Some((" Exporting...".to_string(), std::time::Instant::now()));

        let out_path = self.pdf_path.with_file_name(format!(
            "{}{}.pdf",
            self.pdf_path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy(),
            self.app_state.export_settings.content.file_suffix()
        ));

        let source_path = self.pdf_path.clone();
        let notes_snapshot = self.notes.clone();
        let tx = self.export_tx.clone();
        let export_ctx = ctx.clone();
        let annotations = self.annotations.clone();
        let export_cfg = self.app_state.export_settings.clone();
        let hidden_cfg = self.hidden_pages.clone();
        // Sketches go into the export with the notes, unless the feature is turned off.
        let sketches_snapshot = if self.sketch_enabled() { self.sketches.clone() } else { HashMap::new() };

        thread::spawn(move || {
            let res = export::export_slides_with_notes(&source_path, &out_path, &notes_snapshot, &annotations, &sketches_snapshot, &export_cfg, &hidden_cfg);
            match res {
                Ok(_) => {
                    let _ = tx.send(Ok(out_path));
                }
                Err(e) => {
                    let _ = tx.send(Err(e.to_string()));
                }
            }
            export_ctx.request_repaint();
        });
    }

    fn import_notes_dialog(&mut self, ctx: &egui::Context) {
        self.sync_and_save();
        self.dialogs.open(
            ctx,
            DialogKind::ImportNotes,
            DialogSpec::open_file().filter("Notes (.md, .txt)", &["md", "txt", "markdown"]),
        );
    }

    fn finish_import_notes(&mut self, path: PathBuf) {
        if !self.has_pdf() {
            return;
        }
        self.sync_and_save();

        match notes::import_notes_from_file(&path, self.total_pages) {
            Ok(imported) => {
                let count = imported.len();
                if count == 0 {
                    self.toast_message = Some((
                        "󰅚 No slide headers found".to_string(),
                        std::time::Instant::now(),
                    ));
                    return;
                }

                for (page, text) in imported {
                    self.notes.insert(page, text);
                }

                self.current_note = self
                    .notes
                    .get(&self.current_page)
                    .cloned()
                    .unwrap_or_default();
                self.note_is_dirty = false;
                let _ = notes::save_doc_data(&self.pdf_path, &self.notes, &self.annotations, self.blank_pages.clone(), &self.sketches);

                self.show_notes = true;

                self.toast_message = Some((
                    format!("󰄬 Notes imported for {} pages", count),
                    std::time::Instant::now(),
                ));
            }
            Err(e) => {
                self.toast_message = Some((
                    format!("󰅚 Import error: {}", e),
                    std::time::Instant::now(),
                ));
            }
        }
    }

    fn export_raw_notes_dialog(&mut self, ctx: &egui::Context) {
        self.sync_and_save();

        if self.notes.is_empty() {
            self.toast_message = Some((
                "󰅚 No notes to export".to_string(),
                std::time::Instant::now(),
            ));
            return;
        }

        let default_name = format!(
            "{}_notes.md",
            self.pdf_path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
        );

        let mut spec = DialogSpec::save_file()
            .filter("Markdown", &["md"])
            .filter("Text", &["txt"])
            .file_name(default_name);
        if let Some(parent) = self.pdf_path.parent() {
            spec = spec.directory(parent);
        }
        self.dialogs.open(ctx, DialogKind::ExportNotes, spec);
    }

    fn finish_export_notes(&mut self, out_path: PathBuf) {
        if !self.has_pdf() {
            return;
        }
        match notes::export_raw_notes_md(&out_path, &self.pdf_name, &self.notes) {
            Ok(_) => {
                let fname = out_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy();
                self.toast_message = Some((
                    format!("󰄬 Saved raw notes: {}", fname),
                    std::time::Instant::now(),
                ));
            }
            Err(e) => {
                self.toast_message = Some((
                    format!("󰅚 Export error: {}", e),
                    std::time::Instant::now(),
                ));
            }
        }
    }

    fn import_theme_dialog(&mut self, ctx: &egui::Context) {
        self.dialogs.open(
            ctx,
            DialogKind::ImportTheme,
            DialogSpec::open_file().filter("Suite Theme (.json)", &["json"]),
        );
    }

    fn finish_import_theme(&mut self, path: PathBuf, ctx: &egui::Context) {
        match SuiteTheme::load_from_file(&path) {
            Ok(loaded) => {
                loaded.apply(ctx);
                let _ = loaded.save_to_file(&SuiteTheme::default_config_path());
                self.toast_message = Some((
                    format!("󰄬 Loaded theme: {}", loaded.name),
                    std::time::Instant::now(),
                ));
                self.theme = loaded;
            }
            Err(e) => {
                self.toast_message = Some((
                    format!("󰅚 Invalid theme file: {}", e),
                    std::time::Instant::now(),
                ));
            }
        }
    }

    fn export_theme_dialog(&mut self, ctx: &egui::Context) {
        let mut spec = DialogSpec::save_file()
            .filter("Suite Theme (.json)", &["json"])
            .file_name("suite_theme.json");

        if let Some(parent) = SuiteTheme::default_config_path().parent() {
            spec = spec.directory(parent);
        }

        self.dialogs.open(ctx, DialogKind::ExportTheme, spec);
    }

    fn finish_export_theme(&mut self, path: PathBuf) {
        match self.theme.save_to_file(&path) {
            Ok(_) => {
                self.toast_message = Some((
                    "󰄬 Theme exported".to_string(),
                    std::time::Instant::now(),
                ));
            }
            Err(e) => {
                self.toast_message = Some((
                    format!("󰅚 Theme export error: {}", e),
                    std::time::Instant::now(),
                ));
            }
        }
    }

    fn export_anki_dialog(&mut self, ctx: &egui::Context) {
        let default_name = format!(
            "{}_anki_cards.tsv",
            self.pdf_name.trim_end_matches(".pdf")
        );
        self.dialogs.open(
            ctx,
            DialogKind::ExportAnki,
            DialogSpec::save_file()
                .file_name(default_name)
                .filter("Anki TSV Deck", &["tsv", "txt"]),
        );
    }

    fn finish_export_anki(&mut self, path: PathBuf) {
        match notes::export_flashcards_anki_tsv(
            &path,
            &self.pdf_name,
            &self.flashcards_deck,
        ) {
            Ok(_) => {
                self.toast_message = Some((
                    format!(
                        "󰄬 Exported {} cards",
                        self.flashcards_deck.len()
                    ),
                    std::time::Instant::now(),
                ));
            }
            Err(e) => {
                self.toast_message = Some((
                    format!("󰅚 Export error: {}", e),
                    std::time::Instant::now(),
                ));
            }
        }
    }

    /// Handle the result of a non-blocking dialog.
    fn handle_dialog_result(
        &mut self,
        kind: DialogKind,
        result: Option<PathBuf>,
        ctx: &egui::Context,
    ) {
        match kind {
            DialogKind::PickAsrModelDir { retry } => match result {
                Some(dir) => {
                    if audio::is_asr_model_dir(&dir) {
                        self.asr_model_dir = Some(dir);
                        match retry {
                            AsrRetry::Lecture { wav_path } => {
                                self.start_lecture_transcription(wav_path, ctx)
                            }
                            AsrRetry::Memo { page, memo_idx, wav_path } => {
                                self.start_memo_transcription(page, memo_idx, wav_path, ctx)
                            }
                        }
                    } else {
                        self.audio_toast("󰅚 Not a Parakeet model folder");
                    }
                }
                None => {
                    self.audio_toast(format!("󰅚 Put parakeet-tdt-0.6b-v3-int8 in {}", paths::models_dir().display()));
                }
            },
            kind => {
                let Some(path) = result else {
                    return; // annullato dall'utente
                };
                match kind {
                    DialogKind::OpenPdf => self.open_pdf_user(path, ctx),
                    DialogKind::NewNotebook => {   
                        if self.has_pdf() && doc_lock::canonical(&path) == doc_lock::canonical(&self.pdf_path) {
                            self.toast_message = Some(("Cannot overwrite the open document".to_string(), std::time::Instant::now()));
                            return;
                        }
                        match notes::create_blank_study_notebook(&path) {
                            Ok(()) => self.open_pdf(path, ctx),
                            Err(e) => {
                                self.toast_message = Some((
                                    format!("󰅚 Notebook creation failed: {}", e),
                                    std::time::Instant::now(),
                                ));
                            }
                        }
                    },
                    DialogKind::ExportOriginalPdf => self.finish_download_original_pdf(path),
                    DialogKind::ExportAnki => self.finish_export_anki(path),
                    DialogKind::PickAsrModelDir { .. } => {}
                    DialogKind::ImportLectureMedia { media_dir } => {
                        self.finish_import_lecture_media(path, &media_dir)
                    }
                    DialogKind::AddDictionary => {
                        let msg = match self.add_dictionary_file(&path) {
                            Ok(name) => {
                                self.app_state.save();
                                format!("󰄬 Dictionary {} added", name)
                            }
                            Err(e) => format!("󰅚 {}", e),
                        };

                        self.toast_message = Some((msg, std::time::Instant::now()));
                    }
                    DialogKind::ExportTheme => self.finish_export_theme(path),
                    DialogKind::ImportTheme => self.finish_import_theme(path, ctx),
                    DialogKind::ExportNotes => self.finish_export_notes(path),
                    DialogKind::ImportNotes => self.finish_import_notes(path),
                }
            }
        }
    }

    // Reusable top-bar button groups.

    fn topbar_tools_button(&mut self, ui: &mut egui::Ui, sq_btn: egui::Vec2, labeled: bool) {
        if !self.app_state.settings.tools_button {
            return;
        }
        let btn_size = if labeled { egui::vec2(200.0, sq_btn.y) } else { sq_btn };
            if ui
            .add(
                egui::Button::new(if labeled { format!("{}  Annotation tools", "") } else { " ".to_string() })
                    .selected(self.show_tools_rail)
                    .min_size(btn_size),
            )
            .on_hover_text("Toggle Annotation Tools Bar")
            .clicked()
            && self.has_pdf()
        {
            if labeled { ui.close(); }
            self.show_tools_rail = !self.show_tools_rail;
        }
    }
 
    fn topbar_jump_buttons(&mut self, ui: &mut egui::Ui, sq_btn: egui::Vec2, labeled: bool) {
        let btn_size = if labeled { egui::vec2(200.0, sq_btn.y) } else { sq_btn };
        if ui
            .add_enabled(
                !self.nav_back_stack.is_empty(),
                egui::Button::new(if labeled { format!("{}  Jump back", "󰁍") } else { "󰁍".to_string() }).min_size(btn_size),
            )
            .on_hover_text("Jump Back to previous location [Alt+Backspace]")
            .clicked()
        {
            if labeled { ui.close(); }
            self.nav_back(ui.ctx());
        }
        if ui
            .add_enabled(
                !self.nav_forward_stack.is_empty(),
                egui::Button::new(if labeled { format!("{}  Jump forward", "󰁔") } else { "󰁔".to_string() }).min_size(btn_size),
            )
            .on_hover_text("Jump Forward")
            .clicked()
        {
            if labeled { ui.close(); }
            self.nav_forward(ui.ctx());
        }
    }

    fn topbar_view_mode_menu(&mut self, ui: &mut egui::Ui) {
        // View Mode Dropdown
        let view_label = match self.view_mode {
            ViewMode::SinglePage => "󰈙 Single",
            ViewMode::Continuous => "󰡏 Continuous",
            ViewMode::TwoPage => "󰂺 Book",
            ViewMode::Slideshow => "󰐨 Slideshow",
            ViewMode::SlideshowWithNotes => "󰠮 Present+Notes",
        };

        ui.menu_button(view_label, |ui| {
            ui.set_min_width(215.0);
            if ui
                .selectable_label(
                    self.view_mode == ViewMode::SinglePage,
                    "󰈙  Single Page        [Alt+1]",
                )
                .clicked()
            {
                self.view_mode = ViewMode::SinglePage;
                ui.close();
            }
            if ui
                .selectable_label(
                    self.view_mode == ViewMode::Continuous,
                    "󰡏  Continuous Scroll  [Alt+2]",
                )
                .clicked()
            {
                self.view_mode = ViewMode::Continuous;
                self.continuous_scroll_to_page = Some(self.current_page);
                ui.close();
            }

            let book_menu_label = if self.book_cover_mode {
                "󰂻  Book (2,3 Cover)   [Alt+3]"
            } else {
                "󰂺  Book (1,2 Pair)    [Alt+3]"
            };

            if ui
                .selectable_label(
                    self.view_mode == ViewMode::TwoPage,
                    book_menu_label,
                )
                .on_hover_text(
                    "Click again while in Book mode to toggle between Cover (2,3) and Pair (1,2)",
                )
                .clicked()
            {
                if self.view_mode == ViewMode::TwoPage {
                    self.book_cover_mode = !self.book_cover_mode;
                } else {
                    self.view_mode = ViewMode::TwoPage;
                }
                ui.close();
            }

            ui.separator();
            if ui
                .selectable_label(
                    self.view_mode == ViewMode::Slideshow,
                    "󰐨  Slideshow          [Alt+4]",
                )
                .clicked()
            {
                if self.view_mode != ViewMode::Slideshow && self.view_mode != ViewMode::SlideshowWithNotes {
                        self.prev_view_mode = self.view_mode;
                }
                self.view_mode = ViewMode::Slideshow;
                ui.close();
            }
            if ui
                .selectable_label(
                    self.view_mode == ViewMode::SlideshowWithNotes,
                    "󰠮  Slideshow + Notes  [Alt+5]",
                )
                .clicked()
            {
                if self.view_mode != ViewMode::Slideshow && self.view_mode != ViewMode::SlideshowWithNotes {
                        self.prev_view_mode = self.view_mode;
                    }
                self.view_mode = ViewMode::SlideshowWithNotes;
                ui.close();
            }
        })
        .response
        .on_hover_text("Change View Mode");
    }

    fn topbar_split_button(&mut self, ui: &mut egui::Ui, sq_btn: egui::Vec2, labeled: bool) {
        let btn_size = if labeled { egui::vec2(200.0, sq_btn.y) } else { sq_btn };
        if ui
            .add(
                egui::Button::new(if labeled { format!("{}  Split view", "󰯌") } else { "󰯌".to_string() })
                    .selected(self.split_view_enabled)
                    .min_size(btn_size),
            )
            .on_hover_text("Toggle PDF Split-View (view & annotate two pages side-by-side)")
            .clicked()
            && self.has_pdf()
        {
            if labeled { ui.close(); }
            self.split_view_enabled = !self.split_view_enabled;
            if self.split_view_enabled {
                self.split_secondary_page =
                    (self.current_page + 1).min(self.total_pages.max(1));
            }
        }
    }

    fn topbar_dark_button(&mut self, ui: &mut egui::Ui, sq_btn: egui::Vec2, labeled: bool) {
        let btn_size = if labeled { egui::vec2(200.0, sq_btn.y) } else { sq_btn };
        let dark_icon = if self.pdf_dark_mode { "󰖔" } else { "󰖙" };
        if ui
            .add(
                egui::Button::new(if labeled { format!("{}  PDF night mode", dark_icon) } else { dark_icon.to_string() })
                    .selected(self.pdf_dark_mode)
                    .min_size(btn_size),
            )
            .on_hover_text("Toggle PDF Dark / Night Mode")
            .clicked()
        {
            if labeled { ui.close(); }
            self.pdf_dark_mode = !self.pdf_dark_mode;
            self.page_cache.clear();
            self.page_scale.clear();
            self.thumb_cache.clear();
            self.last_page_req = None;
            self.last_thumb_req = None;
            self.save_session_state();
        }
    }

    fn topbar_search_button(&mut self, ui: &mut egui::Ui, sq_btn: egui::Vec2, labeled: bool) {
        let btn_size = if labeled { egui::vec2(200.0, sq_btn.y) } else { sq_btn };
        if ui
            .add(
                egui::Button::new(if labeled { format!("{} Search in notes", "") } else { " ".to_string() })
                    .selected(self.show_search)
                    .min_size(btn_size),
            )
            .on_hover_text(format!(
                "Search in Notes [{}]",
                self.app_state.shortcuts.search_notes.format_label()
            ))
            .clicked()
            && self.has_pdf()
        {
            if labeled { ui.close(); }
            self.show_search = !self.show_search;
            if self.show_search {
                self.focus_search_input = true;
            }
        }
    }

    fn topbar_flash_button(&mut self, ui: &mut egui::Ui, sq_btn: egui::Vec2, labeled: bool) {
        if !self.app_state.settings.flash_button {
            return;
        }
        let btn_size = if labeled { egui::vec2(200.0, sq_btn.y) } else { sq_btn };
            if ui
            .add(
         egui::Button::new(if labeled { format!("{}  Flashcards", "󰘸") } else { "󰘸 ".to_string() })
             .selected(self.show_flashcards_modal)
             .min_size(btn_size),
            )
            .on_hover_text("Flashcard & Active Recall Trainer (parses '::' and 'Q:/A:' from notes)")
            .clicked()
            && self.has_pdf()
        {
            if labeled { ui.close(); }
            if self.show_flashcards_modal {
                self.show_flashcards_modal = false;
            } else {
                self.open_flashcards_trainer();
            }
        }
    }

    fn topbar_newpage_button(&mut self, ui: &mut egui::Ui, sq_btn: egui::Vec2, labeled: bool) {
        let btn_size = if labeled { egui::vec2(200.0, sq_btn.y) } else { sq_btn };
        let response = ui
            .add(egui::Button::new(if labeled { "  New Page" } else { "" }).min_size(btn_size))
            .on_hover_text("Click: add a page with the default style · Right-click: choose page style");

        let mut chosen_style = None;
        response.context_menu(|ui| {
            ui.set_min_width(190.0);
            ui.label(egui::RichText::new("New page style").strong());
            ui.add_space(4.0);
            for (style, label, description) in [
                (notes::BlankPageStyle::Ruled, "Ruled", "Traditional horizontal lines"),
                (notes::BlankPageStyle::Dotted, "Dotted", "Subtle writing dots"),
                (notes::BlankPageStyle::Grid, "Grid", "Even square grid"),
                (notes::BlankPageStyle::Blank, "Blank", "Completely empty page"),
            ] {
                let selected = self.app_state.settings.default_page_style == style;
                let row_label = if selected { format!("●  {label}") } else { format!("    {label}") };
                let row = ui.selectable_label(selected, egui::RichText::new(row_label).strong());
                let row = row.on_hover_text(description);
                if row.clicked() {
                    chosen_style = Some(style);
                    ui.close();
                }
            }
            ui.separator();
            ui.small("Current default is marked with a dot.");
        });

        if response.clicked() {
            chosen_style = Some(self.app_state.settings.default_page_style);
        }
        if let Some(style) = chosen_style {
            if labeled { ui.close(); }
            self.insert_blank_page_after(self.current_page, style, ui.ctx());
        }
    }

    /// Render a bookmark button and return its popup anchor.
    fn topbar_bookmark_button(&mut self, ui: &mut egui::Ui, sq_btn: egui::Vec2, labeled: bool) -> egui::Rect {
        let btn_size = if labeled { egui::vec2(200.0, sq_btn.y) } else { sq_btn };
        let is_bookmarked = self
            .user_bookmarks
            .iter()
            .any(|b| b.page == self.current_page);
        let bm_icon = if is_bookmarked { "󰃀" } else { "󰃃" };
        let bm_id = ui.make_persistent_id("bookmark_dropdown_open");
        let mut bm_open =
            ui.data_mut(|d| *d.get_temp_mut_or_default::<bool>(bm_id));

        let bm_resp = ui
            .add(
                egui::Button::new(
                    egui::RichText::new(if labeled { format!("{}  Bookmark", bm_icon) } else { bm_icon.to_string() }).color(if is_bookmarked {
                        rgb(self.theme.warning)
                    } else {
                        rgb(self.theme.fg_primary)
                    }),
                )
                .selected(bm_open)
                .min_size(btn_size),
            )
            .on_hover_text("Add / Edit Bookmark for this page");

        if bm_resp.clicked() && self.has_pdf() {
            bm_open = !bm_open;
            if bm_open {
                self.bookmark_input = self
                    .user_bookmarks
                    .iter()
                    .find(|b| b.page == self.current_page)
                    .map(|b| b.title.clone())
                    .unwrap_or_else(|| format!("Page {}", self.current_page));
            }
        }

        if labeled && bm_resp.clicked() {
            ui.data_mut(|d| d.insert_temp(ui.make_persistent_id("bookmark_skip_outside"), true));
            ui.close();
        }
        ui.data_mut(|d| *d.get_temp_mut_or_default::<bool>(bm_id) = bm_open);
        bm_resp.rect
    }

    /// Render the bookmark popup at `anchor`.
    fn render_bookmark_popup(&mut self, ui: &mut egui::Ui, anchor: egui::Rect) {
        let is_bookmarked = self
            .user_bookmarks
            .iter()
            .any(|b| b.page == self.current_page);
        let bm_id = ui.make_persistent_id("bookmark_dropdown_open");
        let mut bm_open = ui.data_mut(|d| *d.get_temp_mut_or_default::<bool>(bm_id));
        if bm_open {
            let mut close_bm = false;
            let popup_pos =
                egui::pos2(anchor.center().x, anchor.bottom() + 6.0);

            let area_resp =
                egui::Area::new(ui.make_persistent_id("bookmark_popup_area"))
                    .fixed_pos(popup_pos)
                    .pivot(egui::Align2::CENTER_TOP)
                    .order(egui::Order::Foreground)
                    .show(ui.ctx(), |ui| {
                        egui::Frame::menu(ui.style())
                            .inner_margin(egui::Margin::same(10))
                            .show(ui, |ui| {
                                ui.set_width(220.0);
                                ui.label(
                                    egui::RichText::new(format!(
                                        "󰃀  Bookmark Page {}",
                                        self.current_page
                                    ))
                                    .strong()
                                    .size(13.0),
                                );
                                ui.add_space(4.0);

                                let input_resp = ui.add(
                                    egui::TextEdit::singleline(
                                        &mut self.bookmark_input,
                                    )
                                    .hint_text("Bookmark name...")
                                    .desired_width(f32::INFINITY),
                                );

                                // Auto-focus the input box when opened
                                if !input_resp.has_focus() {
                                    input_resp.request_focus();
                                }

                                let enter_pressed = input_resp.lost_focus()
                                    && ui.input(|i| i.key_pressed(egui::Key::Enter));

                                ui.add_space(6.0);
                                ui.horizontal(|ui| {
                                    if ui
                                        .add_sized(
                                            [100.0, 26.0],
                                            egui::Button::new("󰄬  Save"),
                                        )
                                        .clicked()
                                        || enter_pressed
                                    {
                                        let name = self.bookmark_input.clone();
                                        self.add_or_update_bookmark(
                                            self.current_page,
                                            name,
                                        );
                                        self.toast_message = Some((
                                            format!(
                                                "󰃀 Bookmark saved for page {}",
                                                self.current_page
                                            ),
                                            std::time::Instant::now(),
                                        ));
                                        close_bm = true;
                                    }

                                    if is_bookmarked
                                        && ui
                                            .add_sized(
                                                [90.0, 26.0],
                                                egui::Button::new("󰆴  Remove"),
                                            )
                                            .clicked()
                                    {
                                        self.remove_bookmark(self.current_page);
                                        close_bm = true;
                                    }
                                });
                            });
                    });

            // Ignore the click that opened the popup.
            let skip_outside = ui
                .data_mut(|d| d.remove_temp::<bool>(ui.make_persistent_id("bookmark_skip_outside")))
                .unwrap_or(false);
            let clicked_out = !skip_outside
                && ui.input(|i| i.pointer.any_click())
                && ui
                    .input(|i| i.pointer.interact_pos())
                    .map(|p| {
                        !anchor.contains(p)
                            && !area_resp.response.rect.contains(p)
                    })
                    .unwrap_or(false);

            if close_bm
                || clicked_out
                || ui.input(|i| i.key_pressed(egui::Key::Escape))
            {
                bm_open = false;
            }
        }
        ui.data_mut(|d| *d.get_temp_mut_or_default::<bool>(bm_id) = bm_open);
    }

fn open_pdf(&mut self, path: PathBuf, ctx: &egui::Context) {
        // Do not open a document already locked by another window.
        // Multiple instances would overwrite notes and annotations.
        let same_doc = self.has_pdf() && doc_lock::canonical(&path) == doc_lock::canonical(&self.pdf_path);
        let new_lock = if same_doc && self.doc_lock.is_some() {
            None // The current window already owns the lock.
        } else {
            match doc_lock::acquire(&path) {
                Ok(l) => Some(l),
                Err(doc_lock::LockError::Busy) => {
                    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    self.toast_message = Some((
                        format!("\"{name}\" is already open in another Lapis window"),
                        std::time::Instant::now(),
                    ));
                    return;
                }
                Err(doc_lock::LockError::Io(_)) => None, // Continue if locking is unavailable.
            }
        };

        if self.has_pdf() {
            self.sync_and_save();
            self.save_session_state();
        }
        if let Some(l) = new_lock {
            self.doc_lock = Some(l); // Drop the previous document lock.
        }

        let (total_pages, outline, first_page_pt) =
            match Document::open(path.to_str().unwrap_or_default()) {
                Ok(doc) => {
                    let pages = doc.page_count().unwrap_or(1) as u32;
                    let mut items = Vec::new();

                    if let Ok(outlines_iter) = doc.outlines() {
                        for node in outlines_iter {
                            let page_idx: i32 = node
                                .dest
                                .as_ref()
                                .map(|d| d.loc.page_number as i32)
                                .unwrap_or(0);
                            let p: u32 = (page_idx.max(0) as u32) + 1;
                            items.push(TocItem {
                                title: node.title.clone(),
                                page: p,
                            });
                        }
                    }
                    let pt = doc
                        .load_page(0)
                        .and_then(|p| p.bounds())
                        .map(|b| (b.x1 - b.x0, b.y1 - b.y0))
                        .unwrap_or((0.0, 0.0));
                    (pages, items, pt)
                }
                Err(_) => (1, Vec::new(), (0.0, 0.0)),
            };

        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Unknown".to_string());

        let doc_data = notes::load_doc_data(&path);
        self.notes = doc_data.notes;
        self.annotations = doc_data.annotations;
        self.active_stroke = None;
        self.selected_textbox = None;
        self.pdf_full_text_index.clear();
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.revealed_tapes.clear();
        self.laser_trail.clear();
        self.blank_pages = doc_data.blank_pages;
        self.sketches = doc_data.sketches;
        self.sketch.reset();
        let saved_entry = self.app_state.find_entry(&path).cloned();
        let start_page = saved_entry
            .as_ref()
            .map(|e| e.last_page.clamp(1, total_pages))
            .unwrap_or(1);

        if let Some(entry) = saved_entry {
            self.zoom_mode = entry.zoom_mode;
            self.zoom_scale = entry.zoom_scale;
            self.show_notes = if self.app_state.settings.restore_show_notes {entry.show_notes} else {false};
            self.show_toc = if self.app_state.settings.restore_show_toc {entry.show_toc} else {false};
            self.user_bookmarks = entry.bookmarks;
            self.pdf_dark_mode = entry.pdf_dark_mode;
            self.hidden_pages = entry.hidden_pages;
            self.view_mode = entry.view_mode;
        } else {
            self.user_bookmarks.clear();
            self.hidden_pages.clear();
        }

        if self.pdf_path != path {
            self.audio_engine.stop_and_release();
            self.memo_audio_engine.stop_and_release();
            self.asr_cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
            self.is_transcribing = false;
            self.asr_progress = None;
            self.transcript_last_seg = None;
            self.show_audio_trimmer = false;
            self.trim_memo_target = None;
        }
        self.audio_index = AudioDocIndex::load(&path);

        self.thumb_cache.clear();
        self.pending_delete_page = None;
        self.last_layout = (0.0, 0.0, 0.0, 0.0);
        self.page_scale.clear();
        self.last_page_req = None;
        self.last_thumb_req = None;
        self.visible_pages = (0, 0);
        self.pdf_path = path.clone();
        self.pdf_name = name;
        self.total_pages = total_pages;
        self.outline = outline;
        self.current_page = start_page;
        self.target_page = start_page;
        self.current_note = self.notes.get(&start_page).cloned().unwrap_or_default();
        self.note_is_dirty = false;
        self.page_cache.clear();
        self.page_text.clear();
        self.pdf_selection = None;
        self.selecting = false;
        self.cache_order.clear();
        self.texture_handle = None;
        self.is_rendering = true;
        self.note_page = start_page;
        self.lock_note_page = false;
        self.audio_dock_open = false;
        self.audio_dock_height = 0.0;

        self.page_pt = first_page_pt;
        let (page_tx, thumb_tx, res_rx) = spawn_render_workers(path, ctx.clone());
        self.render_tx = page_tx;
        self.thumb_tx = thumb_tx;
        self.render_rx = res_rx;
        self.continuous_anchor_page = 1;
        self.continuous_anchor_frac = 0.0;
        self.continuous_prev_layout = (0.0, 0.0, 0.0);
        self.jump_lock_y = None;

        self.save_session_state();
        ctx.request_repaint();
    }

    /// Open a user-requested document unless it is already open.
    fn open_pdf_user(&mut self, path: PathBuf, ctx: &egui::Context) {
        if self.has_pdf() && doc_lock::canonical(&path) == doc_lock::canonical(&self.pdf_path) {
            self.toast_message = Some(("This file is already open".to_string(), std::time::Instant::now()));
            return;
        }
        self.open_pdf(path, ctx);
    }

    fn close_pdf(&mut self) {
        if !self.has_pdf() {
            return;
        }
        self.finish_sketch_edit();
        self.sync_and_save();
        self.save_session_state();

        // Release the per-document lock, otherwise the file stays marked as open
        // (other windows get "already open" and the storage cleanup stays disabled).
        self.doc_lock = None;

        self.pdf_path = PathBuf::new();
        self.pdf_name.clear();
        self.texture_handle = None;
        self.page_cache.clear();
        self.page_text.clear();
        self.pdf_selection = None;
        self.selecting = false;
        self.cache_order.clear();
        self.thumb_cache.clear();
        self.page_scale.clear();
        self.last_page_req = None;
        self.last_thumb_req = None;
        // Drop the channels so the worker threads exit and release the Document.
        self.render_tx = channel::<RenderRequest>().0;
        self.thumb_tx = channel::<RenderRequest>().0;
        self.render_rx = channel::<WorkerMsg>().1;
        self.notes.clear();
        self.annotations.clear();
        self.sketches.clear();
        self.sketch.reset();
        self.current_note.clear();
        self.selected_textbox = None;
        self.is_rendering = false;
        self.pdf_full_text_index.clear();
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.revealed_tapes.clear();
        self.laser_trail.clear();
    }

    fn add_or_update_bookmark(&mut self, page: u32, title: String) {
        let clean = if title.trim().is_empty() {
            format!("Page {}", page)
        } else {
            title.trim().to_string()
        };
        if let Some(existing) = self.user_bookmarks.iter_mut().find(|b| b.page == page) {
            existing.title = clean;
        } else {
            self.user_bookmarks.push(state::BookmarkItem { title: clean, page });
            self.user_bookmarks.sort_unstable_by_key(|b| b.page);
        }
        self.save_session_state();
    }

    fn remove_bookmark(&mut self, page: u32) {
        self.user_bookmarks.retain(|b| b.page != page);
        self.save_session_state();
    }
    
fn paint_annotation(
    painter: &egui::Painter,
    slide_rect: egui::Rect,
    ann: &notes::PageAnnotation,
) {
    let to_screen = |(nx, ny): (f32, f32)| -> egui::Pos2 {
        egui::pos2(
            slide_rect.left() + nx * slide_rect.width(),
            slide_rect.top() + ny * slide_rect.height(),
        )
    };

    let base_col = rgb(ann.color);

    // Unified stroke thickness for all annotations.
    let thickness = ann.stroke_width.clamp(1.0, 18.0);

    match ann.kind {
        // ============================================================
        // HIGHLIGHT
        // Filled rectangle: no adjustable stroke.
        // ============================================================
        notes::AnnotationKind::Highlight => {
            if ann.points.len() >= 2 {
                let p1 = to_screen(ann.points[0]);
                let p2 = to_screen(*ann.points.last().unwrap());
                let rect = egui::Rect::from_two_pos(p1, p2);

                painter.rect_filled(
                    rect,
                    egui::CornerRadius::same(2),
                    egui::Color32::from_rgba_unmultiplied(
                        base_col.r(),
                        base_col.g(),
                        base_col.b(),
                        95,
                    ),
                );
            }
        }

        // ============================================================
        // UNDERLINE
        // ============================================================
        notes::AnnotationKind::Underline => {
            if ann.points.len() >= 2 {
                let p1 = to_screen(ann.points[0]);
                let p2 = to_screen(*ann.points.last().unwrap());

                let start = egui::pos2(p1.x.min(p2.x), p1.y);
                let end = egui::pos2(p1.x.max(p2.x), p1.y);

                painter.line_segment(
                    [start, end],
                    egui::Stroke::new(thickness, base_col),
                );
            }
        }

        // ============================================================
        // FREEHAND / DRAW
        // ============================================================
        notes::AnnotationKind::Freehand => {
            if ann.points.len() >= 2 {
                let pts: Vec<egui::Pos2> =
                    ann.points.iter().copied().map(to_screen).collect();

                painter.add(egui::Shape::line(
                    pts,
                    egui::Stroke::new(thickness, base_col),
                ));
            }
        }

        // ============================================================
        // TEXT BOX
        // ============================================================
        notes::AnnotationKind::TextBox => {}

        // ============================================================
        // RECTANGLE
        // ============================================================
        notes::AnnotationKind::Rect => {
            if ann.points.len() >= 2 {
                let p1 = to_screen(ann.points[0]);
                let p2 = to_screen(*ann.points.last().unwrap());
                let rect = egui::Rect::from_two_pos(p1, p2);

                painter.rect_stroke(
                    rect,
                    egui::CornerRadius::same(2),
                    egui::Stroke::new(thickness, base_col),
                    egui::StrokeKind::Middle,
                );
            }
        }

        // ============================================================
        // CIRCLE
        // ============================================================
        notes::AnnotationKind::Circle => {
            if ann.points.len() >= 2 {
                let p1 = to_screen(ann.points[0]);
                let p2 = to_screen(*ann.points.last().unwrap());
                let rect = egui::Rect::from_two_pos(p1, p2);

                let radius = rect.width().min(rect.height()) * 0.5;

                painter.circle_stroke(
                    rect.center(),
                    radius,
                    egui::Stroke::new(thickness, base_col),
                );
            }
        }

        // ============================================================
        // LINE
        // ============================================================
        notes::AnnotationKind::Line => {
            if ann.points.len() >= 2 {
                let p1 = to_screen(ann.points[0]);
                let p2 = to_screen(*ann.points.last().unwrap());

                painter.line_segment(
                    [p1, p2],
                    egui::Stroke::new(thickness, base_col),
                );
            }
        }

        // ============================================================
        // ARROW
        // ============================================================
        notes::AnnotationKind::Arrow => {
            if ann.points.len() >= 2 {
                let p1 = to_screen(ann.points[0]);
                let p2 = to_screen(*ann.points.last().unwrap());

                // Main arrow line
                painter.line_segment(
                    [p1, p2],
                    egui::Stroke::new(thickness, base_col),
                );

                // Arrow head
                let dir = p2 - p1;
                let len = dir.length();

                if len > 4.0 {
                    let u = dir / len;
                    let perp = egui::vec2(-u.y, u.x);

                    let head_len = (thickness * 4.2).clamp(9.0, 18.0);

                    let wing1 =
                        p2 - u * head_len + perp * (head_len * 0.55);

                    let wing2 =
                        p2 - u * head_len - perp * (head_len * 0.55);

                    painter.line_segment(
                        [p2, wing1],
                        egui::Stroke::new(thickness, base_col),
                    );

                    painter.line_segment(
                        [p2, wing2],
                        egui::Stroke::new(thickness, base_col),
                    );
                }
            }
        }

        // ============================================================
        // OCCLUSION TAPE
        // ============================================================
        notes::AnnotationKind::Tape => {
            if ann.points.len() >= 2 {
                let p1 = to_screen(ann.points[0]);
                let p2 = to_screen(*ann.points.last().unwrap());
                let rect = egui::Rect::from_two_pos(p1, p2);

                painter.rect_filled(
                    rect,
                    egui::CornerRadius::same(4),
                    egui::Color32::from_rgba_unmultiplied(
                        ann.color[0],
                        ann.color[1],
                        ann.color[2],
                        248,
                    ),
                );

                painter.rect_stroke(
                    rect,
                    egui::CornerRadius::same(4),
                    egui::Stroke::new(
                        thickness,
                        egui::Color32::from_white_alpha(180),
                    ),
                    egui::StrokeKind::Inside,
                );

                if rect.width() > 45.0 && rect.height() > 16.0 {
                    painter.text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "󰈈 Peek",
                        egui::FontId::proportional(11.5),
                        egui::Color32::from_black_alpha(190),
                    );
                }
            }
        }

        // ============================================================
        // IMAGE
        // ============================================================
        notes::AnnotationKind::Image => {
            // Images are rendered separately by image_annotations_ui.
        }
    }
}

fn render_page_textboxes(&mut self, ui: &mut egui::Ui, page: u32, page_rect: egui::Rect) {
    const MIN_W: f32 = 0.04; // Page-width fraction.
    const MIN_H: f32 = 0.02;
    const DEF_W: f32 = 0.16;
    const DEF_H: f32 = 0.04;
    const HIT_PAD: f32 = 6.0; // Text hit-area padding.
    const RING: f32 = 10.0; // Drag handle size.

    struct Geo {
        idx: usize,
        p0: (f32, f32),
        p1: (f32, f32),
        text: String,
        pad_i8: i8,
        pad: f32,
        box_w: f32,
        font_id: egui::FontId,
        col: egui::Color32,
        bold: bool,
        italic: bool,
        fs: f32,
        color: [u8; 3],
        bg_mode: u8,
        galley: std::sync::Arc<egui::Galley>,
        rect: egui::Rect,
        hit: egui::Rect,
    }

    // Costruisce il LayoutJob supportando corsivo globale e mini-formattazione inline:
    // - Replace a leading "- " with "• ".
    // - __text__ -> underline
    // - ~~text~~ -> strikethrough
    fn build_text_job(
        raw: &str,
        font_id: egui::FontId,
        col: egui::Color32,
        italic: bool,
        wrap_w: f32,
        render_mode: bool,
    ) -> egui::text::LayoutJob {
        let raw = raw
            .strip_prefix('\u{200B}')
            .or_else(|| raw.strip_prefix('\u{200C}'))
            .unwrap_or(raw);
        let mut job = egui::text::LayoutJob::default();
        job.wrap.max_width = wrap_w;

        let base_fmt = egui::TextFormat {
            font_id,
            color: col,
            italics: italic,
            ..Default::default()
        };

        // Preserve character positions while editing.
        // Replace a leading "- " with "• " in static mode.
        let processed: std::borrow::Cow<'_, str> = if render_mode {
            let mut out = String::with_capacity(raw.len());
            for (li, line) in raw.split('\n').enumerate() {
                if li > 0 {
                    out.push('\n');
                }
                if let Some(rest) = line.strip_prefix("- ") {
                    out.push_str("• ");
                    out.push_str(rest);
                } else {
                    out.push_str(line);
                }
            }
            std::borrow::Cow::Owned(out)
        } else {
            std::borrow::Cow::Borrowed(raw)
        };

        // Parse underline and strikethrough without changing text length.
        // Keep block layout unchanged.
        let s = processed.as_ref();
        let mut i = 0;
        let bytes = s.as_bytes();
        let mut cur_start = 0;
        let mut underline = false;
        let mut strikethrough = false;

        let make_fmt = |u: bool, st: bool| {
            let mut f = base_fmt.clone();
            if u {
                f.underline = egui::Stroke::new(1.2, col);
            }
            if st {
                f.strikethrough = egui::Stroke::new(1.2, col);
            }
            f
        };

        while i + 1 < bytes.len() {
            let is_u = bytes[i] == b'_' && bytes[i + 1] == b'_';
            let is_s = bytes[i] == b'~' && bytes[i + 1] == b'~';
            if is_u || is_s {
                if cur_start < i {
                    job.append(&s[cur_start..i], 0.0, make_fmt(underline, strikethrough));
                }
                if render_mode {
                    // Hide __ and ~~ delimiters in static mode.
                    if is_u {
                        underline = !underline;
                    } else {
                        strikethrough = !strikethrough;
                    }
                    i += 2;
                    cur_start = i;
                } else {
                    // Dim delimiters while editing to preserve cursor indices.
                    let mut delim_fmt = base_fmt.clone();
                    delim_fmt.color = col.gamma_multiply(0.45);
                    if is_u {
                        underline = !underline;
                    } else {
                        strikethrough = !strikethrough;
                    }
                    job.append(&s[i..i + 2], 0.0, delim_fmt);
                    i += 2;
                    cur_start = i;
                }
            } else {
                // Avanza al prossimo char boundary UTF-8
                i += s[i..].chars().next().map_or(1, |c| c.len_utf8());
            }
        }
        if cur_start < s.len() {
            job.append(&s[cur_start..], 0.0, make_fmt(underline, strikethrough));
        }
        if s.is_empty() {
            job.append("", 0.0, base_fmt);
        }
        job
    }

    let interactive = matches!(
        self.annotation_tool,
        AnnotationTool::Cursor | AnnotationTool::TextBox
    );
    let accent = rgb(self.theme.accent);
    let bg_elev = rgb(self.theme.bg_elevated);
    let fg = rgb(self.theme.fg_primary);

    let pw = page_rect.width().max(1.0);
    let ph = page_rect.height().max(1.0);
    let font_scale = ph / 800.0;
    let to_screen =
        |p: (f32, f32)| egui::pos2(page_rect.left() + p.0 * pw, page_rect.top() + p.1 * ph);

    let mut dirty = false;
    let mut just_created_this_frame = false;

    // Selection state: page, index, and editing status.
    let sel_key = ui.make_persistent_id("tb_sel_state");
    let edit_prev_key = ui.make_persistent_id("tb_prev_edit");
    let mut sel: Option<(u32, usize, bool)> = ui.data(|d| d.get_temp(sel_key));

    if let Some((sp, si, _)) = sel {
        let ok = self
            .annotations
            .get(&sp)
            .and_then(|v| v.get(si))
            .map_or(false, |a| a.kind == notes::AnnotationKind::TextBox);
        if !ok {
            sel = None;
        }
    }
    if !interactive {
        sel = None;
    }

    // Remove empty, unselected blocks.
    {
        let sel_here = sel.filter(|s| s.0 == page).map(|s| s.1);
        let empties: Vec<usize> = self
            .annotations
            .get(&page)
            .map(|v| {
                v.iter()
                    .enumerate()
                    .filter(|(i, a)| {
                        a.kind == notes::AnnotationKind::TextBox
                            && a.text.trim_matches(['\u{200B}', '\u{200C}']).trim().is_empty() 
                            && Some(*i) != sel_here
                    })
                    .map(|(i, _)| i)
                    .collect()
            })
            .unwrap_or_default();
        if !empties.is_empty() {
            if let Some(list) = self.annotations.get_mut(&page) {
                for &i in empties.iter().rev() {
                    list.remove(i);
                }
            }
            if let Some((sp, si, ed)) = sel {
                if sp == page {
                    let shift = empties.iter().filter(|&&e| e < si).count();
                    sel = Some((sp, si - shift, ed));
                }
            }
            dirty = true;
        }
    }

    // ───────── 1. Creazione (strumento Text) ─────────
    if interactive && self.annotation_tool == AnnotationTool::TextBox {
        let bg = ui.interact(
            page_rect,
            ui.make_persistent_id(("tb_create", page)),
            egui::Sense::click_and_drag(),
        );
        if bg.hovered() || bg.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        let start_id = ui.make_persistent_id(("tb_create_start", page));

        if bg.drag_started() {
            if let Some(p) = bg.interact_pointer_pos() {
                ui.data_mut(|d| d.insert_temp(start_id, p));
            }
        }
        let start: Option<egui::Pos2> = ui.data(|d| d.get_temp(start_id));

        if bg.dragged() {
            if let (Some(a), Some(b)) = (start, bg.interact_pointer_pos()) {
                ui.painter().rect_stroke(
                    egui::Rect::from_two_pos(a, b),
                    egui::CornerRadius::same(3),
                    egui::Stroke::new(1.5, accent),
                    egui::StrokeKind::Outside,
                );
            }
        }

        let created: Option<egui::Rect> = if bg.drag_stopped() {
            ui.data_mut(|d| d.remove::<egui::Pos2>(start_id));
            match (start, bg.interact_pointer_pos()) {
                (Some(a), Some(b)) => Some(egui::Rect::from_two_pos(a, b)),
                _ => None,
            }
        } else if bg.clicked() {
            bg.interact_pointer_pos()
                .map(|p| egui::Rect::from_min_size(p, egui::vec2(0.0, 0.0)))
        } else {
            None
        };

        if let Some(mut r) = created {
            if r.width() < 24.0 || r.height() < 16.0 {
                r = egui::Rect::from_min_size(r.min, egui::vec2(DEF_W * pw, DEF_H * ph));
            }
            let w = (r.width() / pw).clamp(MIN_W, 1.0);
            let h = (r.height() / ph).clamp(MIN_H, 1.0);
            let nx = ((r.left() - page_rect.left()) / pw).clamp(0.0, 1.0 - w);
            let ny = ((r.top() - page_rect.top()) / ph).clamp(0.0, 1.0 - h);

            self.push_undo_snapshot(page);
            let ann = notes::PageAnnotation {
                kind: notes::AnnotationKind::TextBox,
                color: self.annotation_color,
                points: vec![(nx, ny), (nx + w, ny + h)],
                text: String::new(),
                font_size: self.textbox_font_size,
                bold: self.textbox_bold,
                italic: self.textbox_italic,
                stroke_width: self.stroke_width
            };
            let list = self.annotations.entry(page).or_default();
            list.push(ann);
            let new_idx = list.len() - 1;
            sel = Some((page, new_idx, true)); // Enter edit mode immediately.
            ui.data_mut(|d| d.remove::<(u32, usize)>(edit_prev_key)); // Focus immediately.
            just_created_this_frame = true;
            self.annotation_tool = AnnotationTool::Cursor;
            dirty = true;
        }
    }

    // 2. Geometry for all page blocks.
    let n = self.annotations.get(&page).map(|v| v.len()).unwrap_or(0);
    let mut geos: Vec<Geo> = Vec::new();
    let sel_idx = sel.filter(|s| s.0 == page).map(|s| s.1);
    let editing_here = sel.filter(|s| s.0 == page).map_or(false, |s| s.2);

    for idx in 0..n {
        let (p0, p1, text, fs, bold, italic, color) = {
            let Some(ann) = self.annotations.get(&page).and_then(|v| v.get(idx)) else {
                break;
            };
            if ann.kind != notes::AnnotationKind::TextBox {
                continue;
            }
            let p0 = ann.points.first().copied().unwrap_or((0.1, 0.1));
            let p1 = ann
                .points
                .get(1)
                .copied()
                .unwrap_or((p0.0 + DEF_W, p0.1 + DEF_H));
            (
                p0,
                p1,
                ann.text.clone(),
                ann.font_size,
                ann.bold,
                ann.italic,
                ann.color,
            )
        };

        let screen_fs = (fs * font_scale).clamp(6.0, 200.0);
        // Convert the integer padding to f32 to keep layout coordinates pixel-aligned.
        let pad_i8 = (screen_fs * 0.35).round().clamp(2.0, 64.0) as i8;
        let pad = f32::from(pad_i8);

        let tl = to_screen(p0);
        let br = to_screen(p1);
        let box_w = (br.x - tl.x).max(pw * MIN_W);
        let wrap_w = (box_w - 2.0 * pad).max(10.0);
        let font_id = egui::FontId::monospace(screen_fs);
        let col = rgb(color);

        let is_editing_this = sel_idx == Some(idx) && editing_here;
        let bg_mode = if text.starts_with('\u{200B}') {
            1
        } else if text.starts_with('\u{200C}') {
            2
        } else {
            0
        };
        let job = build_text_job(&text, font_id.clone(), col, italic, wrap_w, !is_editing_this);
        let galley = ui.fonts_mut(|f| f.layout_job(job));

        let box_h = (br.y - tl.y)
            .max(galley.size().y + 2.0 * pad)
            .max(ph * MIN_H);
        let rect = egui::Rect::from_min_size(tl, egui::vec2(box_w, box_h));

        let hit_w = (galley.size().x + 2.0 * pad).min(box_w).max(16.0);
        let hit_h = (galley.size().y + 2.0 * pad).min(box_h);
        let hit = egui::Rect::from_min_size(tl, egui::vec2(hit_w, hit_h)).expand(HIT_PAD);

        geos.push(Geo {
            idx,
            p0,
            p1,
            text,
            pad_i8,
            pad,
            box_w,
            font_id,
            col,
            bold,
            italic,
            fs,
            color,
            bg_mode,
            galley,
            rect,
            hit,
        });
    }

    let mut order: Vec<usize> = (0..geos.len()).collect();
    order.sort_by(|&a, &b| {
        let (ga, gb) = (&geos[a], &geos[b]);
        let (sa, sb) = (Some(ga.idx) == sel_idx, Some(gb.idx) == sel_idx);
        sa.cmp(&sb).then_with(|| {
            gb.hit
                .area()
                .partial_cmp(&ga.hit.area())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    });

    let mut sel_rect: Option<egui::Rect> = None;
    let mut toolbar_rect: Option<egui::Rect> = None;

    // Scale the offset with the font size to emphasize bold text.
    let draw_bold_galley =
        |painter: &egui::Painter, pos: egui::Pos2, galley: & std::sync::Arc<egui::Galley>, col: egui::Color32, fs: f32| {
            let off = (fs * 0.045).clamp(0.6, 2.2);
            painter.galley(pos + egui::vec2(off * 0.5, 0.0), galley.clone(), col);
            painter.galley(pos + egui::vec2(off, 0.0), galley.clone(), col);
        };

    // 3. Drawing and interaction.
    for gi in order {
        let g = &geos[gi];
        let idx = g.idx;
        let selected = sel_idx == Some(idx);
        let editing = selected && editing_here;
        let rect = g.rect;

        let mut move_delta = egui::Vec2::ZERO;
        let mut resize_to: Option<egui::Pos2> = None;

        if g.bg_mode != 0 {
            let bg_fill = if g.bg_mode == 1 {
                // First click: dark translucent gray.
                egui::Color32::from_rgba_unmultiplied(36, 38, 46, 200)
            } else {
                // Second click: light translucent gray.
                egui::Color32::from_rgba_unmultiplied(245, 246, 250, 215)
            };
            let bg_rect = if selected || editing { rect } else { g.hit.shrink(HIT_PAD) };
            ui.painter().rect_filled(bg_rect, egui::CornerRadius::same(4), bg_fill);
        }

        // Static text when not editing.
        if !editing {
            let painter = ui.painter();
            let pos = rect.min + egui::vec2(g.pad, g.pad);
            painter.galley(pos, g.galley.clone(), g.col);
            if g.bold {
                draw_bold_galley(painter, pos, &g.galley, g.col, g.font_id.size);
            }
        }

        if !interactive {
            continue;
        }

        // Unselected block.
        if !selected {
            let resp = ui.interact(
                g.hit,
                ui.make_persistent_id(("tb_hit", page, idx)),
                egui::Sense::click_and_drag(),
            );
            let text_rect = g.hit.shrink(HIT_PAD);
            let hot = resp.hovered() || resp.dragged();
            if hot {
                ui.painter().rect_stroke(
                text_rect,
                egui::CornerRadius::same(3),
                egui::Stroke::new(1.0, accent.gamma_multiply( 0.95)),
                egui::StrokeKind::Outside,
            );
                ui.ctx().set_cursor_icon(if resp.dragged() {
                    egui::CursorIcon::Grabbing
                } else {
                    egui::CursorIcon::Grab
                });
            }

            if resp.double_clicked() {
                sel = Some((page, idx, true));
            } else if resp.drag_started() {
                self.push_undo_snapshot(page);
                sel = Some((page, idx, false));
            } else if resp.clicked() {
                sel = Some((page, idx, false));
            }
            if resp.dragged() {
                move_delta += resp.drag_delta();
            }
        } else {
            // Selected block.
            sel_rect = Some(rect);

            ui.painter().rect_stroke(
                rect,
                egui::CornerRadius::same(3),
                egui::Stroke::new(1.5, accent),
                egui::StrokeKind::Outside,
            );

            if !editing {
                let mv = ui.interact(
                    rect.expand(HIT_PAD),
                    ui.make_persistent_id(("tb_move", page, idx)),
                    egui::Sense::click_and_drag(),
                );
                if mv.hovered() || mv.dragged() {
                    ui.ctx().set_cursor_icon(if mv.dragged() {
                        egui::CursorIcon::Grabbing
                    } else {
                        egui::CursorIcon::Grab
                    });
                }
                if mv.drag_started() {
                    self.push_undo_snapshot(page);
                }
                if mv.dragged() {
                    move_delta += mv.drag_delta();
                }
                if mv.clicked() {
                    sel = Some((page, idx, true));
                }
            }

            // Editing the block.
            if editing {
                let te_id = ui.make_persistent_id(("tb_text", page, idx));
                let prev_edit: Option<(u32, usize)> = ui.data(|d| d.get_temp(edit_prev_key));
                let need_init_cursor = prev_edit != Some((page, idx));
                if prev_edit != Some((page, idx)) {
                    ui.ctx().memory_mut(|m| m.request_focus(te_id));
                    let mut st = egui::text_edit::TextEditState::load(ui.ctx(), te_id)
                        .unwrap_or_default();
                    st.cursor.set_char_range(Some(egui::text::CCursorRange::one(
                        egui::text::CCursor::new(g.text.chars().count()),
                    )));
                    st.store(ui.ctx(), te_id);
                }

                if let Some(ann) = self
                    .annotations
                    .get_mut(&page)
                    .and_then(|v| v.get_mut(idx))
                {
                    let layout_font = g.font_id.clone();
                    let (col, italic) = (g.col, g.italic);
                    let mut layouter =
                        |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
                            let job = build_text_job(
                                buf.as_str(),
                                layout_font.clone(),
                                col,
                                italic,
                                wrap_width,
                                false,
                            );
                            ui.fonts_mut(|f| f.layout_job(job))
                        };

                    let mut child = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(rect)
                            .layout(egui::Layout::top_down(egui::Align::Min)),
                    );

                    // FIX SFASAMENTO: TextEdit somma internamente `visuals.widgets.*.bg_stroke.width`
                    // Preserve the margin when the frame is disabled.
                    // Reset child UI spacing to keep editing positions stable.
                    {
                        let v = &mut child.style_mut().visuals.widgets;
                        v.inactive.bg_stroke.width = 0.0;
                        v.hovered.bg_stroke.width = 0.0;
                        v.active.bg_stroke.width = 0.0;
                        v.open.bg_stroke.width = 0.0;
                        v.noninteractive.bg_stroke.width = 0.0;
                    }
                    child.spacing_mut().item_spacing = egui::Vec2::ZERO;

                    let te = egui::TextEdit::multiline(&mut ann.text)
                        .id(te_id)
                        .frame(egui::Frame::NONE)
                        .margin(egui::Margin::same(g.pad_i8))
                        .desired_width(g.box_w - 2.0 * g.pad)
                        .desired_rows(1)
                        .lock_focus(true)
                        .layouter(&mut layouter)
                        .hint_text("Type here...");
                    let out = te.show(&mut child);
                    if need_init_cursor || !out.response.has_focus() {
                        out.response.request_focus();
                        if need_init_cursor {
                            if let Some(mut st) = egui::text_edit::TextEditState::load(ui.ctx(), te_id) {
                                st.cursor.set_char_range(Some(egui::text::CCursorRange::one(
                                    egui::text::CCursor::new(ann.text.chars().count()),
                                )));
                                st.store(ui.ctx(), te_id);
                            }
                        }
                        ui.ctx().request_repaint();
                    }
                    if out.response.changed() {
                        dirty = true;
                    }
                    if out.response.changed() {
                        dirty = true;
                    }

                    // Draw an extra pass over the TextEdit galley to render bold text.
                    if g.bold && !ann.text.is_empty() {
                        draw_bold_galley(
                            ui.painter(),
                            out.galley_pos,
                            &out.galley,
                            g.col,
                            g.font_id.size,
                        );
                    }
                }
            }

            // ----- Maniglia di ridimensionamento -----
            let handle_rect =
                egui::Rect::from_center_size(rect.right_bottom(), egui::vec2(18.0, 18.0));
            let rh = ui.interact(
                handle_rect,
                ui.make_persistent_id(("tb_resize", page, idx)),
                egui::Sense::drag(),
            );
            if rh.hovered() || rh.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNwSe);
            }
            if rh.drag_started() {
                self.push_undo_snapshot(page);
            }
            if rh.dragged() {
                resize_to = rh.interact_pointer_pos();
            }
            ui.painter()
                .rect_filled(handle_rect.shrink(3.5), egui::CornerRadius::same(2), accent);

            // Block settings toolbar without a shadow.
            let mut t_toggle_bg = false;
            let bg_mode = g.bg_mode;
            let mut t_size: Option<f32> = None;
            let mut t_bold: Option<bool> = None;
            let mut t_italic: Option<bool> = None;
            let mut t_wrap_token: Option<&'static str> = None;
            let mut t_toggle_list = false;
            let mut t_color: Option<[u8; 3]> = None;
            let mut t_delete = false;
            let mut grip_started = false;
            let mut grip_delta = egui::Vec2::ZERO;
            let (fs, bold, italic, color) = (g.fs, g.bold, g.italic, g.color);

            let above = rect.top() - 46.0 >= ui.clip_rect().top();
            let (bar_pos, pivot) = if above {
                (
                    egui::pos2(rect.left(), rect.top() - 8.0),
                    egui::Align2::LEFT_BOTTOM,
                )
            } else {
                (
                    egui::pos2(rect.left(), rect.bottom() + 8.0),
                    egui::Align2::LEFT_TOP,
                )
            };

            let bar = egui::Area::new(ui.make_persistent_id(("tb_toolbar", page, idx)))
                .fixed_pos(bar_pos)
                .pivot(pivot)
                .order(egui::Order::Foreground)
                .constrain(true)
                .show(ui.ctx(), |ui| {
                    egui::Frame::window(ui.style())
                        .shadow(egui::Shadow::NONE) // <-- Ombra rimossa!
                        .fill(bg_elev)
                        .stroke(egui::Stroke::new(1.2, accent))
                        .corner_radius(egui::CornerRadius::same(8))
                        .inner_margin(egui::Margin::symmetric(8, 5))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let grip = ui
                                    .add(
                                        egui::Label::new(
                                            egui::RichText::new("󰆾").size(16.0).color(fg),
                                        )
                                        .sense(egui::Sense::drag()),
                                    )
                                    .on_hover_text("Drag to move")
                                    .on_hover_cursor(egui::CursorIcon::Grab);
                                if grip.drag_started() {
                                    grip_started = true;
                                }
                                if grip.dragged() {
                                    grip_delta += grip.drag_delta();
                                }

                                ui.separator();

                                let mut s = fs;
                                if ui
                                    .add(
                                        egui::DragValue::new(&mut s)
                                            .range(8.0..=72.0)
                                            .speed(0.5)
                                            .suffix(" pt"),
                                    )
                                    .changed()
                                {
                                    t_size = Some(s);
                                }
                                if ui
                                    .add(
                                        egui::Button::new(egui::RichText::new("B").strong())
                                            .selected(bold),
                                    )
                                    .on_hover_text("Bold")
                                    .clicked()
                                {
                                    t_bold = Some(!bold);
                                }
                                if ui
                                    .add(
                                        egui::Button::new(egui::RichText::new("I").italics())
                                            .selected(italic),
                                    )
                                    .on_hover_text("Italic")
                                    .clicked()
                                {
                                    t_italic = Some(!italic);
                                }
                                if ui
                                    .add(egui::Button::new(egui::RichText::new("U").underline()))
                                    .on_hover_text("Underline (__text__)")
                                    .clicked()
                                {
                                    t_wrap_token = Some("__");
                                }
                                if ui
                                    .add(egui::Button::new(
                                        egui::RichText::new("S").strikethrough(),
                                    ))
                                    .on_hover_text("Strikethrough (~~text~~)")
                                    .clicked()
                                {
                                    t_wrap_token = Some("~~");
                                }
                                if ui
                                    .add(egui::Button::new("List"))
                                    .on_hover_text("Toggle bullet list (- )")
                                    .clicked()
                                {
                                    t_toggle_list = true;
                                }
                                let bg_label = match bg_mode {
                                    1 => "󰝤 Bg: Dark",
                                    2 => "󰝤 Bg: Light",
                                    _ => "󰝣 Bg",
                                };
                                if ui
                                    .add(egui::Button::new(bg_label).selected(bg_mode != 0))
                                    .on_hover_text("Background: Off → Dark → Light")
                                    .clicked()
                                {
                                    t_toggle_bg = true;
                                }

                                ui.separator();

                                for (pc, name) in ANNOTATION_PALETTE {
                                    let (sw, resp) = ui.allocate_exact_size(
                                        egui::vec2(18.0, 18.0),
                                        egui::Sense::click(),
                                    );
                                    ui.painter().circle_filled(sw.center(), 6.5, rgb(pc));
                                    if pc == color || resp.hovered() {
                                        ui.painter().circle_stroke(
                                            sw.center(),
                                            8.5,
                                            egui::Stroke::new(1.6, fg),
                                        );
                                    }
                                    if resp.on_hover_text(name).clicked() {
                                        t_color = Some(pc);
                                    }
                                }

                                ui.separator();
                                if ui
                                    .small_button("󰆴")
                                    .on_hover_text("Delete text box [Del]")
                                    .clicked()
                                {
                                    t_delete = true;
                                }
                            });
                        });
                });
            toolbar_rect = Some(bar.response.rect);

            if grip_started {
                self.push_undo_snapshot(page);
            }
            move_delta += grip_delta;

            if t_delete {
                self.push_undo_snapshot(page);
                if let Some(list) = self.annotations.get_mut(&page) {
                    if idx < list.len() {
                        list.remove(idx);
                    }
                }
                sel = None;
                dirty = true;
                break;
            }

            if t_size.is_some()
                || t_bold.is_some()
                || t_italic.is_some()
                || t_color.is_some()
                || t_wrap_token.is_some()
                || t_toggle_list
                || t_toggle_bg
            {
                self.push_undo_snapshot(page);
                if let Some(ann) = self
                    .annotations
                    .get_mut(&page)
                    .and_then(|v| v.get_mut(idx))
                {
                    if let Some(s) = t_size {
                        ann.font_size = s;
                        self.textbox_font_size = s;
                    }
                    if let Some(b) = t_bold {
                        ann.bold = b;
                        self.textbox_bold = b;
                    }
                    if let Some(i) = t_italic {
                        ann.italic = i;
                        self.textbox_italic = i;
                    }
                    if let Some(c) = t_color {
                        ann.color = c;
                        self.annotation_color = c;
                    }
                    if let Some(tok) = t_wrap_token {
                        if ann.text.starts_with(tok) && ann.text.ends_with(tok) && ann.text.len() >= tok.len() * 2 {
                            ann.text = ann.text[tok.len()..ann.text.len() - tok.len()].to_string();
                        } else {
                            ann.text = format!("{tok}{}{tok}", ann.text);
                        }
                    }
                    if t_toggle_list {
                        let all_bullets = ann
                            .text
                            .lines()
                            .filter(|l| !l.trim().is_empty())
                            .all(|l| l.starts_with("- "));
                        ann.text = ann
                            .text
                            .split('\n')
                            .map(|l| {
                                if all_bullets {
                                    l.strip_prefix("- ").unwrap_or(l).to_string()
                                } else if l.starts_with("- ") {
                                    l.to_string()
                                } else {
                                    format!("- {l}")
                                }
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                    }if t_toggle_bg {
                        if ann.text.starts_with('\u{200B}') {
                            // Cycle from dark to light.
                            ann.text.remove(0);
                            ann.text.insert(0, '\u{200C}');
                        } else if ann.text.starts_with('\u{200C}') {
                            // Cycle from light to no background.
                            ann.text.remove(0);
                        } else {
                            // Cycle from no background to dark.
                            ann.text.insert(0, '\u{200B}');
                        }
                    }
                }
                dirty = true;
            }
        }

        // Apply movement or resizing.
        if move_delta != egui::Vec2::ZERO || resize_to.is_some() {
            let (p0, p1) = (g.p0, g.p1);
            let mut np0 = p0;
            let mut np1 = p1;

            if move_delta != egui::Vec2::ZERO {
                let w = p1.0 - p0.0;
                let h = p1.1 - p0.1;
                np0.0 = (p0.0 + move_delta.x / pw).clamp(0.0, (1.0 - w).max(0.0));
                np0.1 = (p0.1 + move_delta.y / ph).clamp(0.0, (1.0 - h).max(0.0));
                np1 = (np0.0 + w, np0.1 + h);
            }
            if let Some(pos) = resize_to {
                let nx = ((pos.x - page_rect.left()) / pw).clamp(0.0, 1.0);
                let ny = ((pos.y - page_rect.top()) / ph).clamp(0.0, 1.0);
                np1 = (nx.max(np0.0 + MIN_W), ny.max(np0.1 + MIN_H));
            }

            if let Some(ann) = self
                .annotations
                .get_mut(&page)
                .and_then(|v| v.get_mut(idx))
            {
                ann.points = vec![np0, np1];
            }
            dirty = true;
        }
    }

    // 4. Escape, Delete, and outside clicks.
    if !just_created_this_frame {
        if let Some((sp, si, ed)) = sel {
            if sp == page {
                let (esc, del, pressed) = ui.input(|i| {
                    (
                        i.key_pressed(egui::Key::Escape),
                        i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
                        if i.pointer.primary_pressed() {
                            i.pointer.interact_pos()
                        } else {
                            None
                        },
                    )
                });

                if esc {
                    sel = if ed { Some((sp, si, false)) } else { None };
                } else if del && !ed && !ui.ctx().egui_wants_keyboard_input() {
                    self.push_undo_snapshot(page);
                    if let Some(list) = self.annotations.get_mut(&page) {
                        if si < list.len() {
                            list.remove(si);
                        }
                    }
                    sel = None;
                    dirty = true;
                } else if let Some(p) = pressed {
                    let on_sel = sel_rect.map_or(false, |r| r.expand(RING + 2.0).contains(p));
                    let on_bar = toolbar_rect.map_or(false, |r| r.expand(4.0).contains(p));
                    let on_any = geos.iter().any(|g| g.hit.contains(p));
                    if !on_sel && !on_bar && !on_any {
                        sel = None;
                    }
                }
            }
        }
    }

    // 5. Save state.
    ui.data_mut(|d| match sel {
        Some(s) => {
            d.insert_temp(sel_key, s);
        }
        None => {
            d.remove::<(u32, usize, bool)>(sel_key);
        }
    });

    let prev_edit: Option<(u32, usize)> = ui.data(|d| d.get_temp(edit_prev_key));
    match sel {
        Some((sp, si, true)) if sp == page => {
            ui.data_mut(|d| d.insert_temp(edit_prev_key, (sp, si)));
        }
        _ => {
            if prev_edit.map(|p| p.0) == Some(page) {
                ui.data_mut(|d| d.remove::<(u32, usize)>(edit_prev_key));
            }
        }
    }

    if page == self.current_page {
        self.selected_textbox = sel.filter(|s| s.0 == page).map(|s| s.1);
    }

    // Check whether the pointer is over a block or its toolbar.
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let over = pointer.map_or(false, |p| {
        geos.iter().any(|g| g.hit.contains(p))
            || sel_rect.map_or(false, |r| r.expand(RING).contains(p))
            || toolbar_rect.map_or(false, |r| r.contains(p))
    });
    let over_id = egui::Id::NULL.with(("pointer_over_textbox", page));
    ui.data_mut(|d| d.insert_temp(over_id, over));

    if dirty {
        let _ = notes::save_doc_data(
            &self.pdf_path,
            &self.notes,
            &self.annotations,
            self.blank_pages.clone(),
            &self.sketches,
        );
    }
}


    /// Highlights the selected text of one page: one rectangle per line, like a real viewer.
    fn paint_text_selection(&self, painter: &egui::Painter, page: u32, slide_rect: egui::Rect) {
        let Some(sel) = self.pdf_selection else {
            return;
        };
        let Some(text) = self.page_text.get(&page) else {
            return;
        };
        let Some((lo, hi)) = sel.range_on_page(page, text.chars.len()) else {
            return;
        };
        let fill = egui::Color32::from_rgba_unmultiplied(125, 156, 255, 85);
        let (w, h) = (slide_rect.width(), slide_rect.height());
        for l in &text.lines {
            let a = l.start.max(lo);
            let b = l.end.min(hi);
            if a >= b {
                continue;
            }
            let (mut x0, mut x1) = (f32::MAX, f32::MIN);
            for c in &text.chars[a..b] {
                x0 = x0.min(c.rect[0]);
                x1 = x1.max(c.rect[2]);
            }
            let r = egui::Rect::from_min_max(
                egui::pos2(slide_rect.left() + x0 * w, slide_rect.top() + l.bbox[1] * h),
                egui::pos2(slide_rect.left() + x1 * w, slide_rect.top() + l.bbox[3] * h),
            );
            painter.rect_filled(r, egui::CornerRadius::same(1), fill);
        }
    }

    /// Text of the current selection (can span several pages). Pages whose text has not
    /// been loaded yet (never rendered) are extracted synchronously: rare, copy-time only.
    fn selection_text(&self) -> Option<String> {
        let sel = self.pdf_selection?;
        let (s, e) = sel.ordered();
        if s == e {
            return None;
        }
        let mut extra: HashMap<u32, PageText> = HashMap::new();
        if (s.page..=e.page).any(|p| !self.page_text.contains_key(&p)) {
            if let Ok(doc) = Document::open(self.pdf_path.to_str().unwrap_or_default()) {
                for p in s.page..=e.page {
                    if !self.page_text.contains_key(&p) {
                        if let Some(t) = extract_text(&doc, p as i32 - 1) {
                            extra.insert(p, t);
                        }
                    }
                }
            }
        }
        let mut parts: Vec<String> = Vec::new();
        for p in s.page..=e.page {
            let text: Option<&PageText> = self
                .page_text
                .get(&p)
                .map(|a| a.as_ref())
                .or_else(|| extra.get(&p));
            let Some(text) = text else { continue };
            if let Some((lo, hi)) = sel.range_on_page(p, text.chars.len()) {
                let t = text.text_in(lo, hi);
                if !t.is_empty() {
                    parts.push(t);
                }
            }
        }
        let out = parts.join("\n\n");
        (!out.trim().is_empty()).then_some(out)
    }

    /// Once per frame (before drawing): while a selection drag is active, move the focus
    /// caret to the pointer, on whatever page it is over, and auto-scroll near the edges.
    fn update_text_drag(&mut self, ctx: &egui::Context) {
        let rects = std::mem::take(&mut self.slide_rects);
        self.last_slide_rects = rects.clone();

        let over_tb = ctx.data(|d| {
            rects.iter().any(|&(p, _)| {
                d.get_temp::<bool>(egui::Id::NULL.with(("pointer_over_textbox", p)))
                    .unwrap_or(false)
            }) || d
                .get_temp::<bool>(egui::Id::NULL.with(("pointer_over_textbox", self.current_page)))
                .unwrap_or(false)
        });
        if over_tb {
            return;
        }

        if !self.selecting {
            return;
        }
        if !ctx.input(|i| i.pointer.primary_down()) {
            self.selecting = false;
            return;
        }
        let Some(pos) = ctx.input(|i| i.pointer.latest_pos()) else {
            return;
        };

        if self.view_mode == ViewMode::Continuous && self.canvas_rect.is_positive() {
            let zone = 40.0;
            let top = self.canvas_rect.top();
            let bottom = self.canvas_rect.bottom();
            if pos.y < top + zone {
                self.continuous_scroll_delta += 6.0 + (top + zone - pos.y).min(80.0) * 0.3;
            } else if pos.y > bottom - zone {
                self.continuous_scroll_delta -= 6.0 + (pos.y - (bottom - zone)).min(80.0) * 0.3;
            }
            ctx.request_repaint();
        }

        let Some((page, rect)) = rects.iter().copied().min_by(|a, b| {
            a.1.distance_to_pos(pos)
                .partial_cmp(&b.1.distance_to_pos(pos))
                .unwrap_or(std::cmp::Ordering::Equal)
        }) else {
            return;
        };
        let Some(text) = self.page_text.get(&page) else {
            return;
        };
        let nx = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        let ny = ((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
        let idx = text.caret_at(nx, ny);
        if let Some(sel) = &mut self.pdf_selection {
            sel.focus = TextCaret { page, idx };
        }
    }

fn handle_slide_annotations(
        &mut self,
        ui: &mut egui::Ui,
        page_num: u32,
        slide_rect: egui::Rect,
        img_resp: &egui::Response,
    ) {
        if self.pdf_panning {
            return;
        }
        self.render_page_textboxes(ui, page_num, slide_rect);
        self.slide_rects.push((page_num, slide_rect));

        let to_norm = |pos: egui::Pos2| -> (f32, f32) {
            (
                ((pos.x - slide_rect.left()) / slide_rect.width()).clamp(0.0, 1.0),
                ((pos.y - slide_rect.top()) / slide_rect.height()).clamp(0.0, 1.0),
            )
        };

        if self.app_state.settings.radial_menu_enabled && img_resp.secondary_clicked() {
            self.radial_menu_pos = ui.input(|i| i.pointer.hover_pos());
        }

        let pointer_pos = ui.input(|i| i.pointer.hover_pos().or_else(|| i.pointer.interact_pos()));
        // Let the PDF set the cursor only when it is the topmost layer.
        // Overlays must keep their own cursor.
        
        let pointer_on_pdf_layer = pointer_pos
            .map(|pos| ui.ctx().layer_id_at(pos) == Some(img_resp.layer_id))
            .unwrap_or(false);
        if img_resp.hovered() && pointer_on_pdf_layer {
            let icon = match self.annotation_tool {
                AnnotationTool::Cursor => egui::CursorIcon::Default,
                _ => egui::CursorIcon::Crosshair,
            };
            ui.ctx().set_cursor_icon(icon);
        }

        let pointer_in_slide = pointer_pos
            .map(|p| slide_rect.contains(p) && pointer_on_pdf_layer)
            .unwrap_or(false);
        let any_text_focused = ui.ctx().memory(|m| m.focused().is_some());

        let hold_key_down = !any_text_focused
            && pointer_in_slide
            && ui.input(|i| self.app_state.shortcuts.trackpad_draw_hold.is_held(i));

        let hold_just_started = hold_key_down && !self.trackpad_drawing_active;
        let hold_just_released = !hold_key_down && self.trackpad_drawing_active;
        if pointer_in_slide || hold_just_released {
            self.trackpad_drawing_active = hold_key_down;
        }

        match self.annotation_tool {
            AnnotationTool::Cursor => {
                let mut hovered_link: Option<PdfLinkZone> = None;
                let mut over_text = false;

                if let Some(pos) = pointer_pos {
                    if slide_rect.contains(pos) && pointer_on_pdf_layer {
                        let (nx, ny) = to_norm(pos);

                        if let Some(links) = self.page_links.get(&page_num) {
                            hovered_link = links
                                .iter()
                                .find(|l| {
                                    nx >= l.rect[0]
                                        && nx <= l.rect[2]
                                        && ny >= l.rect[1]
                                        && ny <= l.rect[3]
                                })
                                .cloned();
                        }

                        if hovered_link.is_none() {
                            if let Some(text) = self.page_text.get(&page_num) {
                                over_text = text.is_over_text(nx, ny);
                            }
                        }
                    }
                }

                if let Some(ref link) = hovered_link {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    if img_resp.clicked() {
                        if let Some(tp) = link.target_page {
                            self.jump_with_history(tp, ui.ctx());
                        } else if let Some(ref url) = link.external_uri {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                        }
                        return;
                    }
                } else if pointer_on_pdf_layer && (over_text || self.selecting) {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
                }

                if img_resp.drag_started() {
                    let origin = ui
                        .input(|i| i.pointer.press_origin())
                        .or(img_resp.interact_pointer_pos());
                    if let Some(pos) = origin {
                        let (nx, ny) = to_norm(pos);
                        if let Some(text) = self.page_text.get(&page_num) {
                            let c = TextCaret {
                                page: page_num,
                                idx: text.caret_at(nx, ny),
                            };
                            self.pdf_selection = Some(TextSelection {
                                anchor: c,
                                focus: c,
                            });
                            self.selecting = true;
                        }
                    }
                } else if img_resp.triple_clicked() || img_resp.double_clicked() {
                    if let Some(pos) = img_resp.interact_pointer_pos() {
                        let (nx, ny) = to_norm(pos);
                        let line = img_resp.triple_clicked();
                        let range = self.page_text.get(&page_num).and_then(|t| {
                            let ci = t.char_at(nx, ny)?;
                            Some(if line { t.line_range(ci) } else { t.word_range(ci) })
                        });
                        if let Some((a, b)) = range {
                            self.pdf_selection = Some(TextSelection {
                                anchor: TextCaret { page: page_num, idx: a },
                                focus: TextCaret { page: page_num, idx: b },
                            });
                        }
                    }
                } else if img_resp.clicked() {
                    self.pdf_selection = None;
                }
            }
            AnnotationTool::Eraser => {
                if img_resp.clicked() || img_resp.dragged() || hold_key_down {
                    if let Some(pos) = pointer_pos {
                        let (nx, ny) = to_norm(pos);
                        let before_state = self
                            .annotations
                            .get(&page_num)
                            .cloned()
                            .unwrap_or_default();
                        let before_len = before_state.len();
                        let mut changed = false;

                        if let Some(list) = self.annotations.get_mut(&page_num) {
                            list.retain(|ann| match ann.kind {
                                notes::AnnotationKind::Highlight
                                | notes::AnnotationKind::Rect
                                | notes::AnnotationKind::Circle
                                | notes::AnnotationKind::Line
                                | notes::AnnotationKind::Arrow
                                | notes::AnnotationKind::Tape => {
                                    if ann.points.len() >= 2 {
                                        let (x1, y1) = ann.points[0];
                                        let (x2, y2) = *ann.points.last().unwrap();
                                        !(nx >= x1.min(x2) - 0.015
                                            && nx <= x1.max(x2) + 0.015
                                            && ny >= y1.min(y2) - 0.015
                                            && ny <= y1.max(y2) + 0.015)
                                    } else {
                                        true
                                    }
                                }
                                notes::AnnotationKind::Underline => {
                                    if ann.points.len() >= 2 {
                                        let (x1, y1) = ann.points[0];
                                        let (x2, _) = *ann.points.last().unwrap();
                                        !(nx >= x1.min(x2) - 0.015
                                            && nx <= x1.max(x2) + 0.015
                                            && (ny - y1).abs() < 0.02)
                                    } else {
                                        true
                                    }
                                }
                                notes::AnnotationKind::Freehand => !ann
                                    .points
                                    .iter()
                                    .any(|&(px, py)| (px - nx).hypot(py - ny) < 0.025),
                                notes::AnnotationKind::TextBox => {
                                    if let Some(&(bx, by)) = ann.points.first() {
                                        let (ex, ey) = ann.points.get(1).copied().unwrap_or((bx + 0.16, by + 0.04));
                                        !(nx >= bx - 0.01 && nx <= ex + 0.01 && ny >= by - 0.01 && ny <= ey + 0.01)
                                    } else {
                                        true
                                    }
                                }
                                notes::AnnotationKind::Image => true, // The eraser skips images.
                            });
                            changed = list.len() != before_len;
                        }

                        if changed {
                            let stack = self.undo_stack.entry(page_num).or_default();
                            if stack.len() >= 40 {
                                stack.remove(0);
                            }
                            stack.push(before_state);
                            self.redo_stack.remove(&page_num);
                            self.selected_textbox = None;

                            let _ = notes::save_doc_data(
                                &self.pdf_path,
                                &self.notes,
                                &self.annotations,
                                self.blank_pages.clone(),
                                &self.sketches,
                            );
                        }
                    }
                }
            }
            AnnotationTool::TextBox => {}
            AnnotationTool::Laser => {
                if img_resp.dragged() || hold_key_down {
                    if let Some(pos) = pointer_pos {
                        self.laser_trail
                            .push((to_norm(pos), std::time::Instant::now()));
                        ui.ctx().request_repaint();
                    }
                }
            }
            AnnotationTool::Snip => {
                let started = img_resp.drag_started() || hold_just_started;
                let active_move = img_resp.dragged() || hold_key_down;
                let stopped = img_resp.drag_stopped() || hold_just_released;

                if started {
                    let press_pos = if hold_just_started {
                        pointer_pos
                    } else {
                        ui.input(|i| i.pointer.press_origin())
                            .or(img_resp.interact_pointer_pos())
                            .or(pointer_pos)
                    };
                    if let Some(pos) = press_pos {
                        let pt = to_norm(pos);
                        self.active_snip_rect = Some((page_num, pt, pt));
                    }
                }
                else if active_move {
                if let (Some(pos), Some(snip)) = (pointer_pos, self.active_snip_rect.as_mut()) {
                    if snip.0 == page_num {
                        snip.2 = to_norm(pos);
                    }
                }
            } else if stopped {
                    if let Some((sp, p1, p2)) = self.active_snip_rect.take() {
                        if sp == page_num {
                            match crop_slide_region_to_png(
                                &self.pdf_path,
                                page_num,
                                p1,
                                p2,
                                self.pdf_dark_mode,
                            ) {
                                Ok(png_path) => {
                                    let md_link = format!(
                                        "\n![Snip p.{}]({})\n",
                                        page_num,
                                        paths::file_uri(&png_path)
                                    );
                                    self.current_note.push_str(&md_link);
                                    self.note_is_dirty = true;
                                    self.sync_and_save();
                                    self.show_notes = true;
                                    self.toast_message = Some((
                                        format!("󰆞 Snip saved to page {} notes", self.note_page),
                                        std::time::Instant::now(),
                                    ));
                                }
                                Err(e) => {
                                    self.toast_message = Some((
                                        format!("󰅚 Snip failed: {}", e),
                                        std::time::Instant::now(),
                                    ));
                                }
                            }
                        }
                    }
                }
            }
            tool => {
                let kind = match tool {
                    AnnotationTool::Highlight => notes::AnnotationKind::Highlight,
                    AnnotationTool::Underline => notes::AnnotationKind::Underline,
                    AnnotationTool::Tape => notes::AnnotationKind::Tape,
                    AnnotationTool::Shape => match self.shape_kind {
                        ShapeKind::Rect => notes::AnnotationKind::Rect,
                        ShapeKind::Circle => notes::AnnotationKind::Circle,
                        ShapeKind::Line => notes::AnnotationKind::Line,
                        ShapeKind::Arrow => notes::AnnotationKind::Arrow,
                    },
                    _ => notes::AnnotationKind::Freehand,
                };

                let started = img_resp.drag_started() || hold_just_started;
                let active_move = img_resp.dragged() || hold_key_down;
                let stopped = img_resp.drag_stopped() || hold_just_released;

                if started {
                    let press_pos = if hold_just_started {
                        pointer_pos
                    } else {
                        ui.input(|i| i.pointer.press_origin())
                            .or(img_resp.interact_pointer_pos())
                            .or(pointer_pos)
                    };
                    if let Some(pos) = press_pos {
                        let pt = to_norm(pos);
                        self.active_stroke = Some(notes::PageAnnotation {
                            kind,
                            color: self.annotation_color,
                            points: vec![pt, pt],
                            text: String::new(),
                            font_size: self.textbox_font_size,
                            bold: false,
                            italic: false,
                            stroke_width: self.stroke_width
                        });
                    }
                } else if active_move {

                    if let (Some(pos), Some(stroke)) = (pointer_pos, &mut self.active_stroke) {
                        let pt = to_norm(pos);
                        match stroke.kind {
                            notes::AnnotationKind::Freehand => {
                                if stroke.points.last() != Some(&pt) {
                                    stroke.points.push(pt);
                                }
                            }
                            _ => {
                                if stroke.points.len() >= 2 {
                                    *stroke.points.last_mut().unwrap() = pt;
                                }
                            }
                        }
                    }
                } else if stopped {
                    if let Some(mut stroke) = self.active_stroke.take() {
                        if stroke.kind == notes::AnnotationKind::Freehand {
                            stroke.points = notes::smooth_freehand_points(&stroke.points);
                        }
                        self.push_undo_snapshot(page_num);
                        self.annotations.entry(page_num).or_default().push(stroke);
                        let _ = notes::save_doc_data(
                            &self.pdf_path,
                            &self.notes,
                            &self.annotations,
                            self.blank_pages.clone(),
                            &self.sketches,
                        );
                    }
                }
            }
        }

        // Click on Occlusion Tapes to peek / reveal them
        if img_resp.clicked() && self.annotation_tool != AnnotationTool::Eraser {
            if let Some(pos) = pointer_pos {
                let (nx, ny) = to_norm(pos);
                if let Some(list) = self.annotations.get(&page_num) {
                    for (idx, ann) in list.iter().enumerate() {
                        if ann.kind == notes::AnnotationKind::Tape && ann.points.len() >= 2 {
                            let (x1, y1) = ann.points[0];
                            let (x2, y2) = *ann.points.last().unwrap();
                            if nx >= x1.min(x2)
                                && nx <= x1.max(x2)
                                && ny >= y1.min(y2)
                                && ny <= y1.max(y2)
                            {
                                let key = (page_num, idx);
                                if !self.revealed_tapes.remove(&key) {
                                    self.revealed_tapes.insert(key);
                                }
                                return;
                            }
                        }
                    }
                }
            }
        }
        self.image_annotations_ui(ui, page_num, slide_rect);
        // Paint saved annotations + active stroke + Snip rect + Laser trail + Text selection
        let painter = ui.painter_at(slide_rect);
        if let Some(page_anns) = self.annotations.get(&page_num) {
            for (idx, ann) in page_anns.iter().enumerate() {
                if ann.kind == notes::AnnotationKind::Tape
                    && self.revealed_tapes.contains(&(page_num, idx))
                {
                    if ann.points.len() >= 2 {
                        let p1 = egui::pos2(
                            slide_rect.left() + ann.points[0].0 * slide_rect.width(),
                            slide_rect.top() + ann.points[0].1 * slide_rect.height(),
                        );
                        let last = *ann.points.last().unwrap();
                        let p2 = egui::pos2(
                            slide_rect.left() + last.0 * slide_rect.width(),
                            slide_rect.top() + last.1 * slide_rect.height(),
                        );
                        let r = egui::Rect::from_two_pos(p1, p2);
                        painter.rect_stroke(
                            r,
                            egui::CornerRadius::same(4),
                            egui::Stroke::new(
                                1.5,
                                egui::Color32::from_rgba_unmultiplied(
                                    ann.color[0],
                                    ann.color[1],
                                    ann.color[2],
                                    90,
                                ),
                            ),
                            egui::StrokeKind::Inside,
                        );
                    }
                } else {
                    Self::paint_annotation(&painter, slide_rect, ann);
                }
            }
        }

        if let Some(active) = &self.active_stroke {
            if img_resp.dragged() || self.trackpad_drawing_active {
                Self::paint_annotation(&painter, slide_rect, active);
            }
        }

        if let Some((snip_page, p1, p2)) = self.active_snip_rect {
            if snip_page == page_num {
                let r = egui::Rect::from_two_pos(
                    egui::pos2(
                        slide_rect.left() + p1.0 * slide_rect.width(),
                        slide_rect.top() + p1.1 * slide_rect.height(),
                    ),
                    egui::pos2(
                        slide_rect.left() + p2.0 * slide_rect.width(),
                        slide_rect.top() + p2.1 * slide_rect.height(),
                    ),
                );
                painter.rect_filled(
                    r,
                    egui::CornerRadius::same(3),
                    egui::Color32::from_rgba_unmultiplied(125, 156, 255, 45),
                );
                painter.rect_stroke(
                    r,
                    egui::CornerRadius::same(3),
                    egui::Stroke::new(1.8, rgb(self.theme.accent)),
                    egui::StrokeKind::Inside,
                );
            }
        }

        let now = std::time::Instant::now();
        self.laser_trail
            .retain(|&(_, t)| now.duration_since(t).as_secs_f32() < 1.2);
        if !self.laser_trail.is_empty() {
            for win in self.laser_trail.windows(2) {
                let ((x0, y0), _) = win[0];
                let ((x1, y1), t1) = win[1];
                let age = now.duration_since(t1).as_secs_f32();
                let alpha = ((1.0 - (age / 1.2)).clamp(0.0, 1.0) * 240.0) as u8;
                let p0 = egui::pos2(
                    slide_rect.left() + x0 * slide_rect.width(),
                    slide_rect.top() + y0 * slide_rect.height(),
                );
                let p1 = egui::pos2(
                    slide_rect.left() + x1 * slide_rect.width(),
                    slide_rect.top() + y1 * slide_rect.height(),
                );
                painter.line_segment(
                    [p0, p1],
                    egui::Stroke::new(
                        5.0,
                        egui::Color32::from_rgba_unmultiplied(255, 45, 55, alpha / 2),
                    ),
                );
                painter.line_segment(
                    [p0, p1],
                    egui::Stroke::new(
                        2.2,
                        egui::Color32::from_rgba_unmultiplied(255, 220, 220, alpha),
                    ),
                );
            }
            if let Some(&((lx, ly), _)) = self.laser_trail.last() {
                let head = egui::pos2(
                    slide_rect.left() + lx * slide_rect.width(),
                    slide_rect.top() + ly * slide_rect.height(),
                );
                painter.circle_filled(head, 5.5, egui::Color32::from_rgb(255, 40, 50));
                painter.circle_filled(head, 2.5, egui::Color32::WHITE);
            }
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(16));
        }

        self.paint_text_selection(&painter, page_num, slide_rect);
    }

    fn render_settings_modal(&mut self, ui: &mut egui::Ui) {
        if !self.show_settings {
            return;
        }

        if ui.input(|i| i.key_pressed(egui::Key::Escape) && self.rebinding_action.is_none()) {
            self.show_settings = false;
            return;
        }

        const MODAL_W: f32 = 690.0;
        const MODAL_H: f32 = 460.0;
        const SIDEBAR_W: f32 = 175.0;
        const PANE_W: f32 = 475.0;
        const BODY_H: f32 = 395.0;

        let full_rect = ui.ctx().content_rect();
        egui::Area::new(ui.make_persistent_id("settings_backdrop"))
            .fixed_pos(full_rect.min)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                let (rect, resp) = ui.allocate_exact_size(full_rect.size(), egui::Sense::click());
                ui.painter().rect_filled(
                    rect,
                    egui::CornerRadius::ZERO,
                    egui::Color32::from_black_alpha(115),
                );
                if resp.clicked() && self.rebinding_action.is_none() {
                    self.show_settings = false;
                }
            });

        egui::Area::new(ui.make_persistent_id("settings_modal_window"))
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .order(egui::Order::Tooltip)
            .constrain(true)
            .show(ui.ctx(), |ui| {
                self.modal_glass_frame(242).show(ui, |ui| {
                    // Dimensione davvero fissa: min == max
                    ui.set_min_size(egui::vec2(MODAL_W, MODAL_H));
                    ui.set_max_size(egui::vec2(MODAL_W, MODAL_H));

                    // Force a vertical layout; the parent may be horizontal.
                    ui.vertical(|ui| {
                        // --- HEADER ---
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("󰒓  Advanced Settings")
                                    .size(17.0)
                                    .strong(),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.button("󰅖 Close").clicked() {
                                        self.show_settings = false;
                                    }
                                },
                            );
                        });
                        ui.add_space(6.0);
                        ui.separator();
                        ui.add_space(6.0);

                        // Main body: left sidebar and scrollable right panel.
                        ui.horizontal_top(|ui| {
                            // 1. Left column: category navigation.
                            egui::Frame::NONE
                                .fill(egui::Color32::from_black_alpha(45))
                                .corner_radius(egui::CornerRadius::same(8))
                                .inner_margin(egui::Margin::same(8))
                                .show(ui, |ui| {
                                    ui.set_min_size(egui::vec2(SIDEBAR_W, BODY_H));
                                    ui.set_max_size(egui::vec2(SIDEBAR_W, BODY_H));

                                    // Frame::show inherits the parent layout.
                                    ui.vertical(|ui| {
                                        let tabs = [
                                            (SettingsTab::StudyAndTools, "󰏫  Study & Canvas"),
                                            (SettingsTab::AppearanceAndTheme, "󰔎  Theme & Fonts"),
                                            (SettingsTab::Shortcuts, "󰌌  Keybindings"),
                                            (SettingsTab::SystemAndData, "󰒓  System & Export"),
                                            (SettingsTab::Storage, "󰋊  Storage & Data"),
                                            (SettingsTab::Dictionaries, "󰗊  Dictionaries"),
                                        ];

                                        for (tab, label) in tabs {
                                            let selected = self.settings_tab == tab;
                                            let btn = egui::Button::new(
                                                egui::RichText::new(label).size(13.0),
                                            )
                                            .selected(selected)
                                            .min_size(egui::vec2(SIDEBAR_W - 16.0, 32.0));

                                            if ui.add(btn).clicked() {
                                                self.settings_tab = tab;
                                            }
                                            ui.add_space(4.0);
                                        }
                                    });
                                });

                            ui.add_space(10.0);

                            // 2. Right column: selected category content.
                            egui::Frame::NONE
                                .inner_margin(egui::Margin::symmetric(6, 2))
                                .show(ui, |ui| {
                                    ui.set_min_size(egui::vec2(PANE_W, BODY_H));
                                    ui.set_max_size(egui::vec2(PANE_W, BODY_H));

                                    egui::ScrollArea::vertical()
                                        .id_salt("settings_right_pane_scroll")
                                        .max_height(BODY_H - 5.0)
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| {
                                            // Use an explicit vertical layout.
                                            ui.vertical(|ui| {
                                                ui.set_max_width(PANE_W - 24.0);

                                                let mut settings_dirty = false;

                                                match self.settings_tab {
                                                    // ───────────── STUDY & CANVAS ─────────────
                                                    SettingsTab::StudyAndTools => {
                                                        ui.label(egui::RichText::new("Annotation & Active Recall Tools").size(15.0).strong());
                                                        ui.add_space(8.0);

                                                        if ui
                                                            .checkbox(
                                                                &mut self.app_state.settings.radial_menu_enabled,
                                                                "Enable Quick Radial Tools Menu on canvas",
                                                            )
                                                            .changed()
                                                        {
                                                            settings_dirty = true;
                                                        }

                                                        if ui
                                                            .checkbox(
                                                                &mut self.app_state.settings.tools_button,
                                                                "Show Annotation Tools button",
                                                            )
                                                            .changed()
                                                        {
                                                            settings_dirty = true;
                                                        }

                                                        ui.add_space(10.0);
                                                        ui.label(egui::RichText::new("Annotation Tools Layout").size(13.5).strong());
                                                        ui.add_space(4.0);

                                                        ui.horizontal(|ui| {
                                                            let current = self.app_state.settings.tools_layout;

                                                            let options = [
                                                                (state::ToolsLayoutMode::SubTopbar, "󰕮  Sub-Topbar"),
                                                                (state::ToolsLayoutMode::VerticalRail, "󰍜  Vertical Rail"),
                                                            ];

                                                            for (mode, label) in options {
                                                                let btn = egui::Button::new(label)
                                                                    .selected(current == mode)
                                                                    .min_size(egui::vec2(150.0, 28.0));

                                                                if ui.add(btn).clicked() && current != mode {
                                                                    self.app_state.settings.tools_layout = mode;
                                                                    settings_dirty = true;
                                                                }
                                                            }
                                                        });

                                                        ui.add_space(10.0);
                                                        ui.horizontal(|ui| {
                                                            ui.label("Stroke Thickness:");
                                                            ui.add(egui::Slider::new(&mut self.stroke_width, 1.0..=18.0).suffix(" px"));
                                                        });

                                                        ui.separator();
                                                        ui.add_space(8.0);
                                                        ui.label(
                                                            egui::RichText::new("Sidebars & Panels")
                                                                .size(15.0)
                                                                .strong(),
                                                        );
                                                        ui.add_space(6.0);
                                                        if ui
                                                            .checkbox(
                                                                &mut self.app_state.settings.restore_show_notes,
                                                                "Remember Notes panel state on reopen",
                                                            )
                                                            .changed()
                                                        {
                                                            settings_dirty = true;
                                                        }
                                                        if ui
                                                            .checkbox(
                                                                &mut self.app_state.settings.restore_show_toc,
                                                                "Remember Outline panel state on reopen",
                                                            )
                                                            .changed()
                                                        {
                                                            settings_dirty = true;
                                                        }
                                                        if ui
                                                            .checkbox(
                                                                &mut self.app_state.settings.bottom_notes,
                                                                "Allow Notes panel to fit under the page when window is in portrait",
                                                            )
                                                            .changed()
                                                        {
                                                            settings_dirty = true;
                                                        }
                                                        if ui
                                                            .checkbox(
                                                                &mut self.app_state.settings.audio_notes,
                                                                "Enable Audio notes",
                                                            )
                                                            .changed()
                                                        {
                                                            settings_dirty = true;
                                                        }
                                                        if ui
                                                            .checkbox(
                                                                &mut self.app_state.settings.audio_dock,
                                                                "Enable Audio Lessons registation and Audio dock",
                                                            )
                                                            .changed()
                                                        {
                                                            settings_dirty = true;
                                                        }
                                                        if ui
                                                            .checkbox(
                                                                &mut self.app_state.settings.sketch_enabled,
                                                                "Enable Sketch (per-page scratchpad tab in the Notes panel)",
                                                            )
                                                            .on_hover_text("Turning it off hides the Sketch tab and leaves sketches out of exports. Your sketches are kept.")
                                                            .changed()
                                                        {
                                                            settings_dirty = true;
                                                            if !self.app_state.settings.sketch_enabled {
                                                                self.finish_sketch_edit();
                                                                if self.note_mode == NoteMode::Sketch {
                                                                    self.note_mode = NoteMode::Edit;
                                                                }
                                                            }
                                                        }
                                                        

                                                        ui.add_space(12.0);
                                                        ui.separator();
                                                        ui.add_space(8.0);
                                                        ui.label(
                                                            egui::RichText::new("Active Recall & Occlusion Tape")
                                                                .size(15.0)
                                                                .strong(),
                                                        );
                                                        ui.add_space(6.0);

                                                        ui.horizontal_wrapped(|ui| {
                                                            if ui
                                                                .button("󰝤  Cover All Tapes (Quiz Mode)")
                                                                .on_hover_text("Hide all revealed occlusion tapes across the PDF")
                                                                .clicked()
                                                            {
                                                                self.revealed_tapes.clear();
                                                            }

                                                            if ui
                                                                .button("󰈈  Reveal All Tapes")
                                                                .on_hover_text("Temporarily uncover all occlusion tapes in the document")
                                                                .clicked()
                                                            {
                                                                for (&p, list) in &self.annotations {
                                                                    for (idx, ann) in list.iter().enumerate() {
                                                                        if matches!(ann.kind, notes::AnnotationKind::Tape) {
                                                                            self.revealed_tapes.insert((p, idx));
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        });

                                                        ui.add_space(8.0);
                                                        if ui
                                                            .checkbox(
                                                                &mut self.app_state.settings.flash_button,
                                                                "Show Flashcards button",
                                                            )
                                                            .changed()
                                                        {
                                                            settings_dirty = true;
                                                        }
                                                        ui.horizontal_wrapped(|ui| {
                                                            if ui.button("󰘸 Open Flashcards Modal").clicked() {
                                                                self.open_flashcards_trainer();
                                                            }

                                                            if ui.button("󰈇 Export Anki Deck (.tsv)").clicked() {
                                                                self.export_anki_dialog(ui.ctx());
                                                            }
                                                        });


                                                        ui.add_space(12.0);
                                                        ui.separator();
                                                        ui.add_space(8.0);

                                                        ui.label(
                                                            egui::RichText::new("New Pages, Slideshow & Notes")
                                                                .size(15.0)
                                                                .strong(),
                                                        );
                                                        ui.add_space(4.0);

                                                        ui.label(egui::RichText::new("Default new page style").strong());
                                                        ui.horizontal_wrapped(|ui| {
                                                            for (style, label) in [
                                                                (notes::BlankPageStyle::Ruled, "Ruled"),
                                                                (notes::BlankPageStyle::Dotted, "Dotted"),
                                                                (notes::BlankPageStyle::Grid, "Grid"),
                                                                (notes::BlankPageStyle::Blank, "Blank"),
                                                            ] {
                                                                if ui.selectable_value(&mut self.app_state.settings.default_page_style, style, label).changed() {
                                                                    settings_dirty = true;
                                                                }
                                                            }
                                                        });
                                                        ui.add_space(8.0);

                                                        ui.weak(
                                                            "Choose how Slideshow + Notes handles slides without annotations.",
                                                        );
                                                        ui.add_space(6.0);

                                                        for (mode, label) in [
                                                            (
                                                                SlideshowEmptyMode::AutoHold,
                                                                "Auto-Hold Previous Note",
                                                            ),
                                                            (
                                                                SlideshowEmptyMode::ContextDashboard,
                                                                "Study Context Dashboard",
                                                            ),
                                                            (
                                                                SlideshowEmptyMode::SkipEmpty,
                                                                "Skip Unannotated Slides",
                                                            ),
                                                            (
                                                                SlideshowEmptyMode::ExpandFull,
                                                                "Auto-Expand Fullscreen",
                                                            ),
                                                            (
                                                                SlideshowEmptyMode::StaticEmpty,
                                                                "Show Empty-Slide Placeholder",
                                                            ),
                                                        ] {
                                                            if ui
                                                                .radio_value(
                                                                    &mut self.app_state.settings.slideshow_empty_mode,
                                                                    mode,
                                                                    label,
                                                                )
                                                                .changed()
                                                            {
                                                                settings_dirty = true;
                                                            }
                                                        }


                                                    }

                                                    // ───────────── THEME & FONTS ─────────────
                                                    SettingsTab::AppearanceAndTheme => {
                                                        ui.label(
                                                            egui::RichText::new("Theme Presets, Colors & Typography")
                                                                .size(15.0)
                                                                .strong(),
                                                        );
                                                        ui.add_space(8.0);

                                                        ui.horizontal_wrapped(|ui| {
                                                            if ui.button("󰋺  Import Theme...").clicked() {
                                                                self.import_theme_dialog(ui.ctx());
                                                            }
                                                            if ui.button("󰈇  Export Theme...").clicked() {
                                                                self.export_theme_dialog(ui.ctx());
                                                            }
                                                            if ui.button("󰑐  Reset Default").clicked() {
                                                                self.theme = SuiteTheme::default();
                                                                self.theme.apply(ui.ctx());
                                                                let _ = self.theme.save();
                                                            }
                                                        });

                                                        ui.add_space(12.0);
                                                        ui.separator();
                                                        ui.add_space(8.0);


                                                        ui.label(
                                                            egui::RichText::new("Theme Presets")
                                                                .size(14.0)
                                                                .strong(),
                                                        );
                                                        ui.add_space(4.0);

                                                        ui.weak(
                                                            "Each preset updates the entire interface palette, including backgrounds, text, borders and widget states.",
                                                        );
                                                        ui.add_space(6.0);

                                                        // ───────────── DARK THEMES · 12 ─────────────
                                                        ui.label(
                                                            egui::RichText::new("  Dark Themes")
                                                                .strong()
                                                                .size(13.5),
                                                        );
                                                        ui.add_space(6.0);

                                                        ui.horizontal_wrapped(|ui| {
                                                            for preset in SuiteTheme::dark_presets() {
                                                                let selected = self.theme.name == preset.name;

                                                                let button = egui::Button::new(
                                                                    egui::RichText::new(format!("{}  {}", preset.symbol(), preset.name))
                                                                        .color(rgb(preset.accent))
                                                                        .strong(),
                                                                )
                                                                .fill(rgb(preset.bg_widget))
                                                                .stroke(egui::Stroke::new(
                                                                    1.0,
                                                                    if selected {
                                                                        rgb(preset.accent)
                                                                    } else {
                                                                        rgb(preset.border)
                                                                    },
                                                                ))
                                                                .corner_radius(egui::CornerRadius::same(8))
                                                                .min_size(egui::vec2(142.0, 36.0));

                                                                if ui.add(button).clicked() && !selected {
                                                                    self.theme = preset;
                                                                    self.theme.apply(ui.ctx());
                                                                    let _ = self
                                                                        .theme
                                                                        .save_to_file(&SuiteTheme::default_config_path());
                                                                }
                                                            }
                                                        });

                                                        ui.add_space(10.0);

                                                        // ───────────── LIGHT THEMES · 6 ─────────────
                                                        ui.label(
                                                            egui::RichText::new("  Light Themes")
                                                                .strong()
                                                                .size(13.5),
                                                        );
                                                        ui.add_space(6.0);

                                                        ui.horizontal_wrapped(|ui| {
                                                            for preset in SuiteTheme::light_presets() {
                                                                let selected = self.theme.name == preset.name;

                                                                let button = egui::Button::new(
                                                                    egui::RichText::new(format!("{}  {}", preset.symbol(), preset.name))
                                                                        .color(rgb(preset.accent))
                                                                        .strong(),
                                                                )
                                                                .fill(rgb(preset.bg_widget))
                                                                .stroke(egui::Stroke::new(
                                                                    1.0,
                                                                    if selected {
                                                                        rgb(preset.accent)
                                                                    } else {
                                                                        rgb(preset.border)
                                                                    },
                                                                ))
                                                                .corner_radius(egui::CornerRadius::same(8))
                                                                .min_size(egui::vec2(142.0, 36.0));

                                                                if ui.add(button).clicked() && !selected {
                                                                    self.theme = preset;
                                                                    self.theme.apply(ui.ctx());
                                                                    let _ = self
                                                                        .theme
                                                                        .save_to_file(&SuiteTheme::default_config_path());
                                                                }
                                                            }
                                                        });

                                                        ui.add_space(16.0);
                                                        ui.separator();
                                                        ui.add_space(8.0);


                                                        // --- Color picker dettagliati & corner radius ---
                                                        ui.label(
                                                            egui::RichText::new("Advanced Manual Palette Tweaks")
                                                                .size(13.5)
                                                                .strong(),
                                                        );
                                                        ui.add_space(6.0);

                                                        let mut theme_changed = false;
                                                        egui::Grid::new("theme_colors_grid_manual")
                                                            .num_columns(4)
                                                            .spacing([18.0, 8.0])
                                                            .show(ui, |ui| {
                                                                ui.label("Canvas BG:");
                                                                theme_changed |= ui.color_edit_button_srgb(&mut self.theme.bg_canvas).changed();
                                                                ui.label("Panel BG:");
                                                                theme_changed |= ui.color_edit_button_srgb(&mut self.theme.bg_panel).changed();
                                                                ui.end_row();

                                                                ui.label("Elevated BG:");
                                                                theme_changed |= ui.color_edit_button_srgb(&mut self.theme.bg_elevated).changed();
                                                                ui.label("Widget BG:");
                                                                theme_changed |= ui.color_edit_button_srgb(&mut self.theme.bg_widget).changed();
                                                                ui.end_row();

                                                                ui.label("Accent Color:");
                                                                theme_changed |= ui.color_edit_button_srgb(&mut self.theme.accent).changed();
                                                                ui.label("Border Color:");
                                                                theme_changed |= ui.color_edit_button_srgb(&mut self.theme.border).changed();
                                                                ui.end_row();
                                                            });

                                                        ui.add_space(8.0);
                                                        ui.horizontal(|ui| {
                                                            ui.label("UI Corner Radius:");
                                                            if ui
                                                                .add(
                                                                    egui::Slider::new(&mut self.theme.corner_radius, 0..=14)
                                                                        .suffix(" px"),
                                                                )
                                                                .changed()
                                                            {
                                                                theme_changed = true;
                                                            }
                                                        });

                                                        if theme_changed {
                                                            self.theme.apply(ui.ctx());
                                                            let _ = self.theme.save();
                                                        }

                                                        ui.add_space(12.0);
                                                        ui.separator();
                                                        ui.add_space(8.0);

                                                        // --- PDF dark mode & tipografia ---
                                                        ui.label(
                                                            egui::RichText::new("PDF Rendering & Notes Font")
                                                                .size(14.0)
                                                                .strong(),
                                                        );
                                                        ui.add_space(6.0);

                                                        if ui
                                                            .checkbox(
                                                                &mut self.pdf_dark_mode,
                                                                "PDF Dark Mode (invert slide background to deep slate)",
                                                            )
                                                            .changed()
                                                        {
                                                            self.page_cache.clear();
                                                            self.thumb_cache.clear();
                                                            ui.ctx().request_repaint();
                                                        }

                                                        ui.add_space(6.0);
                                                        ui.horizontal(|ui| {
                                                            ui.label("Notes Font Size:");
                                                            if ui
                                                                .add(
                                                                    egui::Slider::new(
                                                                        &mut self.app_state.settings.note_font_size,
                                                                        11.0..=24.0,
                                                                    )
                                                                    .suffix(" pt"),
                                                                )
                                                                .changed()
                                                            {
                                                                settings_dirty = true;
                                                            }
                                                        });

                                                        ui.add_space(8.0);
                                                        ui.separator();
                                                        ui.add_space(6.0);

                                                        ui.label(
                                                            egui::RichText::new("Custom Notes Font")
                                                                .size(14.0)
                                                                .strong(),
                                                        );
                                                        ui.add_space(4.0);

                                                        let font_label = self
                                                            .app_state
                                                            .settings
                                                            .custom_font_path
                                                            .as_ref()
                                                            .and_then(|p| p.file_name())
                                                            .and_then(|s| s.to_str())
                                                            .unwrap_or("Default font");

                                                        ui.weak(format!("Active font: {}", font_label));
                                                        ui.add_space(6.0);

                                                        ui.horizontal_wrapped(|ui| {
                                                            if ui.button("󰛖  Load Font (.ttf/.otf)...").clicked() {
                                                                if let Some(path) = rfd::FileDialog::new()
                                                                    .add_filter("Font Files", &["ttf", "otf"])
                                                                    .pick_file()
                                                                {
                                                                    self.app_state.settings.custom_font_path = Some(path);
                                                                    self.apply_custom_font(ui.ctx());
                                                                    settings_dirty = true;
                                                                }
                                                            }

                                                            if self.app_state.settings.custom_font_path.is_some()
                                                                && ui.button("Reset Font").clicked()
                                                            {
                                                                self.app_state.settings.custom_font_path = None;
                                                                self.apply_custom_font(ui.ctx());
                                                                settings_dirty = true;
                                                            }
                                                        });

                                                    }
                                                    // ───────────── KEYBINDINGS ─────────────
                                                    
                                                    SettingsTab::Shortcuts => {
                                                        ui.label(egui::RichText::new("Keyboard Shortcuts").size(15.0).strong());
                                                        ui.add_space(4.0);
                                                        ui.weak("Click a shortcut, then press the new combination. Hold a key and press another for multi-key combos (e.g. G + H). Esc cancels.");
                                                        ui.add_space(8.0);
                                                    
                                                        // Defaults and snapshots for reset and conflict detection.
                                                        let mut defaults_cfg = ShortcutsConfig::default();
                                                        let defaults: Vec<(&'static str, ShortcutBinding)> = shortcut_entries(&mut defaults_cfg)
                                                            .into_iter()
                                                            .map(|(l, b)| (l, b.clone()))
                                                            .collect();
                                                    
                                                        let cfg = &mut self.app_state.shortcuts;
                                                        let snapshot: Vec<(&'static str, ShortcutBinding)> = shortcut_entries(cfg)
                                                            .into_iter()
                                                            .map(|(l, b)| (l, b.clone()))
                                                            .collect();
                                                    
                                                        for (label, binding) in shortcut_entries(cfg) {
                                                            if self.rebinding_action.as_deref() == Some(label) {
                                                                if let Some(cap) = capture_shortcut(ui) {
                                                                    if let Capture::Set(nb) = cap {
                                                                        *binding = nb;
                                                                        settings_dirty = true;
                                                                    }
                                                                    self.rebinding_action = None;
                                                                }
                                                            }
                                                    
                                                            let is_rebinding = self.rebinding_action.as_deref() == Some(label);
                                                            let default = defaults.iter().find(|(l, _)| *l == label).map(|(_, b)| b.clone());
                                                    
                                                            let conflict = snapshot
                                                                .iter()
                                                                .find(|(l, b)| *l != label && *b == *binding)
                                                                .map(|(l, _)| *l);
                                                    
                                                            ui.horizontal(|ui| {
                                                                ui.label(label);
                                                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                                    // Reset (rightmost)
                                                                    if let Some(def) = &default {
                                                                        if *binding != *def
                                                                            && ui.small_button("󰑐").on_hover_text("Reset to default").clicked()
                                                                        {
                                                                            *binding = def.clone();
                                                                            settings_dirty = true;
                                                                            if is_rebinding {
                                                                                self.rebinding_action = None;
                                                                            }
                                                                        }
                                                                    }
                                                    
                                                                    let text = if is_rebinding {
                                                                        "󰌌  Press keys…  (Esc to cancel)".to_string()
                                                                    } else {
                                                                        binding.format_label()
                                                                    };
                                                                    let btn = egui::Button::new(egui::RichText::new(text).monospace())
                                                                        .selected(is_rebinding)
                                                                        .min_size(egui::vec2(170.0, 24.0));
                                                    
                                                                    if ui.add(btn).clicked() {
                                                                        ui.data_mut(|d| d.remove::<PendingChord>(ui.make_persistent_id(PENDING_ID)));
                                                                        self.rebinding_action = if is_rebinding {
                                                                            None
                                                                        } else {
                                                                            Some(label.to_string())
                                                                        };
                                                                    }
                                                                });
                                                            });
                                                    
                                                            if let Some(other) = conflict {
                                                                ui.label(
                                                                    egui::RichText::new(format!("⚠ Same combination as “{}”", other))
                                                                        .size(11.5)
                                                                        .color(egui::Color32::from_rgb(251, 191, 36)),
                                                                );
                                                            }
                                                            ui.separator();
                                                        }
                                                    
                                                        ui.add_space(8.0);
                                                        if ui.button("󰑐  Reset All Shortcuts to Default").clicked() {
                                                            *cfg = ShortcutsConfig::default();
                                                            self.rebinding_action = None;
                                                            settings_dirty = true;
                                                        }
                                                    
                                                        ui.add_space(12.0);
                                                        ui.label(egui::RichText::new("Fixed shortcuts").size(13.5).strong());
                                                        ui.add_space(4.0);
                                                        for (name, keys) in [
                                                            ("Command Palette", "Ctrl + P / Ctrl + K"),
                                                            ("Global Tri-Modal Search", "Ctrl + F"),
                                                            ("Undo / Redo Annotation", "Ctrl + Z / Ctrl + Y"),
                                                        ] {
                                                            ui.horizontal(|ui| {
                                                                ui.label(name);
                                                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                                    ui.code(keys);
                                                                });
                                                            });
                                                        }
                                                    }


                                                    // ───────────── SYSTEM & EXPORT ─────────────
                                                    SettingsTab::SystemAndData => {
                                                                                                                ui.label(egui::RichText::new("System Integration & Document Management").size(15.0).strong());
                                                        ui.add_space(8.0);
                                                        ui.label(egui::RichText::new("Current PDF Actions").size(15.0).strong());
                                                        ui.add_space(6.0);

                                                        ui.horizontal_wrapped(|ui| {
                                                            if ui.button("󰇚  Download Original PDF").clicked() {
                                                                self.prompt_download_original_pdf(ui.ctx());
                                                            }
                                                            let export_label = if self.is_exporting {
                                                                "󱑚  Exporting...".to_string()
                                                            } else {
                                                                format!(
                                                                    "󰚥  Export PDF ({})",
                                                                    self.app_state.export_settings.content.short_label()
                                                                )
                                                            };
                                                            if ui.button(export_label).clicked() {
                                                                if !self.is_exporting {
                                                                    self.start_export(ui.ctx());
                                                                }
                                                            }
                                                        });

                                                    ui.add_space(12.0);
                                                    ui.separator();
                                                    ui.add_space(8.0);
                                                        ui.label(
                                                            egui::RichText::new("PDF Export")
                                                                .size(15.0)
                                                                .strong(),
                                                        );
                                                        ui.add_space(4.0);

                                                        ui.weak(
                                                            "Customize note typography and slide proportions in exported PDFs.",
                                                        );
                                                        ui.add_space(8.0);

                                                        let mut export_changed = false;

                                                        {
                                                            let cfg = &mut self.app_state.export_settings;

                                                            egui::Grid::new("export_settings_grid")
                                                                .num_columns(2)
                                                                .spacing([12.0, 10.0])
                                                                .show(ui, |ui| {
                                                                    ui.label("Notes Base Font Size:");

                                                                    if ui
                                                                        .add(
                                                                            egui::DragValue::new(&mut cfg.base_font_size)
                                                                                .range(12.0..=32.0)
                                                                                .speed(0.2)
                                                                                .suffix(" pt"),
                                                                        )
                                                                        .changed()
                                                                    {
                                                                        export_changed = true;
                                                                    }

                                                                    ui.end_row();

                                                                    ui.label("Export content:");

                                                                    ui.vertical(|ui| {
                                                                        let current = cfg.content;

                                                                        for content in crate::state::ExportContent::ALL {
                                                                            let btn = egui::Button::new(content.label())
                                                                                .selected(current == content);

                                                                            if ui
                                                                                .add_sized([220.0, 28.0], btn)
                                                                                .clicked()
                                                                                && current != content
                                                                            {
                                                                                cfg.content = content;
                                                                                export_changed = true;
                                                                            }
                                                                        }
                                                                    });

                                                                    ui.end_row();

                                                                    ui.label("Slide Width on Split Pages:");

                                                                    if ui
                                                                        .add(
                                                                            egui::DragValue::new(&mut cfg.slide_width_pct)
                                                                                .range(40.0..=80.0)
                                                                                .speed(0.5)
                                                                                .suffix(" %"),
                                                                        )
                                                                        .changed()
                                                                    {
                                                                        export_changed = true;
                                                                    }

                                                                    ui.end_row();
                                                                });

                                                            ui.add_space(8.0);

                                                            if ui
                                                                .button("󰑐  Restore Export Defaults")
                                                                .clicked()
                                                            {
                                                                *cfg = state::ExportSettings::default();
                                                                export_changed = true;
                                                            }

                                                            if export_changed {
                                                                cfg.clamp_valid();
                                                            }
                                                        }

                                                        if export_changed {
                                                            settings_dirty = true;
                                                        }

                                                    }     
                                                    SettingsTab::Storage => {
                                                        let cache_id = ui.make_persistent_id("storage_scan_cache");
                                                        let stretch_id = ui.make_persistent_id("storage_stretch_size");
                                                        let confirm_id = ui.make_persistent_id("storage_confirm");
                                                    
                                                        // Scan on refresh or action, not on every frame.
                                                        let entries: Vec<StorageEntry> =
                                                            match ui.data(|d| d.get_temp::<Vec<StorageEntry>>(cache_id)) {
                                                                Some(v) => v,
                                                                None => {
                                                                    let v = scan_storage(&self.app_state.recent_files);
                                                                    let s = dir_size(&stretch_cache_dir());
                                                                    ui.data_mut(|d| {
                                                                        d.insert_temp(cache_id, v.clone());
                                                                        d.insert_temp(stretch_id, s);
                                                                    });
                                                                    v
                                                                }
                                                            };
                                                        let stretch_size: u64 = ui.data(|d| d.get_temp::<u64>(stretch_id)).unwrap_or(0);
                                                        let mut confirm: String = ui
                                                            .data(|d| d.get_temp::<String>(confirm_id))
                                                            .unwrap_or_default();
                                                        let mut action: Option<StorageAction> = None;
                                                    
                                                        let open_canon = if self.has_pdf() {
                                                            dunce::canonicalize(&self.pdf_path).ok()
                                                        } else {
                                                            None
                                                        };
                                                    
                                                        ui.label(egui::RichText::new("Stored Data & Cleanup").size(15.0).strong());
                                                        ui.add_space(4.0);
                                                        ui.weak("Notes, audio and original-PDF copies per document, largest first.");
                                                        ui.add_space(8.0);
                                                    
                                                        let total: u64 = entries.iter().map(|e| e.total()).sum();
                                                        let audio_total: u64 = entries.iter().map(|e| e.audio_size).sum();
                                                        let orphan_count = entries
                                                            .iter()
                                                            .filter(|e| e.pdf_path.is_some() && !e.pdf_exists)
                                                            .count();
                                                        let orphan_bytes: u64 = entries
                                                            .iter()
                                                            .filter(|e| e.pdf_path.is_some() && !e.pdf_exists)
                                                            .map(|e| e.total())
                                                            .sum();
                                                    
                                                        ui.horizontal_wrapped(|ui| {
                                                            ui.label(format!(
                                                                "{} document(s) · {} total · {} audio",
                                                                entries.len(),
                                                                human_size(total),
                                                                human_size(audio_total)
                                                            ));
                                                            if ui.button("󰑐 Refresh").clicked() {
                                                                ui.data_mut(|d| d.remove::<Vec<StorageEntry>>(cache_id));
                                                            }
                                                        });
                                                    
                                                        if orphan_count > 0 {
                                                            ui.add_space(4.0);
                                                            ui.horizontal_wrapped(|ui| {
                                                                if danger_button(
                                                                    ui,
                                                                    &mut confirm,
                                                                    "orphans".to_string(),
                                                                    &format!(
                                                                        "󰆴 Clean {} orphan(s) · {}",
                                                                        orphan_count,
                                                                        human_size(orphan_bytes)
                                                                    ),
                                                                    "Delete notes, audio and original copies of documents whose PDF no longer exists",
                                                                    true,
                                                                ) {
                                                                    action = Some(StorageAction::CleanOrphans);
                                                                }
                                                            });
                                                        }
                                                    
                                                        ui.add_space(8.0);
                                                        ui.separator();
                                                        ui.add_space(6.0);
                                                    
                                                        if entries.is_empty() {
                                                            ui.weak("No stored document data found.");
                                                        }
                                                    
                                                        for (i, en) in entries.iter().enumerate() {
                                                            let is_open = en.pdf_path.is_some() && en.pdf_path == open_canon;
                                                    
                                                            egui::Frame::NONE
                                                                .fill(egui::Color32::from_black_alpha(35))
                                                                .corner_radius(egui::CornerRadius::same(6))
                                                                .inner_margin(egui::Margin::same(8))
                                                                .show(ui, |ui| {
                                                                    ui.vertical(|ui| {
                                                                        ui.set_width(ui.available_width());
                                                    
                                                                        // Nome + dimensione totale
                                                                        ui.horizontal(|ui| {
                                                                            ui.with_layout(
                                                                                egui::Layout::right_to_left(egui::Align::Center),
                                                                                |ui| {
                                                                                    ui.label(
                                                                                        egui::RichText::new(human_size(en.total())).strong(),
                                                                                    );
                                                                                    ui.with_layout(
                                                                                        egui::Layout::left_to_right(egui::Align::Center),
                                                                                        |ui| {
                                                                                            ui.add(
                                                                                                egui::Label::new(
                                                                                                    egui::RichText::new(&en.name).strong(),
                                                                                                )
                                                                                                .truncate(),
                                                                                            );
                                                                                        },
                                                                                    );
                                                                                },
                                                                            );
                                                                        });
                                                    
                                                                        // Path and state.
                                                                        match (&en.pdf_path, en.pdf_exists) {
                                                                            (Some(p), true) => {
                                                                                ui.add(
                                                                                    egui::Label::new(
                                                                                        egui::RichText::new(p.display().to_string())
                                                                                            .size(11.0)
                                                                                            .weak(),
                                                                                    )
                                                                                    .truncate(),
                                                                                );
                                                                            }
                                                                            (Some(p), false) => {
                                                                                ui.add(
                                                                                    egui::Label::new(
                                                                                        egui::RichText::new(format!(
                                                                                            "⚠ PDF not found: {}",
                                                                                            p.display()
                                                                                        ))
                                                                                        .size(11.0)
                                                                                        .color(egui::Color32::from_rgb(251, 191, 36)),
                                                                                    )
                                                                                    .truncate(),
                                                                                );
                                                                            }
                                                                            (None, _) => {
                                                                                ui.label(
                                                                                    egui::RichText::new(
                                                                                        "⚠ Source PDF unknown (legacy data, re-open the PDF once to link it)",
                                                                                    )
                                                                                    .size(11.0)
                                                                                    .color(egui::Color32::from_rgb(251, 191, 36)),
                                                                                );
                                                                            }
                                                                        }
                                                    
                                                                        // Dettagli
                                                                        let mut detail = format!(
                                                                            "{} note page(s) · {} annotation(s) · backup {}",
                                                                            en.notes_count,
                                                                            en.ann_count,
                                                                            human_size(en.backup_size)
                                                                        );
                                                                        if en.orig_size > 0 {
                                                                            detail.push_str(&format!(
                                                                                " · original copy {}",
                                                                                human_size(en.orig_size)
                                                                            ));
                                                                        }
                                                                        ui.weak(detail);
                                                                        if en.audio_size > 0 {
                                                                            ui.weak(format!(
                                                                                "audio {} · lecture {} · {} voice memo(s) {}",
                                                                                human_size(en.audio_size),
                                                                                human_size(en.lecture_size),
                                                                                en.memo_count,
                                                                                human_size(en.memos_size)
                                                                            ));
                                                                        }
                                                                        ui.add_space(4.0);
                                                    
                                                                        // Azioni
                                                                        ui.horizontal_wrapped(|ui| {
                                                                            if en.pdf_exists {
                                                                                let busy = "Close this PDF first";
                                                    
                                                                                if danger_button(
                                                                                    ui,
                                                                                    &mut confirm,
                                                                                    format!("clear:{}", en.id),
                                                                                    "󰆴 Notes",
                                                                                    if is_open {
                                                                                        busy
                                                                                    } else {
                                                                                        "Removes notes & annotations from the backup AND from the data appended inside the PDF. Cannot be undone."
                                                                                    },
                                                                                    en.has_notes() && !is_open,
                                                                                ) {
                                                                                    action = Some(StorageAction::ClearNotes(i));
                                                                                }
                                                    
                                                                                if en.lecture_size > 0
                                                                                    && danger_button(
                                                                                        ui,
                                                                                        &mut confirm,
                                                                                        format!("lec:{}", en.id),
                                                                                        "󰆴 Lecture audio",
                                                                                        if is_open {
                                                                                            busy
                                                                                        } else {
                                                                                            "Deletes the lecture recording with its transcript, slide markers and waveform. Voice memos are kept."
                                                                                        },
                                                                                        !is_open,
                                                                                    )
                                                                                {
                                                                                    action = Some(StorageAction::DeleteLecture(i));
                                                                                }
                                                    
                                                                                if en.memo_count > 0
                                                                                    && danger_button(
                                                                                        ui,
                                                                                        &mut confirm,
                                                                                        format!("memo:{}", en.id),
                                                                                        "󰆴 Voice memos",
                                                                                        if is_open {
                                                                                            busy
                                                                                        } else {
                                                                                            "Deletes every voice memo and its transcript. The lecture recording is kept."
                                                                                        },
                                                                                        !is_open,
                                                                                    )
                                                                                {
                                                                                    action = Some(StorageAction::DeleteMemos(i));
                                                                                }
                                                    
                                                                                if en.orig_size > 0
                                                                                    && danger_button(
                                                                                        ui,
                                                                                        &mut confirm,
                                                                                        format!("orig:{}", en.id),
                                                                                        "󰆴 Original copy",
                                                                                        "Deletes the hidden .orig copy. 'Download Original PDF' will then return the current (modified) PDF.",
                                                                                        true,
                                                                                    )
                                                                                {
                                                                                    action = Some(StorageAction::DeleteOrig(i));
                                                                                }
                                                                            } else if danger_button(
                                                                                ui,
                                                                                &mut confirm,
                                                                                format!("left:{}", en.id),
                                                                                "󰆴 Delete leftovers",
                                                                                "Deletes the notes backup, the audio and the .orig copy of this document",
                                                                                true,
                                                                            ) {
                                                                                action = Some(StorageAction::DeleteLeftovers(i));
                                                                            }
                                                                        });
                                                                    });
                                                                });
                                                            ui.add_space(6.0);
                                                        }
                                                    
                                                        // ───────── Cache temporanea ─────────
                                                        ui.add_space(6.0);
                                                        ui.separator();
                                                        ui.add_space(6.0);
                                                        ui.label(egui::RichText::new("Temporary Cache").size(14.0).strong());
                                                        ui.add_space(4.0);
                                                        ui.weak(format!(
                                                            "Time-stretched audio for playback speeds other than 1x: {}. Re-created on demand.",
                                                            human_size(stretch_size)
                                                        ));
                                                        ui.add_space(4.0);
                                                        ui.horizontal_wrapped(|ui| {
                                                            if danger_button(
                                                                ui,
                                                                &mut confirm,
                                                                "stretch".to_string(),
                                                                "󰆴 Clear playback-speed cache",
                                                                "Safe: files are regenerated the next time you play at that speed",
                                                                stretch_size > 0,
                                                            ) {
                                                                action = Some(StorageAction::ClearStretchCache);
                                                            }
                                                        });
                                                    
                                                        // ───────── App state ─────────
                                                        ui.add_space(6.0);
                                                        ui.separator();
                                                        ui.add_space(6.0);
                                                        ui.label(egui::RichText::new("App State").size(14.0).strong());
                                                        ui.add_space(4.0);
                                                    
                                                        let state_path = crate::state::AppState::config_path();
                                                        let state_size = std::fs::metadata(&state_path).map(|m| m.len()).unwrap_or(0);
                                                        ui.add(
                                                            egui::Label::new(
                                                                egui::RichText::new(format!(
                                                                    "{}  ·  {}",
                                                                    state_path.display(),
                                                                    human_size(state_size)
                                                                ))
                                                                .size(11.0)
                                                                .weak(),
                                                            )
                                                            .truncate(),
                                                        );
                                                        ui.weak(format!(
                                                            "{} recent file(s) with saved page, zoom and bookmarks.",
                                                            self.app_state.recent_files.len()
                                                        ));
                                                        ui.add_space(4.0);
                                                        ui.horizontal_wrapped(|ui| {
                                                            if danger_button(
                                                                ui,
                                                                &mut confirm,
                                                                "recent".to_string(),
                                                                "󰆴 Clear recent files list",
                                                                "Also forgets last page, zoom and bookmarks of every recent file. Notes and audio are not touched.",
                                                                !self.app_state.recent_files.is_empty(),
                                                            ) {
                                                                action = Some(StorageAction::ClearRecent);
                                                            }
                                                        });
                                                    
                                                        ui.add_space(4.0);
                                                        ui.weak(format!(
                                                            "Notes folder: {}",
                                                            notes::get_backup_dir().display()
                                                        ));
                                                        ui.weak(format!(
                                                            "Audio folder: {}",
                                                            audio::AudioDocIndex::media_root().display()
                                                        ));
                                                    
                                                        // ───────── Esecuzione azione ─────────
                                                        if let Some(act) = action {
                                                            let report = |what: &str, name: &str, r: std::io::Result<()>| -> String {
                                                                match r {
                                                                    Ok(_) => format!("󰄬 {} {}", what, name),
                                                                    Err(e) => format!("󰅚 Error: {}", e),
                                                                }
                                                            };
                                                    
                                                            let msg = match act {
                                                                StorageAction::ClearNotes(i) => report(
                                                                    "Cleared notes of",
                                                                    &entries[i].name,
                                                                    storage_clear_notes(&entries[i]),
                                                                ),
                                                                StorageAction::DeleteOrig(i) => report(
                                                                    "Deleted original copy of",
                                                                    &entries[i].name,
                                                                    storage_delete_orig(&entries[i]),
                                                                ),
                                                                StorageAction::DeleteLecture(i) => {
                                                                    if entries[i].pdf_path.as_ref() == Some(&self.pdf_path) {
                                                                        self.audio_engine.stop_and_release();
                                                                    }
                                                                    report(
                                                                        "Deleted lecture audio of",
                                                                        &entries[i].name,
                                                                        storage_delete_lecture(&entries[i]),
                                                                    )
                                                                }
                                                                StorageAction::DeleteMemos(i) => {
                                                                    if entries[i].pdf_path.as_ref() == Some(&self.pdf_path) {
                                                                        self.memo_audio_engine.stop_and_release();
                                                                    }
                                                                    report(
                                                                        "Deleted voice memos of",
                                                                        &entries[i].name,
                                                                        storage_delete_memos(&entries[i]),
                                                                    )
                                                                }
                                                                StorageAction::DeleteLeftovers(i) => {
                                                                    if entries[i].pdf_path.as_ref() == Some(&self.pdf_path) {
                                                                        self.audio_engine.stop_and_release();
                                                                        self.memo_audio_engine.stop_and_release();
                                                                    }
                                                                    report(
                                                                        "Deleted leftovers of",
                                                                        &entries[i].name,
                                                                        storage_delete_leftovers(&entries[i]),
                                                                    )
                                                                }
                                                                StorageAction::CleanOrphans => {
                                                                    let mut ok = 0usize;
                                                                    let mut freed = 0u64;
                                                                    for en in entries
                                                                        .iter()
                                                                        .filter(|e| e.pdf_path.is_some() && !e.pdf_exists)
                                                                    {
                                                                        if storage_delete_leftovers(en).is_ok() {
                                                                            ok += 1;
                                                                            freed += en.total();
                                                                        }
                                                                    }
                                                                    format!("󰄬 Cleaned {} orphan(s), freed {}", ok, human_size(freed))
                                                                }
                                                                StorageAction::ClearStretchCache => {
                                                                    let freed = stretch_size;
                                                                    match std::fs::remove_dir_all(stretch_cache_dir()) {
                                                                        Ok(_) => format!("󰄬 Cleared cache, freed {}", human_size(freed)),
                                                                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                                                                            "󰄬 Cache already empty".to_string()
                                                                        }
                                                                        Err(e) => format!("󰅚 Error: {}", e),
                                                                    }
                                                                }
                                                                StorageAction::ClearRecent => {
                                                                    self.app_state.recent_files.clear();
                                                                    settings_dirty = true;
                                                                    "󰄬 Recent files list cleared".to_string()
                                                                }
                                                            };
                                                            self.toast_message = Some((msg, std::time::Instant::now()));
                                                            ui.data_mut(|d| d.remove::<Vec<StorageEntry>>(cache_id)); // forza nuova scansione
                                                        }
                                                    
                                                        ui.data_mut(|d| d.insert_temp(confirm_id, confirm));
                                                    }
                                                    SettingsTab::Dictionaries => {
                                                        let confirm_id = ui.make_persistent_id("dict_confirm");
                                                        let words_cache_id = ui.make_persistent_id("dict_user_words_cache");
                                                        let input_id = ui.make_persistent_id("dict_new_word_input");
                                                        let mut confirm: String = ui
                                                            .data(|d| d.get_temp::<String>(confirm_id))
                                                            .unwrap_or_default();
                                                    
                                                        ui.label(egui::RichText::new("Spellcheck Dictionaries").size(15.0).strong());
                                                        ui.add_space(4.0);
                                                        ui.weak("English is built in. Add more languages as Hunspell .dic files or plain word lists (.txt, one word per line).");
                                                        ui.add_space(8.0);
                                                    
                                                        if ui
                                                            .checkbox(
                                                                &mut self.app_state.settings.spellcheck_enabled,
                                                                "Enable offline spellcheck in Markdown notes",
                                                            )
                                                            .changed()
                                                        {
                                                            self.spellchecker.enabled = self.app_state.settings.spellcheck_enabled;
                                                            settings_dirty = true;
                                                        }
                                                    
                                                        ui.add_space(6.0);
                                                        ui.weak(format!(
                                                            "{} words loaded · {}",
                                                            self.spellchecker.word_count(),
                                                            self.spellchecker.loaded_languages.join(", ")
                                                        ));
                                                    
                                                        ui.add_space(8.0);
                                                        ui.separator();
                                                        ui.add_space(6.0);
                                                    
                                                        // ───────── Dizionari ─────────
                                                        ui.label(egui::RichText::new("Installed dictionaries").size(14.0).strong());
                                                        ui.add_space(4.0);
                                                    
                                                        egui::Frame::NONE
                                                            .fill(egui::Color32::from_black_alpha(35))
                                                            .corner_radius(egui::CornerRadius::same(6))
                                                            .inner_margin(egui::Margin::same(8))
                                                            .show(ui, |ui| {
                                                                ui.set_width(ui.available_width());
                                                                ui.label(egui::RichText::new("English (built-in)").strong());
                                                                ui.weak("Always loaded. Cannot be removed.");
                                                            });
                                                        ui.add_space(4.0);
                                                    
                                                        let extras = self.app_state.settings.extra_dictionaries.clone();
                                                        let mut remove_idx: Option<usize> = None;
                                                    
                                                        for (i, p) in extras.iter().enumerate() {
                                                            let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
                                                            let missing = !p.exists();
                                                            let name = p
                                                                .file_stem()
                                                                .map(|s| s.to_string_lossy().to_string())
                                                                .unwrap_or_else(|| p.display().to_string());
                                                    
                                                            egui::Frame::NONE
                                                                .fill(egui::Color32::from_black_alpha(35))
                                                                .corner_radius(egui::CornerRadius::same(6))
                                                                .inner_margin(egui::Margin::same(8))
                                                                .show(ui, |ui| {
                                                                    ui.vertical(|ui| {
                                                                        ui.set_width(ui.available_width());
                                                                        ui.horizontal(|ui| {
                                                                            ui.with_layout(
                                                                                egui::Layout::right_to_left(egui::Align::Center),
                                                                                |ui| {
                                                                                    ui.label(if missing {
                                                                                        "missing".to_string()
                                                                                    } else {
                                                                                        human_size(size)
                                                                                    });
                                                                                    ui.with_layout(
                                                                                        egui::Layout::left_to_right(egui::Align::Center),
                                                                                        |ui| {
                                                                                            ui.add(
                                                                                                egui::Label::new(egui::RichText::new(&name).strong())
                                                                                                    .truncate(),
                                                                                            );
                                                                                        },
                                                                                    );
                                                                                },
                                                                            );
                                                                        });
                                                                        ui.add(
                                                                            egui::Label::new(
                                                                                egui::RichText::new(p.display().to_string()).size(11.0).weak(),
                                                                            )
                                                                            .truncate(),
                                                                        );
                                                                        if missing {
                                                                            ui.label(
                                                                                egui::RichText::new("⚠ File not found, it is skipped at startup")
                                                                                    .size(11.0)
                                                                                    .color(egui::Color32::from_rgb(251, 191, 36)),
                                                                            );
                                                                        }
                                                                        ui.add_space(2.0);
                                                                        ui.horizontal_wrapped(|ui| {
                                                                            if danger_button(
                                                                                ui,
                                                                                &mut confirm,
                                                                                format!("dict:{}", p.display()),
                                                                                "󰆴 Remove",
                                                                                "Removes it from the list. If the copy lives in the app's dictionaries folder it is deleted too.",
                                                                                true,
                                                                            ) {
                                                                                remove_idx = Some(i);
                                                                            }
                                                                        });
                                                                    });
                                                                });
                                                            ui.add_space(4.0);
                                                        }
                                                    
                                                        if extras.is_empty() {
                                                            ui.weak("No extra dictionaries yet.");
                                                        }
                                                    
                                                        ui.add_space(4.0);
                                                        if ui.button("󰐕  Add dictionary...").clicked() {
                                                            self.dialogs.open(
                                                                ui.ctx(),
                                                                DialogKind::AddDictionary,
                                                                DialogSpec::open_file().filter("Dictionary / word list", &["dic", "txt", "dict", "words"]),
                                                            );
                                                        }
                                                    
                                                        if let Some(i) = remove_idx {
                                                            let p = self.app_state.settings.extra_dictionaries.remove(i);
                                                            // Delete only app-owned copies, never external files.
                                                            if p.starts_with(spellcheck::dict_dir()) {
                                                                let _ = std::fs::remove_file(&p);
                                                            }
                                                            self.rebuild_spellchecker();
                                                            settings_dirty = true;
                                                            self.toast_message = Some((
                                                                "󰄬 Dictionary removed".to_string(),
                                                                std::time::Instant::now(),
                                                            ));
                                                        }
                                                    
                                                        // ───────── Parole personali ─────────
                                                        ui.add_space(10.0);
                                                        ui.separator();
                                                        ui.add_space(6.0);
                                                        ui.label(egui::RichText::new("Custom words").size(14.0).strong());
                                                        ui.add_space(2.0);
                                                        ui.weak("Words you added with “Add to dictionary” in the notes editor.");
                                                        ui.add_space(4.0);
                                                    
                                                        let user_words: Vec<String> = match ui.data(|d| d.get_temp::<Vec<String>>(words_cache_id)) {
                                                            Some(v) => v,
                                                            None => {
                                                                let v = spellcheck::read_user_words();
                                                                ui.data_mut(|d| d.insert_temp(words_cache_id, v.clone()));
                                                                v
                                                            }
                                                        };
                                                    
                                                        let mut new_word: String = ui
                                                            .data(|d| d.get_temp::<String>(input_id))
                                                            .unwrap_or_default();
                                                        ui.horizontal(|ui| {
                                                            let te = ui.add(
                                                                egui::TextEdit::singleline(&mut new_word)
                                                                    .hint_text("Add a word...")
                                                                    .desired_width(200.0),
                                                            );
                                                            let enter = te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                                                            if (ui.button("Add").clicked() || enter) && !new_word.trim().is_empty() {
                                                                self.spellchecker.add_user_word(&new_word);
                                                                new_word.clear();
                                                                ui.data_mut(|d| d.remove::<Vec<String>>(words_cache_id));
                                                            }
                                                        });
                                                        ui.data_mut(|d| d.insert_temp(input_id, new_word));
                                                    
                                                        ui.add_space(4.0);
                                                        if user_words.is_empty() {
                                                            ui.weak("No custom words.");
                                                        } else {
                                                            ui.weak(format!("{} word(s) — click one to remove it", user_words.len()));
                                                            ui.add_space(2.0);
                                                    
                                                            let mut remove_word: Option<String> = None;
                                                            ui.horizontal_wrapped(|ui| {
                                                                for w in user_words.iter().take(300) {
                                                                    if ui
                                                                        .small_button(format!("{} ×", w))
                                                                        .on_hover_text("Remove from dictionary")
                                                                        .clicked()
                                                                    {
                                                                        remove_word = Some(w.clone());
                                                                    }
                                                                }
                                                            });
                                                            if user_words.len() > 300 {
                                                                ui.weak(format!("…and {} more", user_words.len() - 300));
                                                            }
                                                    
                                                            if let Some(w) = remove_word {
                                                                let rest: Vec<String> = user_words.iter().filter(|x| **x != w).cloned().collect();
                                                                let _ = spellcheck::write_user_words(&rest);
                                                                self.rebuild_spellchecker();
                                                                ui.data_mut(|d| d.remove::<Vec<String>>(words_cache_id));
                                                            }
                                                    
                                                            ui.add_space(4.0);
                                                            ui.horizontal_wrapped(|ui| {
                                                                if danger_button(
                                                                    ui,
                                                                    &mut confirm,
                                                                    "user_words".to_string(),
                                                                    "󰆴 Clear all custom words",
                                                                    "Deletes your personal word list",
                                                                    true,
                                                                ) {
                                                                    let _ = spellcheck::write_user_words(&[]);
                                                                    self.rebuild_spellchecker();
                                                                    ui.data_mut(|d| d.remove::<Vec<String>>(words_cache_id));
                                                                }
                                                            });
                                                        }
                                                    
                                                        ui.data_mut(|d| d.insert_temp(confirm_id, confirm));
                                                    }

                                                }

                                                if settings_dirty {
                                                    self.app_state.save();
                                                }
                                            }); // End right-panel vertical layout.
                                        });
                                });
                        });
                    }); // End modal vertical layout.
                });
            });
    }
    
    fn render_search_popup(&mut self, ui: &mut egui::Ui) {
        if !self.show_search {
            return;
        }

        let (esc_pressed, enter_pressed) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::Escape),
                i.key_pressed(egui::Key::Enter),
            )
        });

        if esc_pressed {
            self.show_search = false;
            return;
        }

        let mut jump_action: Option<(u32, Option<u64>)> = None;

        egui::Area::new(ui.make_persistent_id("global_search_popover"))
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-16.0, 46.0))
            .order(egui::Order::Foreground)
            .constrain(true)
            .show(ui.ctx(), |ui| {
                self.modal_glass_frame(238).show(ui, |ui| {
                    ui.set_width(420.0);
                    ui.set_max_height(390.0);

                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("󰍉")
                                .size(15.0)
                                .color(rgb(self.theme.accent)),
                        );
                        ui.label(
                            egui::RichText::new("Tri-Modal Search")
                                .size(13.5)
                                .strong(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .small_button("󰅖")
                                .on_hover_text("Close search [Esc]")
                                .clicked()
                            {
                                self.show_search = false;
                            }
                            ui.label(
                                egui::RichText::new("Click results to preview")
                                    .size(11.0)
                                    .weak(),
                            );
                        });
                    });

                    ui.add_space(6.0);

                    let edit_resp = ui.add(
                        egui::TextEdit::singleline(&mut self.search_query)
                            .hint_text("Search Notes, PDF slides, Audio transcripts...")
                            .desired_width(f32::INFINITY),
                    );
                    // Request focus only when first opened.
                    if self.search_query.is_empty() && !edit_resp.has_focus() {
                        edit_resp.request_focus();
                    }

                    ui.add_space(6.0);
                    ui.separator();
                    ui.add_space(4.0);

                    self.ensure_pdf_search_index();
                    let q = self.search_query.trim().to_lowercase();
                    let mut matches: Vec<(u32, String, String, Option<u64>)> = Vec::new();

                    if !q.is_empty() {
                        for p in 1..=self.total_pages {
                            if let Some(note_text) = self.notes.get(&p) {
                                for line in note_text.lines() {
                                    let clean = line.trim();
                                    if clean.to_lowercase().contains(&q) {
                                        let prev: String = clean.chars().take(46).collect();
                                        matches.push((p, " Note".to_string(), prev, None));
                                    }
                                }
                            }
                            if let Some(pdf_text) = self.pdf_full_text_index.get(&p) {
                                for line in pdf_text.lines() {
                                    let clean = line.trim();
                                    if clean.to_lowercase().contains(&q) {
                                        let prev: String = clean.chars().take(46).collect();
                                        matches.push((p, "󰈙 PDF".to_string(), prev, None));
                                    }
                                }
                            }
                        }

                        for seg in &self.audio_index.segments {
                            if seg.text.to_lowercase().contains(&q) {
                                let prev: String = seg.text.chars().take(42).collect();
                                let badge = format!("󰕾 {}", audio::format_ms(seg.start_ms));
                                matches.push((seg.page, badge, prev, Some(seg.start_ms)));
                            }
                        }

                        for (&p, memos) in &self.audio_index.page_memos {
                            for m in memos {
                                if let Some(ref tr) = m.transcript {
                                    if tr.to_lowercase().contains(&q) {
                                        let prev: String = tr.chars().take(42).collect();
                                        matches.push((p, format!("🎙 {}", m.id), prev, None));
                                    }
                                }
                            }
                        }
                    }

                    if enter_pressed && edit_resp.lost_focus() {
                        if let Some((first_p, _, _, first_ts)) = matches.first() {
                            jump_action = Some((*first_p, *first_ts));
                        }
                    }

                    // Keep the window compact when there are few results.
                    egui::ScrollArea::vertical()
                        .id_salt("global_search_results_scroll")
                        .max_height(260.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            if q.is_empty() {
                                ui.add_space(6.0);
                                ui.weak("Type a keyword to filter slides, notes & audio...");
                                ui.add_space(6.0);
                            } else if matches.is_empty() {
                                ui.add_space(6.0);
                                ui.weak("No matches found.");
                                ui.add_space(6.0);
                            } else {
                                ui.label(
                                    egui::RichText::new(format!("{} results found", matches.len()))
                                        .size(11.0)
                                        .weak(),
                                );
                                ui.add_space(2.0);
                                for (p, kind_badge, snippet, opt_ts) in matches {
                                    let is_cur = p == self.current_page;
                                    let label = format!("p.{} [{}] – {}", p, kind_badge, snippet);
                                    let btn = egui::Button::new(
                                        egui::RichText::new(label).size(12.2),
                                    )
                                    .selected(is_cur)
                                    .min_size(egui::vec2(ui.available_width(), 26.0));

                                    if ui.add(btn).clicked() {
                                        jump_action = Some((p, opt_ts));
                                    }
                                }
                            }
                        });
                });
            });

        // Jump to the page or timestamp without closing search.
        if let Some((target_p, opt_ts)) = jump_action {
            self.jump_with_history(target_p, ui.ctx());
            if let Some(ts_ms) = opt_ts {
                if self.audio_engine.total_duration_ms == 0 {
                    if let Some(ref wav_name) = self.audio_index.lecture_wav {
                        let wav_path =
                            audio::AudioDocIndex::media_dir_for_pdf(&self.pdf_path).join(wav_name);
                        let _ = self.audio_engine.load_and_play(
                            &wav_path,
                            "Lecture".to_string(),
                            ts_ms,
                        );
                    }
                } else {
                    self.audio_engine.seek_to_ms(ts_ms);
                }
            }
        }
    }   
    fn render_command_palette(&mut self, ui: &mut egui::Ui) {
        if !self.show_command_palette {
            return;
        }

        let (esc_pressed, down_pressed, up_pressed, enter_pressed) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::Escape),
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::Enter),
            )
        });

        if esc_pressed {
            self.show_command_palette = false;
            return;
        }

        let full_rect = ui.max_rect();
        egui::Area::new(ui.make_persistent_id("cmd_palette_backdrop"))
            .fixed_pos(full_rect.min)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                let (rect, resp) = ui.allocate_exact_size(full_rect.size(), egui::Sense::click());
                ui.painter().rect_filled(
                    rect,
                    egui::CornerRadius::ZERO,
                    egui::Color32::from_black_alpha(150),
                );
                if resp.clicked() {
                    self.show_command_palette = false;
                }
            });

        let commands: &[(&str, &str, &str)] = &[
            ("toggle_split", "󰯌  Toggle Split-View (Side-by-Side PDF)", "View"),
            ("mode_continuous", "󰍜  Switch to Continuous Scroll Mode", "View"),
            ("mode_single", "󰈙  Switch to Single Page Mode", "View"),
            ("mode_toggle_cont_single", "󰍜  Toggle Continuous / Single Page [C]", "View"),
            ("mode_slideshow", "󰐨  Enter Presentation / Slideshow Mode", "View"),
            ("toggle_fullscreen", "󰊓  Toggle Fullscreen [F11]", "View"),
            ("toggle_sketch", "󰠱  Toggle Sketch (page scratchpad) [Alt+S]", "View"),
            ("toggle_dark", "󰔎  Toggle PDF Smart Dark Mode", "Appearance"),
            ("insert_blank", "󰐕  Insert Blank Ruled Page After Current Slide", "Document"),
            ("delete_page", "󰆴  Delete Current Page from PDF", "Document"),
            ("download_orig", "󰇚  Download / Save Original Unmodified PDF...", "Document"),
            ("global_search", "󰍉  Open Global Tri-Modal Search", "Search"),
            ("flashcards", "󰘸  Open Active Recall Flashcards Deck", "Study"),
            ("export_anki", "󰈇  Export Notes Flashcards to Anki (.tsv)", "Study"),
            ("tool_cursor", "󰇀  Tool: Cursor / Text Selection", "Tools"),
            ("tool_pen", "󰏫  Tool: Freehand Pen (Chaikin Smoothed)", "Tools"),
            ("tool_highlight", "  Tool: Highlighter", "Tools"),
            ("tool_tape", "󰝤  Tool: Occlusion Tape (Active Recall)", "Tools"),
            ("tool_laser", "󰐾  Tool: Laser Pointer", "Tools"),
            ("tool_snip", "󰆞  Tool: Snip Region to Markdown Notes", "Tools"),
            ("tool_eraser", "󰇾  Tool: Eraser", "Tools"),
            ("undo", "󰕌  Undo Last Annotation on Current Slide", "Edit"),
            ("redo", "󰑎  Redo Annotation on Current Slide", "Edit"),
        ];

        let q = self.command_palette_query.trim().to_lowercase();

        let direct_page_jump: Option<u32> = q
            .strip_prefix('p')
            .unwrap_or(&q)
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|&p| p >= 1 && p <= self.total_pages.max(1));

        let mut filtered: Vec<(&str, String, &str)> = Vec::new();
        if let Some(target_p) = direct_page_jump {
            filtered.push((
                "__jump_page__",
                format!("󰁔  Jump directly to Slide p.{}", target_p),
                "Navigation",
            ));
        }

        for &(id, label, cat) in commands {
            if q.is_empty()
                || label.to_lowercase().contains(&q)
                || cat.to_lowercase().contains(&q)
                || q.split_whitespace().all(|tok| label.to_lowercase().contains(tok))
            {
                filtered.push((id, label.to_string(), cat));
            }
        }

        if filtered.is_empty() {
            self.command_palette_selected = 0;
        } else {
            if down_pressed {
                self.command_palette_selected = (self.command_palette_selected + 1) % filtered.len();
            }
            if up_pressed {
                self.command_palette_selected = if self.command_palette_selected == 0 {
                    filtered.len() - 1
                } else {
                    self.command_palette_selected - 1
                };
            }
            self.command_palette_selected = self.command_palette_selected.min(filtered.len() - 1);
        }

        let mut triggered_cmd: Option<(String, Option<u32>)> = None;

        egui::Area::new(ui.make_persistent_id("cmd_palette_modal"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 80.0))
            .order(egui::Order::Tooltip)
            .show(ui.ctx(), |ui| {
                self.modal_glass_frame(238).show(ui, |ui| {
                        ui.set_width(540.0);

                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("󰘳")
                                    .size(16.0)
                                    .color(rgb(self.theme.accent)),
                            );
                            let prev_q = self.command_palette_query.clone();
                            let edit_resp = ui.add(
                                egui::TextEdit::singleline(&mut self.command_palette_query)
                                    .hint_text("Type a command or slide number (e.g. 'tape', 'split', 'p14')...")
                                    .desired_width(f32::INFINITY),
                            );
                            edit_resp.request_focus();
                            if self.command_palette_query != prev_q {
                                self.command_palette_selected = 0;
                            }
                        });

                        ui.add_space(6.0);
                        ui.separator();
                        ui.add_space(4.0);

                        egui::ScrollArea::vertical()
                            .id_salt("cmd_palette_scroll")
                            .auto_shrink([false, true])
                            .max_height(280.0)
                            .show(ui, |ui| {
                                if filtered.is_empty() {
                                    ui.weak("No matching command found.");
                                } else {
                                    for (idx, (id, label, cat)) in filtered.iter().enumerate() {
                                        let is_sel = idx == self.command_palette_selected;
                                        let row_resp = ui.horizontal(|ui| {
                                            let btn = egui::Button::new(
                                                egui::RichText::new(label).size(13.0),
                                            )
                                            .selected(is_sel)
                                            .min_size(egui::vec2(ui.available_width() - 85.0, 28.0));

                                            let b_resp = ui.add(btn);
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    ui.label(
                                                        egui::RichText::new(*cat)
                                                            .size(11.0)
                                                            .color(rgb(self.theme.fg_muted)),
                                                    );
                                                },
                                            );
                                            b_resp
                                        });

                                        if row_resp.inner.clicked() {
                                            triggered_cmd = Some(((*id).to_string(), direct_page_jump));
                                        }
                                    }
                                }
                            });
                    });
            });

        if enter_pressed && triggered_cmd.is_none() {
            if let Some((id, _, _)) = filtered.get(self.command_palette_selected) {
                triggered_cmd = Some(((*id).to_string(), direct_page_jump));
            }
        }

        if let Some((cmd_id, opt_page)) = triggered_cmd {
            self.show_command_palette = false;
            self.execute_palette_command(&cmd_id, opt_page, ui.ctx());
        }
    }

fn execute_palette_command(&mut self, cmd_id: &str, opt_page: Option<u32>, ctx: &egui::Context) {
        match cmd_id {
            "__jump_page__" => {
                if let Some(p) = opt_page {
                    self.jump_with_history(p, ctx);
                }
            }
            "toggle_split" => {
                self.split_view_enabled = !self.split_view_enabled;
                if self.split_view_enabled {
                    self.split_secondary_page = self.current_page;
                }
            }
            "mode_continuous" => self.view_mode = ViewMode::Continuous,
            "mode_book" => self.view_mode = ViewMode::TwoPage,
            "mode_single" => self.view_mode = ViewMode::SinglePage,
            "mode_toggle_cont_single" => {
                if self.view_mode == ViewMode::Continuous {
                    self.view_mode = ViewMode::SinglePage;
                } else if !matches!(self.view_mode, ViewMode::Slideshow | ViewMode::SlideshowWithNotes) {
                    self.view_mode = ViewMode::Continuous;
                    self.continuous_scroll_to_page = Some(self.current_page);
                }
            }
            "mode_slideshow" => self.view_mode = ViewMode::Slideshow,
            "toggle_sketch" => self.toggle_sketch_tab(),
            "toggle_fullscreen" => {
                if !matches!(self.view_mode, ViewMode::Slideshow | ViewMode::SlideshowWithNotes) {
                    self.user_fullscreen = !self.user_fullscreen;
                }
            }
            "toggle_dark" => {
                self.pdf_dark_mode = !self.pdf_dark_mode;
                self.page_cache.clear();
                self.thumb_cache.clear();
                ctx.request_repaint();
            }
            "insert_blank" => self.insert_blank_page_after(self.current_page, self.app_state.settings.default_page_style, ctx),
            "delete_page" => self.request_delete_page(self.current_page, ctx),
            "download_orig" => self.prompt_download_original_pdf(ctx),
            "global_search" => {
                self.show_search = true;
            }
            "flashcards" => {
                self.sync_and_save();
                self.open_flashcards_trainer();
            }
            "export_anki" => {
                self.sync_and_save();
                self.export_anki_dialog(ctx);
            }
            "tool_cursor" => self.annotation_tool = AnnotationTool::Cursor,
            "tool_pen" => self.annotation_tool = AnnotationTool::Draw,
            "tool_highlight" => self.annotation_tool = AnnotationTool::Highlight,
            "tool_tape" => self.annotation_tool = AnnotationTool::Tape,
            "tool_laser" => self.annotation_tool = AnnotationTool::Laser,
            "tool_snip" => self.annotation_tool = AnnotationTool::Snip,
            "tool_eraser" => self.annotation_tool = AnnotationTool::Eraser,
            "undo" => self.undo_annotation(self.current_page),
            "redo" => self.redo_annotation(self.current_page),
            "paste_screenshot" => {
                let page = self.current_page;
                self.shots.request_clipboard(ctx, ShotTarget::Pdf { page, center: (0.5, 0.5) });
            }
            _ => {}
        }
    }

    fn audio_toast(&mut self, msg: impl Into<String>) {
        self.toast_message = Some((msg.into(), std::time::Instant::now()));
    }

    /// Loads the ASR model in the background so the first transcription starts right away.
    fn preload_asr_model(&self) {
        if let Some(model) = audio::find_asr_model() {
            thread::spawn(move || audio::preload_asr_model(&model));
        }
    }

    fn toggle_lecture_recording(&mut self, media_dir: &Path) {
        self.confirm_remove_lecture = false;
        if self.audio_engine.rec_target == Some(RecordingTarget::Lecture) {
            let name = self.audio_index.lecture_wav.clone().unwrap_or_else(|| self.audio_index.fresh_wav_name());
            let final_wav = media_dir.join(&name);
            if let Some((_, dur)) = self.audio_engine.stop_recording(&final_wav) {
                self.audio_index.lecture_wav = Some(name);
                self.audio_index.lecture_duration_ms = dur;
                self.audio_index.waveform_peaks = audio::compute_waveform_peaks(&final_wav, 240);
                self.audio_index.page_range = self.audio_index.timeline.iter().map(|t| t.1).min().zip(self.audio_index.timeline.iter().map(|t| t.1).max());
                self.audio_index.save(&self.pdf_path);
                self.preload_asr_model();
                self.audio_toast("󰄬 Lecture recording saved");
            }
        } else if !self.audio_engine.is_recording() {
            let raw_wav = media_dir.join("lecture_raw.wav");
            match self.audio_engine.start_recording(raw_wav, RecordingTarget::Lecture) {
                Ok(()) => {
                    // Slide changes during recording become trusted live markers
                    self.audio_index.segments.clear();
                    self.audio_index.timeline.clear();
                    self.audio_index.timeline.push((0, self.current_page));
                    self.audio_index.timeline_is_live = true;
                    self.audio_index.save(&self.pdf_path);
                }
                Err(e) => self.audio_toast(format!("󰅚 Mic error: {}", e)),
            }
        }
    }

    fn import_lecture_media(&mut self, media_dir: &Path, ctx: &egui::Context) {
        self.dialogs.open(
            ctx,
            DialogKind::ImportLectureMedia {
                media_dir: media_dir.to_path_buf(),
            },
            DialogSpec::open_file().filter(
                "Audio & Video",
                &["mp4", "mkv", "webm", "mp3", "m4a", "wav", "ogg", "flac"],
            ),
        );
    }

    fn finish_import_lecture_media(&mut self, src_path: PathBuf, media_dir: &Path) {
        self.confirm_remove_lecture = false;
        let name = self.audio_index.lecture_wav.clone().unwrap_or_else(|| self.audio_index.fresh_wav_name());
        let out_wav = media_dir.join(&name);
        self.audio_engine.stop_and_release();
        match audio::convert_media_to_16k_wav(&src_path, &out_wav) {
            Ok(dur) => {
                // A new track invalidates the old transcript and markers
                let idx = &mut self.audio_index;
                idx.lecture_wav = Some(name);
                idx.lecture_duration_ms = dur;
                idx.waveform_peaks = audio::compute_waveform_peaks(&out_wav, 240);
                idx.segments.clear();
                idx.timeline.clear();
                idx.timeline_is_live = false;
                idx.save(&self.pdf_path);
                self.show_audio_trimmer = true;
                self.trim_start_ms = 0;
                self.trim_end_ms = dur;
                self.trim_memo_target = None;
                let _ = self.audio_engine.load_and_play(&out_wav, "Lecture".to_string(), 0);
                self.preload_asr_model();
                self.audio_toast("󰄬 Media converted and loaded!");
            }
            Err(e) => self.audio_toast(format!("󰅚 Conversion error: {}", e)),
        }
    }

const AUDIO_BTN_SIZE: f32 = 22.0;

fn render_audio_dock_in_rect(&mut self, ui: &mut egui::Ui, show_ticker: bool) {
    let media_dir = AudioDocIndex::media_dir_for_pdf(&self.pdf_path);
    let mut close_dock_clicked = false;

    if self.audio_dock_open {
        egui::Frame::NONE
            .fill(ui.visuals().panel_fill)
            .stroke(egui::Stroke::NONE)
            .corner_radius(egui::CornerRadius::ZERO)
            .inner_margin(egui::Margin {
                left: 8,
                right: 8,
                top: 1,
                bottom: 1,
            })
            .show(ui, |ui| {
                let avail_w = ui.available_width();
                ui.set_width(avail_w);
                ui.spacing_mut().item_spacing.y = 1.0;

                // Header fisso
                if self.show_audio_handle {
                    self.render_audio_handle(ui);
                }

                ui.horizontal(|ui| {
                    let row_w = ui.available_width();
                    let btn_space = Self::AUDIO_BTN_SIZE + 8.0;
                    let title_max_w = (row_w - btn_space).max(20.0);

                    let header_title = if row_w < 145.0 {
                        "Audio"
                    } else {
                        "Audio recording"
                    };

                    ui.add_sized(
                        [title_max_w, 20.0],
                        egui::Label::new(
                            egui::RichText::new(header_title)
                                .size(10.5)
                                .strong()
                                .color(rgb(self.theme.fg_primary)),
                        )
                        .truncate(),
                    );

                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            let response = Self::audio_fab_button(
                                ui,
                                "×",
                                rgb(self.theme.accent),
                                rgb(self.theme.bg_elevated),
                                rgb(self.theme.border),
                                rgb(self.theme.fg_primary),
                                rgb(self.theme.fg_muted),
                                true,
                                true,
                                Self::AUDIO_BTN_SIZE,
                                "Close Lesson Audio",
                                self.audio_index.lecture_wav.is_some(),
                            );

                            close_dock_clicked = response.clicked();
                        },
                    );
                });

                ui.separator();

                // Fill the remaining vertical space.
                let body_height = ui.available_height();
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), body_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        self.render_audio_body(ui, &media_dir);
                    },
                );
            });
    } else if show_ticker {
        // Full-width transcript row.
        egui::Frame::NONE
            .fill(ui.visuals().panel_fill)
            .stroke(egui::Stroke::NONE)
            .inner_margin(egui::Margin::symmetric(6, 3))
            .show(ui, |ui| {
                self.render_compact_transcript_ticker(ui, &media_dir);
            });
    }

    if close_dock_clicked {
        self.audio_dock_open = false;
    }
}

fn render_compact_transcript_ticker(&mut self, ui: &mut egui::Ui, media_dir: &Path) {
    let wav = self
        .audio_index
        .lecture_wav
        .as_ref()
        .map(|name| media_dir.join(name));

    let playing = self.audio_engine.is_playing();
    let pos_ms = self.audio_engine.current_pos_ms();
    let is_transcribing = self.is_transcribing;

    // ---------------------------------------------------------
    // Current transcript segment.
    // ---------------------------------------------------------
    let seg_opt = if playing || pos_ms > 0 {
        self.audio_index
            .segment_index_at(pos_ms)
            .and_then(|i| self.audio_index.segments.get(i))
            .or_else(|| {
                self.audio_index
                    .segments
                    .iter()
                    .find(|s| s.page == self.current_page)
            })
    } else {
        self.audio_index
            .segments
            .iter()
            .find(|s| s.page == self.current_page)
    }
    .map(|s| (s.start_ms, s.text.clone()));

    // ---------------------------------------------------------
    // Transcription progress.
    // ---------------------------------------------------------
    let progress_pct = self
        .asr_progress
        .filter(|_| is_transcribing)
        .map(|(done, total)| (done as f32 / total.max(1) as f32 * 100.0) as u32);

    let accent = rgb(self.theme.accent);
    let panel_bg = ui.visuals().panel_fill;
    let primary = rgb(self.theme.fg_primary);
    let muted = rgb(self.theme.fg_muted);

    let mut toggle_play = false;
    let mut open_dock = false;

    // ---------------------------------------------------------
    // Full-width transcript ticker.
    // ---------------------------------------------------------
    let full_w = ui.available_width();
    let ultra_narrow = full_w < 150.0;

    egui::Frame::NONE
        .fill(panel_bg)
        .stroke(egui::Stroke::NONE)
        .corner_radius(egui::CornerRadius::ZERO)
        .inner_margin(egui::Margin::symmetric(7, 2))
        .show(ui, |ui| {
            ui.set_width((full_w - 14.0).max(40.0));

            ui.horizontal(|row| {
                row.spacing_mut().item_spacing.x = 6.0;

                // PLAY / PAUSE
                if wav.is_some() {
                    let icon = if playing { "󰏤" } else { "󰐊" };

                    let play_button = egui::Button::new(
                        egui::RichText::new(icon)
                            .size(11.0)
                            .color(egui::Color32::WHITE),
                    )
                    .fill(accent)
                    .stroke(egui::Stroke::NONE)
                    .corner_radius(egui::CornerRadius::same(5));

                    if row
                        .add_sized([20.0, 19.0], play_button)
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text("Play / Pause lesson")
                        .clicked()
                    {
                        toggle_play = true;
                    }
                }

                // Full-width text.
                match (progress_pct, seg_opt.as_ref()) {
                    (Some(pct), _) => {
                        row.spinner();
                        let text_width = row.available_width().max(12.0);
                        let lbl = if ultra_narrow {
                            format!("{}%", pct)
                        } else {
                            format!("Transcribing {}%", pct)
                        };
                        row.add_sized(
                            [text_width, 19.0],
                            egui::Label::new(
                                egui::RichText::new(lbl)
                                    .size(10.0)
                                    .strong()
                                    .color(accent),
                            )
                            .truncate(),
                        );
                    }

                    (None, Some((start_ms, text))) => {
                        // Hide the timestamp in narrow panels.
                        if !ultra_narrow {
                            row.add_sized(
                                [38.0, 19.0],
                                egui::Label::new(
                                    egui::RichText::new(audio::format_ms(*start_ms))
                                        .monospace()
                                        .size(9.5)
                                        .color(accent),
                                )
                                .truncate(),
                            );
                        }

                        let text_width = row.available_width().max(12.0);
                        let text_response = row.add_sized(
                            [text_width, 19.0],
                            egui::Label::new(
                                egui::RichText::new(text).size(10.2).color(primary),
                            )
                            .truncate()
                            .sense(egui::Sense::click()),
                        );

                        if text_response
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text("Open full transcript")
                            .clicked()
                        {
                            open_dock = true;
                        }
                    }

                    (None, None) if is_transcribing => {
                        row.spinner();
                        let text_width = row.available_width().max(12.0);
                        row.add_sized(
                            [text_width, 19.0],
                            egui::Label::new(
                                egui::RichText::new("Preparing transcript...")
                                    .size(10.0)
                                    .color(accent),
                            )
                            .truncate(),
                        );
                    }

                    (None, None) => {
                        let text_width = row.available_width().max(12.0);
                        let resp = row.add_sized(
                            [text_width, 19.0],
                            egui::Label::new(
                                egui::RichText::new("No transcript available")
                                    .size(10.0)
                                    .color(muted),
                            )
                            .truncate()
                            .sense(egui::Sense::click()),
                        );
                        if resp.clicked() {
                            open_dock = true;
                        }
                    }
                }
            });
        });

    if toggle_play {
        if let Some(wav_path) = wav {
            if self.audio_engine.total_duration_ms == 0 {
                let _ = self
                    .audio_engine
                    .load_and_play(&wav_path, "Lecture".to_string(), 0);
            } else {
                self.audio_engine.toggle_play_pause();
            }
        }
    }

    if open_dock {
        self.audio_dock_open = true;
    }
}

fn render_page_voice_memos_strip(&mut self, ui: &mut egui::Ui) {
    if !self.app_state.settings.audio_notes {
        return;
    }

    let media_dir = AudioDocIndex::media_dir_for_pdf(&self.pdf_path);
    let page = self.current_page;

    let Some(memos) = self.audio_index.page_memos.get(&page).cloned() else {
        return;
    };

    if memos.is_empty() {
        return;
    }

    let strip_w = ui.available_width();
    // Use wider thresholds to prevent overlap.
    let show_page_lbl = strip_w >= 235.0;
    let compact_title = strip_w < 185.0;
    let ultra_narrow = strip_w < 135.0;

    let accent = rgb(self.theme.accent);
    let muted = rgb(self.theme.fg_muted);
    let primary = rgb(self.theme.fg_primary);
    let error = rgb(self.theme.error);
    let elevated = rgb(self.theme.bg_elevated);
    let border = rgb(self.theme.border);

    let is_transcribing = self.is_transcribing;
    let is_recording = self.audio_engine.is_recording();

    let memo_is_recording =
        self.audio_engine.rec_target == Some(RecordingTarget::PageMemo(page));

    let lesson_btn_info = self.lesson_button_state();

    let mut rename_text = self.rename_memo_text.clone();
    let active_rename_idx =
        self.rename_memo_target
            .as_ref()
            .and_then(|(target_page, idx)| {
                if *target_page == page && *idx < memos.len() {
                    Some(*idx)
                } else {
                    None
                }
            });

    // Azioni differite
    let mut memo_fab_clicked = false;
    let mut lesson_fab_clicked = false;

    let mut to_play: Option<(PathBuf, String)> = None;
    let mut to_delete: Option<usize> = None;
    let mut to_transcribe: Option<(usize, PathBuf)> = None;
    let mut to_trim: Option<(usize, u64)> = None;

    let mut to_start_rename: Option<(usize, String)> = None;
    let mut to_save_rename: Option<(usize, String)> = None;
    let mut cancel_rename = false;

    // ---------------------------------------------------------
    // Header with memo and lesson controls on the right.
    // ---------------------------------------------------------
    ui.horizontal(|ui| {
        ui.set_height(22.0);
        ui.spacing_mut().item_spacing.x = 4.0;

        ui.label(egui::RichText::new("󰍬").size(11.5).color(accent));

        if !ultra_narrow {
            let hdr_text = if compact_title { "Memos" } else { "Voice Memos" };
            ui.label(
                egui::RichText::new(hdr_text)
                    .size(10.5)
                    .strong()
                    .color(primary),
            );
        }

        ui.label(
            egui::RichText::new(format!("{:02}", memos.len()))
                .size(9.5)
                .monospace()
                .color(accent),
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 5.0;

            // 1. Voice memo recording button.
            let fab_response = Self::audio_fab_button(
                ui,
                if memo_is_recording { "■" } else { "󰍬" },
                if memo_is_recording { error } else { accent },
                elevated,
                border,
                primary,
                muted,
                memo_is_recording,
                memo_is_recording || !is_recording,
                Self::AUDIO_BTN_SIZE,
                if memo_is_recording {
                    "Stop Voice Memo"
                } else {
                    "Record Voice Memo"
                },
                false,
            );
            memo_fab_clicked = fab_response.clicked();

            // 2. Lesson audio button.
            if let Some((l_glyph, l_accent, l_active, l_tooltip)) = lesson_btn_info {
                let lesson_resp = Self::audio_fab_button(
                    ui,
                    l_glyph,
                    l_accent,
                    elevated,
                    border,
                    primary,
                    muted,
                    l_active,
                    true,
                    Self::AUDIO_BTN_SIZE,
                    l_tooltip,
                    self.audio_index.lecture_wav.is_some(),
                );
                lesson_fab_clicked = lesson_resp.clicked();
            }

            if show_page_lbl {
                ui.label(
                    egui::RichText::new(format!("Page {}", page))
                        .size(8.5)
                        .color(muted),
                );
            }
        });
    });

    // ---------------------------------------------------------
    // LISTA
    // ---------------------------------------------------------
    let list_height = (ui.available_height()).max(22.0);

    egui::ScrollArea::vertical()
        .id_salt(("voice_memos_list", page))
        .max_height(list_height)
        .auto_shrink([false, false])
        .show(ui, |list_ui| {
            list_ui.spacing_mut().item_spacing.y = 2.0;

            for (i, memo) in memos.iter().enumerate() {
                let memo_filename = memo.filename.clone();
                let memo_duration = memo.duration_ms;
                let memo_transcript = memo.transcript.clone();

                let display_name = memo
                    .title
                    .as_deref()
                    .filter(|title| !title.trim().is_empty())
                    .unwrap_or(&memo.id)
                    .to_owned();

                let row_width = list_ui.available_width();
                let show_duration = row_width >= 195.0;
                let memo_path = media_dir.join(&memo_filename);
                let memo_is_active = self.memo_audio_engine.active_file.as_ref() == Some(&memo_path);
                let mut memo_pos_ms = if memo_is_active {
                    self.memo_audio_engine.current_pos_ms().min(memo_duration)
                } else {
                    0
                };
                let show_memo_player = memo_is_active && memo_pos_ms < memo_duration;

                list_ui.allocate_ui_with_layout(
                    egui::vec2(row_width, 22.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |row| {
                        row.set_width(row_width);
                        row.spacing_mut().item_spacing.x = 4.0;

                        // -------------------------------------
                        // Independent memo playback.
                        // -------------------------------------
                        let memo_is_loaded = memo_is_active
                            && memo_pos_ms < self.memo_audio_engine.total_duration_ms;
                        let is_this_memo_playing =
                            memo_is_loaded && self.memo_audio_engine.is_playing();

                        let play_icon = if is_this_memo_playing { "󰏤" } else { "󰐊" };

                        let play_button = egui::Button::new(
                            egui::RichText::new(play_icon).size(10.5).color(primary),
                        )
                        .frame(false)
                        .corner_radius(egui::CornerRadius::same(10));

                        if row
                            .add_sized([20.0, 20.0], play_button)
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text(if is_this_memo_playing {
                                "Pause Voice Memo"
                            } else {
                                memo_transcript.as_deref().unwrap_or("Play Voice Memo")
                            })
                            .clicked()
                        {
                            if memo_is_loaded {
                                self.memo_audio_engine.toggle_play_pause();
                            } else {
                                to_play = Some((memo_path.clone(), display_name.clone()));
                            }
                        }

                        // -------------------------------------
                        // NOME O EDITOR DI RINOMINA
                        // -------------------------------------
                        let is_renaming = active_rename_idx == Some(i);
                        // Reserve space for the right-side controls.
                        let right_controls_w = if is_renaming {
                            76.0
                        } else if show_memo_player {
                            22.0
                        } else if show_duration {
                            78.0
                        } else {
                            32.0
                        };
                        let name_width =
                            (row.available_width() - right_controls_w).max(18.0);

                        if is_renaming {
                            let edit_resp = row.add_sized(
                                [name_width, 20.0],
                                egui::TextEdit::singleline(&mut rename_text)
                                    .font(egui::TextStyle::Small)
                                    .margin(egui::vec2(3.0, 1.0)),
                            );

                            if edit_resp.lost_focus()
                                && row.input(|inp| inp.key_pressed(egui::Key::Enter))
                            {
                                to_save_rename =
                                    Some((i, rename_text.trim().to_owned()));
                            }

                            if row
                                .add_sized(
                                    [18.0, 19.0],
                                    egui::Button::new(
                                        egui::RichText::new("✓")
                                            .size(11.0)
                                            .color(accent),
                                    )
                                    .frame(false),
                                )
                                .clicked()
                            {
                                to_save_rename =
                                    Some((i, rename_text.trim().to_owned()));
                            }

                            if row
                                .add_sized(
                                    [18.0, 19.0],
                                    egui::Button::new(
                                        egui::RichText::new("×")
                                            .size(12.0)
                                            .color(muted),
                                    )
                                    .frame(false),
                                )
                                .clicked()
                            {
                                cancel_rename = true;
                            }
                        } else if show_memo_player {
                            let title_width = 42.0_f32.min(name_width);
                            row.add_sized(
                                [title_width, 18.0],
                                egui::Label::new(
                                    egui::RichText::new(&display_name)
                                        .size(9.0)
                                        .color(primary),
                                )
                                .truncate(),
                            )
                            .on_hover_text(&display_name);

                            let time_width = 39.0;
                            let slider_width = (row.available_width()
                                - right_controls_w
                                - time_width
                                - 4.0)
                                .max(18.0);
                            let seek = row.add_sized(
                                [slider_width, 12.0],
                                egui::Slider::new(
                                    &mut memo_pos_ms,
                                    0..=memo_duration.max(1),
                                )
                                .show_value(false)
                                .trailing_fill(true),
                            );
                            if seek.changed() {
                                self.memo_audio_engine.seek_to_ms(memo_pos_ms);
                            }
                            row.add_sized(
                                [time_width, 16.0],
                                egui::Label::new(
                                    egui::RichText::new(format!(
                                        "{}/{}s",
                                        memo_pos_ms / 1000,
                                        memo_duration / 1000
                                    ))
                                    .monospace()
                                    .size(8.0)
                                    .color(muted),
                                ),
                            );
                        } else {
                            row.add_sized(
                                [name_width, 20.0],
                                egui::Label::new(
                                    egui::RichText::new(&display_name)
                                        .size(10.0)
                                        .color(primary),
                                )
                                .truncate(),
                            )
                            .on_hover_text(&display_name);
                        }

                        // -------------------------------------
                        // Duration and menu.
                        // -------------------------------------
                        row.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |right| {
                                right.menu_button("⋯", |menu| {
                                    menu.set_min_width(150.0);

                                    if menu.button("󰑕 Rename").clicked() {
                                        to_start_rename = Some((
                                            i,
                                            memo.title
                                                .clone()
                                                .unwrap_or_else(|| memo.id.clone()),
                                        ));
                                        menu.close();
                                    }

                                    if menu
                                        .add_enabled(
                                            !is_transcribing,
                                            egui::Button::new("󰗊 Transcribe"),
                                        )
                                        .clicked()
                                    {
                                        to_transcribe = Some((
                                            i,
                                            media_dir.join(&memo_filename),
                                        ));
                                        menu.close();
                                    }

                                    if menu.button("󰆐 Trim").clicked() {
                                        to_trim = Some((i, memo_duration));
                                        menu.close();
                                    }

                                    if menu
                                        .add(egui::Button::new(
                                            egui::RichText::new("󰆴 Delete")
                                                .color(error),
                                        ))
                                        .clicked()
                                    {
                                        to_delete = Some(i);
                                        menu.close();
                                    }
                                });

                                if show_duration && !is_renaming && !show_memo_player {
                                    right.label(
                                        egui::RichText::new(audio::format_ms(
                                            memo_duration,
                                        ))
                                        .monospace()
                                        .size(9.0)
                                        .color(muted),
                                    );
                                }
                            },
                        );
                    },
                );

            }
        });

    // ---------------------------------------------------------
    // Apply actions after the UI closures.
    // ---------------------------------------------------------
    if memo_fab_clicked {
        if memo_is_recording {
            self.finish_memo_recording(&media_dir);
        } else if !is_recording {
            let raw_wav = media_dir.join("memo_raw.wav");
            let _ = self
                .audio_engine
                .start_recording(raw_wav, RecordingTarget::PageMemo(page));
        }
    }

    if lesson_fab_clicked {
        self.audio_dock_open = !self.audio_dock_open;
    }

    if let Some((idx, initial_text)) = to_start_rename {
        self.rename_memo_target = Some((page, idx));
        self.rename_memo_text = initial_text;
    }

    if cancel_rename {
        self.rename_memo_target = None;
        self.rename_memo_text.clear();
    }

    if let Some((idx, title)) = to_save_rename {
        if let Some(memo) = self
            .audio_index
            .page_memos
            .get_mut(&page)
            .and_then(|list| list.get_mut(idx))
        {
            let title = title.trim();
            memo.title = if title.is_empty() {
                None
            } else {
                Some(title.to_owned())
            };
            self.audio_index.save(&self.pdf_path);
        }
        self.rename_memo_target = None;
        self.rename_memo_text.clear();
    } else if active_rename_idx.is_some() {
        self.rename_memo_text = rename_text;
    }

    if let Some((path, label)) = to_play {
        let _ = self.memo_audio_engine.load_and_play(&path, label, 0);
    }

    if let Some((idx, path)) = to_transcribe {
        self.start_memo_transcription(page, idx, path, ui.ctx());
    }

    if let Some((idx, duration)) = to_trim {
        self.show_audio_trimmer = true;
        self.audio_dock_open = true;
        self.trim_start_ms = 0;
        self.trim_end_ms = duration.max(1000);
        self.trim_memo_target = Some((page, idx));
    }

    if let Some(idx) = to_delete {
        if let Some(list) = self.audio_index.page_memos.get_mut(&page) {
            if idx < list.len() {
                let removed = list.remove(idx);
                let removed_path = media_dir.join(&removed.filename);
                if self.memo_audio_engine.active_file.as_ref() == Some(&removed_path) {
                    self.memo_audio_engine.stop_and_release();
                }
                let _ = std::fs::remove_file(removed_path);
                self.audio_index.save(&self.pdf_path);
            }
        }
        if self.rename_memo_target == Some((page, idx)) {
            self.rename_memo_target = None;
            self.rename_memo_text.clear();
        }
    }
}

fn render_audio_body(&mut self, ui: &mut egui::Ui, media_dir: &Path) {
    egui::ScrollArea::vertical()
        .id_salt("audio_dock_scroll_area")
        .auto_shrink([false, false])
        .max_height(ui.available_height())
        .show(ui, |ui| {
            let body_w = ui.available_width();
            let compact_controls = body_w < 265.0;
            let ultra_narrow = body_w < 175.0;

            ui.spacing_mut().item_spacing = if compact_controls {
                egui::vec2(4.0, 6.0)
            } else {
                egui::vec2(8.0, 9.0)
            };
            ui.spacing_mut().button_padding = if compact_controls {
                egui::vec2(6.0, 4.0)
            } else {
                egui::vec2(9.0, 5.0)
            };

            let wav_path = self
                .audio_index
                .lecture_wav
                .as_ref()
                .map(|n| media_dir.join(n));
            let is_rec = self.audio_engine.rec_target == Some(RecordingTarget::Lecture);
            let busy = self.is_transcribing;

            if self.audio_index.lecture_wav.is_some() || !self.audio_index.other_lectures.is_empty() {
                self.render_lecture_picker(ui);
            }

            if let Some(wav_path) = wav_path.clone() {
                let is_playing = self.audio_engine.is_playing();
                let mut pos_ms = self.audio_engine.current_pos_ms();
                let total_ms = self
                    .audio_engine
                    .total_duration_ms
                    .max(self.audio_index.lecture_duration_ms)
                    .max(1);

                if self.audio_index.waveform_peaks.is_empty() && wav_path.exists() {
                    self.audio_index.waveform_peaks =
                        audio::compute_waveform_peaks(&wav_path, 200);
                    self.audio_index.save(&self.pdf_path);
                }

                // 1. Player controls and waveform.
                ui.horizontal(|ui| {
                    let play_icon = if is_playing { "󰏤" } else { "󰐊" };
                    let play_btn = egui::Button::new(
                        egui::RichText::new(play_icon)
                            .size(16.0)
                            .color(egui::Color32::WHITE),
                    )
                    .fill(rgb(self.theme.accent))
                    .stroke(egui::Stroke::NONE)
                    .corner_radius(egui::CornerRadius::same(8));

                    if ui
                        .add_sized([30.0, 30.0], play_btn)
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text("Play / Pause")
                        .clicked()
                    {
                        if self.audio_engine.total_duration_ms == 0 {
                            let _ = self
                                .audio_engine
                                .load_and_play(&wav_path, "Lecture".to_string(), 0);
                        } else {
                            self.audio_engine.toggle_play_pause();
                        }
                    }

                    let wave_h = 30.0;
                    let (wave_rect, wave_resp) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width().max(8.0), wave_h),
                        egui::Sense::click_and_drag(),
                    );
                    let wave_resp = wave_resp
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text(format!(
                            "{} / {}",
                            audio::format_ms(pos_ms),
                            audio::format_ms(total_ms)
                        ));
                    let painter = ui.painter_at(wave_rect);

                    // Waveform background.
                    painter.rect_filled(
                        wave_rect,
                        egui::CornerRadius::same(8),
                        rgb(self.theme.bg_canvas),
                    );
                    painter.rect_stroke(
                        wave_rect,
                        egui::CornerRadius::same(8),
                        egui::Stroke::new(1.0, rgb(self.theme.border).gamma_multiply(0.6)),
                        egui::StrokeKind::Inside,
                    );

                    let frac = (pos_ms as f32 / total_ms as f32).clamp(0.0, 1.0);
                    let peaks = &self.audio_index.waveform_peaks;
                    let n = peaks.len().max(1);
                    let pad_x = 6.0;
                    let usable_w = (wave_rect.width() - pad_x * 2.0).max(10.0);
                    let step_x = usable_w / n as f32;

                    for (idx, &pk) in peaks.iter().enumerate() {
                        let x = wave_rect.left() + pad_x + (idx as f32 + 0.5) * step_x;
                        let half_h = (pk * (wave_h * 0.40)).max(1.5);
                        let passed = (idx as f32 / n as f32) <= frac;
                        let col = if passed {
                            rgb(self.theme.accent)
                        } else {
                            rgb(self.theme.fg_muted).gamma_multiply(0.35)
                        };
                        painter.line_segment(
                            [
                                egui::pos2(x, wave_rect.center().y - half_h),
                                egui::pos2(x, wave_rect.center().y + half_h),
                            ],
                            egui::Stroke::new((step_x * 0.55).clamp(1.2, 2.6), col),
                        );
                    }

                    // Slide markers.
                    for &(ts, _) in &self.audio_index.timeline {
                        let tx = wave_rect.left()
                            + pad_x
                            + (ts as f32 / total_ms as f32).clamp(0.0, 1.0) * usable_w;
                        painter.line_segment(
                            [
                                egui::pos2(tx, wave_rect.bottom() - 6.0),
                                egui::pos2(tx, wave_rect.bottom() - 1.0),
                            ],
                            egui::Stroke::new(2.0, rgb(self.theme.warning)),
                        );
                    }

                    // Playhead nitido
                    let head_x = wave_rect.left() + pad_x + frac * usable_w;
                    painter.line_segment(
                        [
                            egui::pos2(head_x, wave_rect.top() + 2.0),
                            egui::pos2(head_x, wave_rect.bottom() - 2.0),
                        ],
                        egui::Stroke::new(1.5, egui::Color32::WHITE),
                    );
                    painter.circle_filled(
                        egui::pos2(head_x, wave_rect.top() + 4.0),
                        3.0,
                        egui::Color32::WHITE,
                    );

                    // Hover scrub preview line
                    if let Some(hover_pos) = wave_resp.hover_pos() {
                        let h_x = hover_pos
                            .x
                            .clamp(wave_rect.left() + pad_x, wave_rect.right() - pad_x);
                        painter.line_segment(
                            [
                                egui::pos2(h_x, wave_rect.top() + 2.0),
                                egui::pos2(h_x, wave_rect.bottom() - 2.0),
                            ],
                            egui::Stroke::new(1.0, egui::Color32::from_white_alpha(70)),
                        );
                    }

                    // Responsive timestamp badge.
                    if wave_rect.width() > 68.0 {
                        let time_str = if wave_rect.width() > 125.0 {
                            format!(
                                "{} / {}",
                                audio::format_ms(pos_ms),
                                audio::format_ms(total_ms)
                            )
                        } else {
                            audio::format_ms(pos_ms)
                        };
                        let time_galley = painter.layout_no_wrap(
                            time_str,
                            egui::FontId::monospace(9.5),
                            egui::Color32::from_gray(235),
                        );
                        let badge_size = time_galley.size() + egui::vec2(8.0, 4.0);
                        let badge_rect = egui::Rect::from_min_size(
                            egui::pos2(
                                wave_rect.right() - badge_size.x - 5.0,
                                wave_rect.top() + 4.0,
                            ),
                            badge_size,
                        );
                        painter.rect_filled(
                            badge_rect,
                            egui::CornerRadius::same(5),
                            egui::Color32::from_black_alpha(175),
                        );
                        painter.galley(
                            badge_rect.min + egui::vec2(4.0, 2.0),
                            time_galley,
                            egui::Color32::WHITE,
                        );
                    }

                    // Keep seeking enabled at narrow widths.
                    if wave_resp.clicked() || wave_resp.dragged() {
                        if let Some(ptr) = wave_resp.interact_pointer_pos() {
                            let new_frac = ((ptr.x - (wave_rect.left() + pad_x)) / usable_w)
                                .clamp(0.0, 1.0);
                            pos_ms = (new_frac * total_ms as f32) as u64;
                            self.audio_engine.seek_to_ms(pos_ms);
                        }
                    }
                });

                // 2. Action Pill Bar
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = if compact_controls { 4.0 } else { 5.0 };

                    const SPEEDS: [f32; 4] = [1.0, 1.25, 1.5, 2.0];
                    let cur = self.audio_engine.speed;
                    let speed_txt = format!("{:.2}x", cur)
                        .trim_end_matches('0')
                        .trim_end_matches('.')
                        .to_string();
                    let speed_btn = egui::Button::new(
                        egui::RichText::new(speed_txt).monospace().size(11.0).strong(),
                    )
                    .corner_radius(egui::CornerRadius::same(6));

                    if ui
                        .add(speed_btn)
                        .on_hover_text("Playback speed")
                        .clicked()
                    {
                        let next = SPEEDS
                            .iter()
                            .position(|s| (s - cur).abs() < 0.05)
                            .map(|i| SPEEDS[(i + 1) % SPEEDS.len()])
                            .unwrap_or(1.0);
                        self.audio_engine.set_speed(next);
                    }

                    ui.toggle_value(
                        &mut self.audio_engine.follow_pdf,
                        if compact_controls { "󰁔" } else { "󰁔 Sync" },
                    )
                    .on_hover_text("Automatically synchronize slides with audio");

                    if let Some(slide_ts) = self.audio_index.time_for_page(self.current_page) {
                        let jmp_txt = if ultra_narrow {
                            "󰁨".to_string()
                        } else {
                            format!("󰁨 {}", audio::format_ms(slide_ts))
                        };
                        let jmp_btn = egui::Button::new(
                            egui::RichText::new(jmp_txt)
                                .size(11.5)
                                .color(rgb(self.theme.accent)),
                        )
                        .fill(rgb(self.theme.accent).gamma_multiply(0.14))
                        .corner_radius(egui::CornerRadius::same(6));

                        if ui
                            .add(jmp_btn)
                            .on_hover_text(format!(
                                "Jump to slide timestamp ({})",
                                audio::format_ms(slide_ts)
                            ))
                            .clicked()
                        {
                            self.audio_engine.seek_to_ms(slide_ts);
                        }
                    } else {
                        let pin_txt = if compact_controls {
                            "󰃀"
                        } else {
                            "󰃀 Pin Slide"
                        };
                        if ui
                            .add(
                                egui::Button::new(egui::RichText::new(pin_txt).size(11.5))
                                    .corner_radius(egui::CornerRadius::same(6)),
                            )
                            .on_hover_text("Associate the current minute of the audio with this slide")
                            .clicked()
                        {
                            self.audio_index.timeline.push((pos_ms, self.current_page));
                            self.audio_index.timeline.sort_unstable_by_key(|k| k.0);
                            self.audio_index.timeline_is_live = true;
                            self.audio_index.save(&self.pdf_path);
                        }
                    }

                    if !ultra_narrow {
                        ui.separator();
                    }

                    let trim_open = self.show_audio_trimmer && self.trim_memo_target.is_none();
                    if ui
                        .add_enabled(
                            !busy,
                            egui::Button::new("󰆐")
                                .selected(trim_open)
                                .corner_radius(egui::CornerRadius::same(6)),
                        )
                        .on_hover_text("Trim lesson start/end")
                        .clicked()
                    {
                        self.show_audio_trimmer = !trim_open;
                        self.trim_start_ms = 0;
                        self.trim_end_ms = total_ms;
                        self.trim_memo_target = None;
                    }

                    if ui
                        .add_enabled(
                            !busy,
                            egui::Button::new("󰖁").corner_radius(egui::CornerRadius::same(6)),
                        )
                        .on_hover_text("Remove silences (timestamps preserved)")
                        .clicked()
                    {
                        self.audio_engine.stop_and_release();
                        match audio::strip_silences_from_lecture(
                            &wav_path,
                            &mut self.audio_index,
                            1800,
                        ) {
                            Ok((old_d, new_d)) => {
                                self.audio_index.save(&self.pdf_path);
                                let _ = self.audio_engine.load_and_play(
                                    &wav_path,
                                    "Lecture".to_string(),
                                    0,
                                );
                                self.audio_toast(format!(
                                    "󰄬 Removed {}s of silence!",
                                    old_d.saturating_sub(new_d) / 1000
                                ));
                            }
                            Err(e) => self.audio_toast(format!("󰅚 {}", e)),
                        }
                    }

                    if ui
                        .add_enabled(
                            !busy && !self.audio_engine.is_recording(),
                            egui::Button::new(if self.confirm_remove_lecture {
                                "󰆴 Confirm"
                            } else {
                                "󰆴"
                            })
                            .corner_radius(egui::CornerRadius::same(6)),
                        )
                        .on_hover_text(if self.confirm_remove_lecture {
                            "Confirm removal of the lesson audio and its transcript"
                        } else {
                            "Remove lesson audio and its transcript"
                        })
                        .clicked()
                    {
                        if self.confirm_remove_lecture {
                            self.audio_engine.stop_and_release();
                            let remove_result = std::fs::remove_file(&wav_path);
                            if remove_result.is_ok() || !wav_path.exists() {
                                let _ = std::fs::remove_file(media_dir.join("lecture_raw.wav"));
                                self.audio_index.remove_active_lecture();
                                self.audio_index.save(&self.pdf_path);
                                self.show_audio_trimmer = false;
                                self.trim_memo_target = None;
                                self.trim_start_ms = 0;
                                self.trim_end_ms = 0;
                                self.transcript_last_seg = None;
                                self.transcript_all = false;
                                self.audio_toast("Lesson audio removed");
                            } else if let Err(err) = remove_result {
                                self.audio_toast(format!("Could not remove lesson audio: {err}"));
                            }
                            self.confirm_remove_lecture = false;
                        } else {
                            self.confirm_remove_lecture = true;
                        }
                    }

                    let can_rec = is_rec || (!self.audio_engine.is_recording() && !busy);
                    let rec_lbl = if is_rec {
                        let txt = if ultra_narrow {
                            "󰓛".to_string()
                        } else {
                            format!(
                                "󰓛 {}",
                                audio::format_ms(self.audio_engine.recording_elapsed_ms())
                            )
                        };
                        egui::RichText::new(txt)
                            .color(rgb(self.theme.error))
                            .strong()
                    } else {
                        egui::RichText::new("󰍬")
                    };
                    if ui
                        .add_enabled(
                            can_rec,
                            egui::Button::new(rec_lbl).corner_radius(egui::CornerRadius::same(6)),
                        )
                        .on_hover_text(if is_rec {
                            "Stop and save recording"
                        } else {
                            "Record a new lesson from the microphone"
                        })
                        .clicked()
                    {
                        self.toggle_lecture_recording(media_dir);
                    }

                    if ui
                        .add_enabled(
                            !self.audio_engine.is_recording() && !busy,
                            egui::Button::new("󰈦").corner_radius(egui::CornerRadius::same(6)),
                        )
                        .on_hover_text("Replace with audio/video file (mp4, mkv, mp3, wav...)")
                        .clicked()
                    {
                        self.import_lecture_media(media_dir, ui.ctx());
                    }

                    if !busy {
                        let tx_label = if compact_controls {
                            "󰗊"
                        } else {
                            "󰗊 Transcribe"
                        };

                        let tx_btn = egui::Button::new(
                            egui::RichText::new(tx_label)
                                .size(11.0)
                                .strong()
                                .color(rgb(self.theme.accent)),
                        )
                        .fill(rgb(self.theme.accent).gamma_multiply(0.15))
                        .stroke(egui::Stroke::new(
                            1.0,
                            rgb(self.theme.accent).gamma_multiply(0.35),
                        ))
                        .corner_radius(egui::CornerRadius::same(6));

                        if ui
                            .add_enabled(!self.audio_engine.is_recording(), tx_btn)
                            .on_hover_text("Transcribe the audio lesson and align it with the slides")
                            .clicked()
                        {
                            self.start_lecture_transcription(wav_path.clone(), ui.ctx());
                        }
                        let has_tx = !self.audio_index.segments.is_empty();
                        let al_label = if compact_controls { "󰌷" } else { "󰌷 Align" };
                        if ui.add_enabled(has_tx && !self.audio_engine.is_recording(),
                                egui::Button::new(al_label).corner_radius(egui::CornerRadius::same(6)))
                            .on_hover_text("Map the transcript to a range of slides")
                            .clicked()
                        {
                            let (a, b) = self.audio_index.page_range.unwrap_or((1, self.total_pages.max(1)));
                            self.align_from = a;
                            self.align_to = b;
                            self.show_align_panel = !self.show_align_panel;
                        }
                    }
                });

                if busy {
                    egui::Frame::NONE
                        .fill(rgb(self.theme.bg_canvas))
                        .corner_radius(egui::CornerRadius::same(6))
                        .inner_margin(egui::Margin::symmetric(8, 5))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                let bar_w = (ui.available_width() - 32.0).max(24.0);
                                if let Some((done, total)) = self.asr_progress {
                                    ui.add(
                                        egui::ProgressBar::new(
                                            done as f32 / total.max(1) as f32,
                                        )
                                        .desired_width(bar_w)
                                        .show_percentage(),
                                    );
                                } else {
                                    let w = (ui.available_width() - 28.0).max(20.0);
                                    ui.add_sized(
                                        [w, 18.0],
                                        egui::Label::new(
                                            egui::RichText::new("Transcribing audio...").size(11.5),
                                        )
                                        .truncate(),
                                    );
                                }
                                if ui
                                    .small_button("󰅖")
                                    .on_hover_text("Cancel (retains partial transcription)")
                                    .clicked()
                                {
                                    self.asr_cancel
                                        .store(true, std::sync::atomic::Ordering::Relaxed);
                                }
                            });
                        });
                }
            } else {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;

                    let compact = ui.available_width() < 235.0;

                    if is_rec {
                        let elapsed = audio::format_ms(
                            self.audio_engine.recording_elapsed_ms(),
                        );

                        if ui.button(format!("■ {}", elapsed)).clicked() {
                            self.toggle_lecture_recording(media_dir);
                        }
                    } else {
                        let record = if compact {
                            "󰍬"
                        } else {
                            "󰍬 Record Lesson"
                        };

                        if ui
                            .add_enabled(
                                !self.audio_engine.is_recording(),
                                egui::Button::new(record),
                            )
                            .on_hover_text("Record a new lesson")
                            .clicked()
                        {
                            self.toggle_lecture_recording(media_dir);
                        }

                        let import = if compact {
                            "󰈦"
                        } else {
                            "󰈦 Import Audio / Video"
                        };

                        if ui
                            .add_enabled(
                                !self.audio_engine.is_recording() && !busy,
                                egui::Button::new(import),
                            )
                            .on_hover_text("Import audio or video")
                            .clicked()
                        {
                            self.import_lecture_media(media_dir, ui.ctx());
                        }
                    }
                });
            }

            if self.show_audio_trimmer {
                self.render_audio_trimmer(ui, media_dir);
            }
            if self.show_align_panel {
                self.render_align_panel(ui);
            }

            // 3. Transcript section with interactive cards.
            if !self.audio_index.segments.is_empty() {
                let avail_tx_w = ui.available_width();
                let compact_tx = avail_tx_w < 245.0;
                let tiny_tx = avail_tx_w < 145.0;

                let title = if tiny_tx {
                    if self.transcript_all {
                        "󰗊".to_string()
                    } else {
                        format!("P.{}", self.current_page)
                    }
                } else if compact_tx {
                    if self.transcript_all {
                        "󰗊 ALL".to_string()
                    } else {
                        format!("󰗊 P.{}", self.current_page)
                    }
                } else if self.transcript_all {
                    "󰗊 Full Transcript".to_string()
                } else {
                    format!("󰗊 Slide {} Transcript", self.current_page)
                };

                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;

                    ui.label(
                        egui::RichText::new(title)
                            .strong()
                            .size(11.5)
                            .color(rgb(self.theme.fg_primary)),
                    );

                    let note_label = if compact_tx {
                        "󰈙"
                    } else {
                        "󰈙 +Note"
                    };

                    let note_btn = egui::Button::new(
                        egui::RichText::new(note_label)
                            .size(10.5)
                            .color(rgb(self.theme.accent)),
                    )
                    .fill(rgb(self.theme.accent).gamma_multiply(0.14))
                    .stroke(egui::Stroke::NONE)
                    .corner_radius(egui::CornerRadius::same(5));

                    if ui
                        .add(note_btn)
                        .on_hover_text("Append transcript to Markdown notes")
                        .clicked()
                    {
                        let lines: Vec<String> = self
                            .audio_index
                            .segments
                            .iter()
                            .filter(|s| s.page == self.current_page)
                            .map(|s| {
                                format!("- **[{}]**: {}", audio::format_ms(s.start_ms), s.text)
                            })
                            .collect();
                        if !lines.is_empty() {
                            if !self.current_note.is_empty() {
                                self.current_note.push_str("\n\n");
                            }
                            self.current_note.push_str("### 󰗊 Lecture Transcript\n");
                            self.current_note.push_str(&lines.join("\n"));
                            self.note_is_dirty = true;
                            self.sync_and_save();
                            self.audio_toast("󰄬 Transcription added to the notes");
                        }
                    }

                    let all_label = if compact_tx { "All" } else { "All Slides" };
                    ui.toggle_value(&mut self.transcript_all, all_label)
                        .on_hover_text("Show transcript for all slides");
                });

                let active = if self.audio_engine.total_duration_ms > 0 {
                    let pos = self.audio_engine.current_pos_ms();
                    self.audio_index.segment_index_at(pos)
                } else {
                    None
                };
                let scroll_to_active = active != self.transcript_last_seg;
                self.transcript_last_seg = active;

                let rows: Vec<(usize, u64, String)> = self
                    .audio_index
                    .segments
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| self.transcript_all || s.page == self.current_page)
                    .map(|(i, s)| (i, s.start_ms, s.text.clone()))
                    .collect();

                egui::Frame::NONE
                    .fill(rgb(self.theme.bg_canvas))
                    .corner_radius(egui::CornerRadius::same(8))
                    .inner_margin(egui::Margin::symmetric(6, 6))
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("audio_transcript_rows")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 3.0;
                                if rows.is_empty() {
                                    ui.horizontal_wrapped(|ui| {
                                        ui.weak(
                                            "No transcript segments associated with this slide.",
                                        );
                                        if ui.small_button("Show all slides").clicked() {
                                            self.transcript_all = true;
                                        }
                                    });
                                } else {
                                    let narrow_rows = ui.available_width() < 155.0;

                                    for (i, start_ms, text) in rows {
                                        let is_active = Some(i) == active;
                                        let row_bg = if is_active {
                                            rgb(self.theme.accent).gamma_multiply(0.16)
                                        } else {
                                            egui::Color32::TRANSPARENT
                                        };

                                        let frame_resp = egui::Frame::NONE
                                            .fill(row_bg)
                                            .corner_radius(egui::CornerRadius::same(6))
                                            .inner_margin(egui::Margin::symmetric(8, 5))
                                            .show(ui, |ui| {
                                                ui.set_width(ui.available_width());

                                                let ts_col = if is_active {
                                                    rgb(self.theme.accent)
                                                } else {
                                                    rgb(self.theme.fg_muted)
                                                };
                                                let txt_col = if is_active {
                                                    rgb(self.theme.fg_primary)
                                                } else {
                                                    rgb(self.theme.fg_primary)
                                                        .gamma_multiply(0.88)
                                                };
                                                let rt = egui::RichText::new(text)
                                                    .size(11.8)
                                                    .color(txt_col);

                                                // In narrow panels, place the timestamp
                                                // above the text to give it the full row.
                                                if narrow_rows {
                                                    ui.vertical(|ui| {
                                                        ui.spacing_mut().item_spacing.y = 2.0;
                                                        ui.label(
                                                            egui::RichText::new(
                                                                audio::format_ms(start_ms),
                                                            )
                                                            .monospace()
                                                            .size(9.5)
                                                            .color(ts_col),
                                                        );
                                                        ui.add(
                                                            egui::Label::new(if is_active {
                                                                rt.strong()
                                                            } else {
                                                                rt
                                                            })
                                                            .wrap(),
                                                        );
                                                    });
                                                } else {
                                                    ui.horizontal(|ui| {
                                                        ui.spacing_mut().item_spacing.x = 8.0;
                                                        ui.label(
                                                            egui::RichText::new(
                                                                audio::format_ms(start_ms),
                                                            )
                                                            .monospace()
                                                            .size(10.5)
                                                            .color(ts_col),
                                                        );
                                                        ui.add(
                                                            egui::Label::new(if is_active {
                                                                rt.strong()
                                                            } else {
                                                                rt
                                                            })
                                                            .wrap(),
                                                        );
                                                    });
                                                }
                                            });

                                        let row_rect = frame_resp.response.rect;
                                        if is_active {
                                            let bar = egui::Rect::from_min_size(
                                                egui::pos2(
                                                    row_rect.left() + 1.5,
                                                    row_rect.top() + 4.0,
                                                ),
                                                egui::vec2(
                                                    2.5,
                                                    (row_rect.height() - 8.0).max(4.0),
                                                ),
                                            );
                                            ui.painter().rect_filled(
                                                bar,
                                                egui::CornerRadius::same(1),
                                                rgb(self.theme.accent),
                                            );
                                        }

                                        let interact = ui
                                            .interact(
                                                row_rect,
                                                ui.make_persistent_id(("tx_row", i)),
                                                egui::Sense::click(),
                                            )
                                            .on_hover_cursor(egui::CursorIcon::PointingHand);

                                        if interact.hovered() && !is_active {
                                            ui.painter().rect_filled(
                                                row_rect,
                                                egui::CornerRadius::same(6),
                                                egui::Color32::from_white_alpha(8),
                                            );
                                        }

                                        if is_active && scroll_to_active {
                                            interact.scroll_to_me(Some(egui::Align::Center));
                                        }
                                        if interact.clicked() {
                                            self.audio_engine.seek_to_ms(start_ms);
                                        }
                                    }
                                }
                            });
                    });
            }
        });
}

fn render_audio_handle(&mut self, ui: &mut egui::Ui) {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 5.0),
        egui::Sense::click_and_drag(),
    );

    let hovered = response.hovered();
    let dragging = response.dragged();

    ui.painter().rect_filled(
        egui::Rect::from_center_size(
            rect.center(),
            egui::vec2(
                if hovered || dragging { 38.0 } else { 26.0 },
                4.0,
            ),
        ),
        egui::CornerRadius::same(2),
        if hovered || dragging {
            rgb(self.theme.accent)
        } else {
            rgb(self.theme.fg_muted).gamma_multiply(0.45)
        },
    );

    if dragging {
        let delta = response.drag_delta().y;

        self.audio_dock_height =
            (self.audio_dock_height - delta).clamp(100.0, 600.0);
    }

    response
        .on_hover_cursor(egui::CursorIcon::ResizeVertical)
        .on_hover_text("Drag to resize Lesson Audio");
}

/// Return the lesson audio button state and icon.
fn lesson_button_state(&self) -> Option<(&'static str, egui::Color32, bool, &'static str)> {
    // Keep the button visible when the audio dock is enabled,
    // even when no lesson audio is loaded.
    if !self.app_state.settings.audio_dock {
        return None;
    }

    let is_rec = self.audio_engine.rec_target == Some(RecordingTarget::Lecture);
    let playing = self.audio_engine.is_playing();
    let has_wav = self.audio_index.lecture_wav.is_some();

    let accent = if is_rec {
        rgb(self.theme.error)
    } else {
        rgb(self.theme.accent)
    };

    // Show a suitable icon when no audio or transcript is loaded.
    // The button still opens the lesson audio dock.
    let glyph = if is_rec {
        "●"
    } else if playing && has_wav {
        "󰏤"
    } else if has_wav {
        "󰕾"
    } else {
        "󰋋"
    };

    let active = self.audio_dock_open || is_rec || playing;
    let tooltip = if self.audio_dock_open {
        "Close Lesson Audio"
    } else {
        "Open Lesson Audio"
    };

    Some((glyph, accent, active, tooltip))
}


fn render_floating_audio_fabs(
    &mut self,
    ctx: &egui::Context,
    bottom_rect: egui::Rect,
    media_dir: &Path,
) {
    let page = self.current_page;

    let show_memo = self.app_state.settings.audio_notes && self.show_memo_fab;
    // Keep the lesson audio button visible beside the ticker.
    // Hide it only when the full dock is open.
    let lesson_info = self.lesson_button_state();

    if !show_memo && lesson_info.is_none() {
        return;
    }

    let is_rec_memo =
        self.audio_engine.rec_target == Some(RecordingTarget::PageMemo(page));

    // Anchor the controls 6 px above `bottom_rect.top()`.
    // Place it above the ticker when visible.
    // Otherwise place it above the panel bottom.
    let anchor = egui::pos2(bottom_rect.right() - 8.0, bottom_rect.top() - 6.0);

    let mut memo_clicked = false;
    let mut lesson_clicked = false;

    egui::Area::new(egui::Id::new("floating_audio_fabs_cluster"))
        // Keep the floating controls above panels and notes.
        .order(egui::Order::Tooltip)
        .pivot(egui::Align2::RIGHT_BOTTOM)
        .fixed_pos(anchor)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;

                // 1. Lesson audio button.
                if let Some((l_glyph, l_accent, l_active, l_tooltip)) = lesson_info {
                    if Self::audio_fab_button(
                        ui,
                        l_glyph,
                        l_accent,
                        rgb(self.theme.bg_elevated),
                        rgb(self.theme.border),
                        rgb(self.theme.fg_primary),
                        rgb(self.theme.fg_muted),
                        l_active,
                        true,
                        Self::AUDIO_BTN_SIZE,
                        l_tooltip,
                        self.audio_index.lecture_wav.is_some(),
                    )
                    .clicked()
                    {
                        lesson_clicked = true;
                    }
                }

                // 2. Voice memo recording button.
                if show_memo {
                    let accent = if is_rec_memo {
                        rgb(self.theme.error)
                    } else {
                        rgb(self.theme.accent)
                    };

                    if Self::audio_fab_button(
                        ui,
                        if is_rec_memo { "■" } else { "󰍬" },
                        accent,
                        rgb(self.theme.bg_elevated),
                        rgb(self.theme.border),
                        rgb(self.theme.fg_primary),
                        rgb(self.theme.fg_muted),
                        is_rec_memo,
                        is_rec_memo || !self.audio_engine.is_recording(),
                        Self::AUDIO_BTN_SIZE,
                        if is_rec_memo {
                            "Stop Voice Memo"
                        } else {
                            "Record Voice Memo"
                        },
                        false,
                    )
                    .clicked()
                    {
                        memo_clicked = true;
                    }
                }
            });
        });

    if lesson_clicked {
        self.audio_dock_open = !self.audio_dock_open;
    }

    if memo_clicked {
        if is_rec_memo {
            self.finish_memo_recording(media_dir);
        } else if !self.audio_engine.is_recording() {
            let raw_wav = media_dir.join("memo_raw.wav");
            let _ = self
                .audio_engine
                .start_recording(raw_wav, RecordingTarget::PageMemo(page));
        }
    }
}

fn audio_fab_button(
    ui: &mut egui::Ui,
    glyph: &str,
    accent: egui::Color32,
    base: egui::Color32,
    border: egui::Color32,
    foreground: egui::Color32,
    muted: egui::Color32,
    active: bool,
    enabled: bool,
    size: f32,
    tooltip: &str,
    has_content: bool, // True when lesson audio is available.
) -> egui::Response {
    ui.scope(|ui| {
        // Use the content color when audio is available.
        let effective_accent = if has_content && !active {
            foreground
        } else {
            accent
        };

        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(size, size),
            egui::Sense::click(),
        );

        let hovered = resp.hovered();

        // Compute background and border colors.
        let bg_fill = if hovered {
            effective_accent.gamma_multiply(0.24)
        } else if active {
            effective_accent.gamma_multiply(0.30)
        } else if has_content {
            effective_accent.gamma_multiply(0.12)
        } else {
            base.gamma_multiply(0.92)
        };

        let stroke_col = if hovered || active {
            effective_accent.gamma_multiply(0.9)
        } else if has_content {
            effective_accent.gamma_multiply(0.5)
        } else {
            border.gamma_multiply(0.55)
        };

        // Draw the circular button background.
        ui.painter().circle(
            rect.center(),
            size / 2.0,
            bg_fill,
            egui::Stroke::new(1.0, stroke_col),
        );

        let icon_color = if !enabled {
            muted
        } else if active || has_content {
            effective_accent
        } else {
            foreground
        };

        let icon_galley = ui.painter().layout_no_wrap(
            glyph.to_owned(),
            egui::FontId::monospace(12.0),
            icon_color,
        );
        ui.painter().galley(
            rect.center() - icon_galley.size() * 0.5,
            icon_galley,
            icon_color,
        );

        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(tooltip)
    }).inner
}

    fn render_audio_trimmer(&mut self, ui: &mut egui::Ui, media_dir: &Path) {
        let avail_w = ui.available_width();
        let h_margin = if avail_w < 165.0 { 6 } else { 10 };

        egui::Frame::NONE
            .fill(rgb(self.theme.bg_canvas))
            .stroke(egui::Stroke::new(
                1.0,
                rgb(self.theme.accent).gamma_multiply(0.5),
            ))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::symmetric(h_margin, 8))
            .show(ui, |ui| {
                let inner_w = ui.available_width();
                let compact = inner_w < 215.0;
                let ultra_narrow = inner_w < 150.0;

                ui.spacing_mut().item_spacing = if compact {
                    egui::vec2(4.0, 5.0)
                } else {
                    egui::vec2(6.0, 6.0)
                };

                let (full_title, short_title, max_dur_ms, target_wav) = match self.trim_memo_target {
                    None => (
                        "󰆐 Trim Lecture".to_string(),
                        "󰆐 Trim".to_string(),
                        self.audio_index.lecture_duration_ms.max(1),
                        self.audio_index.lecture_wav.as_ref().map(|n| media_dir.join(n)),
                    ),
                    Some((p, idx)) => {
                        let memo = self.audio_index.page_memos.get(&p).and_then(|v| v.get(idx));
                        match memo {
                            Some(m) => {
                                let name = m
                                    .title
                                    .as_deref()
                                    .filter(|t| !t.trim().is_empty())
                                    .unwrap_or(&m.id);
                                (
                                    format!("󰆐 Trim {} (p.{})", name, p),
                                    "󰆐 Trim".to_string(),
                                    m.duration_ms.max(1),
                                    Some(media_dir.join(&m.filename)),
                                )
                            }
                            None => {
                                self.show_audio_trimmer = false;
                                return;
                            }
                        }
                    }
                };

                self.trim_end_ms = self.trim_end_ms.clamp(200, max_dur_ms);
                self.trim_start_ms = self
                    .trim_start_ms
                    .clamp(0, self.trim_end_ms.saturating_sub(200));
                let cur_pos = if self.trim_memo_target.is_some() {
                    self.memo_audio_engine.current_pos_ms()
                } else {
                    self.audio_engine.current_pos_ms()
                };
                let kept_ms = self.trim_end_ms.saturating_sub(self.trim_start_ms);
                let kept_str = audio::format_ms(kept_ms);

                // ---------------------------------------------------------
                // Responsive header without close-button overlap.
                // ---------------------------------------------------------
                ui.horizontal(|ui| {
                    let row_w = ui.available_width();
                    let close_btn_space = 24.0;
                    let left_max_w = (row_w - close_btn_space).max(24.0);

                    ui.allocate_ui_with_layout(
                        egui::vec2(left_max_w, 18.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |left_ui| {
                            left_ui.set_max_width(left_max_w);
                            left_ui.spacing_mut().item_spacing.x = 5.0;

                            if ultra_narrow {
                                left_ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(format!("󰆐 {}", kept_str))
                                            .strong()
                                            .monospace()
                                            .size(10.5)
                                            .color(rgb(self.theme.accent)),
                                    )
                                    .truncate(),
                                );
                            } else {
                                let hdr_txt = if compact { &short_title } else { &full_title };
                                left_ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(hdr_txt)
                                            .strong()
                                            .size(11.0)
                                            .color(rgb(self.theme.accent)),
                                    )
                                    .truncate(),
                                );

                                let sel_txt = if compact {
                                    kept_str.clone()
                                } else {
                                    format!("Selected: {}", kept_str)
                                };
                                if left_ui.available_width() > 28.0 {
                                    left_ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(sel_txt)
                                                .monospace()
                                                .size(10.0)
                                                .color(rgb(self.theme.fg_muted)),
                                        )
                                        .truncate(),
                                    );
                                }
                            }
                        },
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .small_button("󰅖")
                            .on_hover_text("Close trimmer without cutting")
                            .clicked()
                        {
                            self.show_audio_trimmer = false;
                        }
                    });
                });

                // ---------------------------------------------------------
                // Timeline with trim handles.
                // ---------------------------------------------------------
                let bar_h = 28.0;
                let (bar_rect, bar_resp) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width().max(8.0), bar_h),
                    egui::Sense::click_and_drag(),
                );
                let bar_resp = bar_resp
                    .on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
                    .on_hover_text(format!(
                        "In: {}  |  Out: {}  (Selected: {})",
                        audio::format_ms(self.trim_start_ms),
                        audio::format_ms(self.trim_end_ms),
                        kept_str
                    ));

                let start_frac = (self.trim_start_ms as f32 / max_dur_ms as f32).clamp(0.0, 1.0);
                let end_frac = (self.trim_end_ms as f32 / max_dur_ms as f32).clamp(0.0, 1.0);
                let play_frac = (cur_pos as f32 / max_dur_ms as f32).clamp(0.0, 1.0);

                let x_start = bar_rect.left() + start_frac * bar_rect.width();
                let x_end = bar_rect.left() + end_frac * bar_rect.width();
                let x_play = bar_rect.left() + play_frac * bar_rect.width();

                if bar_resp.clicked() || bar_resp.dragged() {
                    if let Some(ptr) = ui.input(|i| i.pointer.interact_pos()) {
                        let frac = ((ptr.x - bar_rect.left()) / bar_rect.width()).clamp(0.0, 1.0);
                        let target_ms = (frac * max_dur_ms as f32) as u64;
                        let dist_to_start = (ptr.x - x_start).abs();
                        let dist_to_end = (ptr.x - x_end).abs();

                        if dist_to_start <= dist_to_end {
                            self.trim_start_ms =
                                target_ms.min(self.trim_end_ms.saturating_sub(200));
                        } else {
                            self.trim_end_ms =
                                target_ms.max(self.trim_start_ms + 200).min(max_dur_ms);
                        }
                    }
                }

                let painter = ui.painter_at(bar_rect);
                painter.rect_filled(
                    bar_rect,
                    egui::CornerRadius::same(6),
                    rgb(self.theme.bg_panel),
                );

                if self.trim_memo_target.is_none() && !self.audio_index.waveform_peaks.is_empty() {
                    let peaks = &self.audio_index.waveform_peaks;
                    let n = peaks.len().max(1);
                    let step_x = bar_rect.width() / n as f32;
                    for (idx, &pk) in peaks.iter().enumerate() {
                        let x = bar_rect.left() + (idx as f32 + 0.5) * step_x;
                        let half_h = (pk * (bar_h * 0.40)).max(1.0);
                        let in_keep = x >= x_start && x <= x_end;
                        let col = if in_keep {
                            rgb(self.theme.accent)
                        } else {
                            egui::Color32::from_gray(70)
                        };
                        painter.line_segment(
                            [
                                egui::pos2(x, bar_rect.center().y - half_h),
                                egui::pos2(x, bar_rect.center().y + half_h),
                            ],
                            egui::Stroke::new((step_x * 0.55).clamp(1.0, 2.2), col),
                        );
                    }
                }

                // Selected region and dimmed trim areas.
                let keep_rect = egui::Rect::from_min_max(
                    egui::pos2(x_start, bar_rect.top()),
                    egui::pos2(x_end, bar_rect.bottom()),
                );
                painter.rect_filled(
                    keep_rect,
                    egui::CornerRadius::same(3),
                    rgb(self.theme.accent).gamma_multiply(0.14),
                );
                painter.rect_stroke(
                    keep_rect,
                    egui::CornerRadius::same(3),
                    egui::Stroke::new(1.5, rgb(self.theme.accent)),
                    egui::StrokeKind::Inside,
                );

                if x_start > bar_rect.left() + 1.0 {
                    let cut_l = egui::Rect::from_min_max(
                        bar_rect.left_top(),
                        egui::pos2(x_start, bar_rect.bottom()),
                    );
                    painter.rect_filled(
                        cut_l,
                        egui::CornerRadius::same(4),
                        egui::Color32::from_black_alpha(155),
                    );
                }
                if x_end < bar_rect.right() - 1.0 {
                    let cut_r = egui::Rect::from_min_max(
                        egui::pos2(x_end, bar_rect.top()),
                        bar_rect.right_bottom(),
                    );
                    painter.rect_filled(
                        cut_r,
                        egui::CornerRadius::same(4),
                        egui::Color32::from_black_alpha(155),
                    );
                }

                painter.line_segment(
                    [
                        egui::pos2(x_play, bar_rect.top()),
                        egui::pos2(x_play, bar_rect.bottom()),
                    ],
                    egui::Stroke::new(1.2, egui::Color32::from_white_alpha(190)),
                );

                // Handles with center notches.
                for &hx in &[x_start, x_end] {
                    let grip = egui::Rect::from_center_size(
                        egui::pos2(hx, bar_rect.center().y),
                        egui::vec2(6.0, bar_h - 2.0),
                    );
                    painter.rect_filled(
                        grip,
                        egui::CornerRadius::same(3),
                        rgb(self.theme.accent),
                    );
                    painter.line_segment(
                        [
                            egui::pos2(hx, bar_rect.center().y - 4.0),
                            egui::pos2(hx, bar_rect.center().y + 4.0),
                        ],
                        egui::Stroke::new(1.0, rgb(self.theme.bg_canvas)),
                    );
                }

                // ---------------------------------------------------------
                // CONTROLLI INFERIORI (DragValue + Set + Preview + Apply)
                // ---------------------------------------------------------
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = if compact { 4.0 } else { 6.0 };

                    let in_prefix = if ultra_narrow { "" } else { "In " };
                    let out_prefix = if ultra_narrow { "" } else { "Out " };

                    let mut start_sec = self.trim_start_ms as f64 / 1000.0;
                    let max_start_sec =
                        (self.trim_end_ms.saturating_sub(200) as f64 / 1000.0).max(0.0);
                    if ui
                        .add(
                            egui::DragValue::new(&mut start_sec)
                                .prefix(in_prefix)
                                .suffix("s")
                                .speed(0.1)
                                .fixed_decimals(1)
                                .range(0.0..=max_start_sec),
                        )
                        .on_hover_text("Drag to fine-tune Start time")
                        .changed()
                    {
                        self.trim_start_ms = (start_sec * 1000.0) as u64;
                    }
                    if ui
                        .small_button("[󰁨")
                        .on_hover_text("Set Start point to current playhead")
                        .clicked()
                    {
                        self.trim_start_ms = cur_pos.min(self.trim_end_ms.saturating_sub(200));
                    }

                    let mut end_sec = self.trim_end_ms as f64 / 1000.0;
                    let min_end_sec = ((self.trim_start_ms + 200) as f64 / 1000.0)
                        .min(max_dur_ms as f64 / 1000.0);
                    let max_end_sec = (max_dur_ms as f64 / 1000.0).max(0.2);
                    if ui
                        .add(
                            egui::DragValue::new(&mut end_sec)
                                .prefix(out_prefix)
                                .suffix("s")
                                .speed(0.1)
                                .fixed_decimals(1)
                                .range(min_end_sec..=max_end_sec),
                        )
                        .on_hover_text("Drag to fine-tune End time")
                        .changed()
                    {
                        self.trim_end_ms = (end_sec * 1000.0) as u64;
                    }
                    if ui
                        .small_button("󰁨]")
                        .on_hover_text("Set End point to current playhead")
                        .clicked()
                    {
                        self.trim_end_ms = cur_pos.max(self.trim_start_ms + 200).min(max_dur_ms);
                    }

                    let preview_lbl = if compact { "󰐊" } else { "󰐊 Preview" };
                    if ui
                        .small_button(preview_lbl)
                        .on_hover_text("Preview audio from the selected Start point")
                        .clicked()
                    {
                        if let Some(p) = &target_wav {
                            let engine = if self.trim_memo_target.is_some() {
                                &mut self.memo_audio_engine
                            } else {
                                &mut self.audio_engine
                            };
                            let _ = engine.load_and_play(
                                p,
                                "Preview".to_string(),
                                self.trim_start_ms,
                            );
                        }
                    }

                    let cut_lbl = if ultra_narrow {
                        "󰆐"
                    } else if compact {
                        "󰆐 Cut"
                    } else {
                        "󰆐 Apply Cut"
                    };

                    let cut_btn = egui::Button::new(
                        egui::RichText::new(cut_lbl)
                            .size(11.0)
                            .strong()
                            .color(rgb(self.theme.success)),
                    )
                    .fill(rgb(self.theme.success).gamma_multiply(0.15))
                    .stroke(egui::Stroke::new(
                        1.0,
                        rgb(self.theme.success).gamma_multiply(0.4),
                    ))
                    .corner_radius(egui::CornerRadius::same(5));

                    if ui
                        .add(cut_btn)
                        .on_hover_text("Permanently trim audio to [In .. Out]")
                        .clicked()
                    {
                        if let Some(wav) = target_wav {
                            self.apply_audio_trim(&wav);
                        }
                    }
                });
            });
    }

    fn apply_audio_trim(&mut self, wav: &Path) {
        if self.trim_memo_target.is_some() {
            self.memo_audio_engine.stop_and_release();
        } else {
            self.audio_engine.stop_and_release();
        }
        match audio::trim_wav_file(wav, self.trim_start_ms, self.trim_end_ms) {
            Ok(new_dur) => {
                match self.trim_memo_target {
                    None => {
                        self.audio_index
                            .apply_lecture_trim(self.trim_start_ms, self.trim_end_ms);
                        self.audio_index.lecture_duration_ms = new_dur;
                        self.audio_index.waveform_peaks = audio::compute_waveform_peaks(wav, 240);
                        let _ =
                            self.audio_engine
                                .load_and_play(wav, "Lecture".to_string(), 0);
                    }
                    Some((p, idx)) => {
                        if let Some(m) = self
                            .audio_index
                            .page_memos
                            .get_mut(&p)
                            .and_then(|v| v.get_mut(idx))
                        {
                            m.duration_ms = new_dur;
                        }
                    }
                }
                self.audio_index.save(&self.pdf_path);
                self.show_audio_trimmer = false;
                self.audio_toast(format!("󰄬 Audio trimmed ({})", audio::format_ms(new_dur)));
            }
            Err(e) => self.audio_toast(format!("󰅚 Trim error: {}", e)),
        }
    }

    fn finish_memo_recording(&mut self, media_dir: &Path) {
        let page = self.current_page;
        let list = self.audio_index.page_memos.entry(page).or_default();
        let idx = list.len() + 1;
        let fname = format!("memo_p{}_{}.wav", page, idx);
        if let Some((_, dur)) = self.audio_engine.stop_recording(&media_dir.join(&fname)) {
            list.push(audio::VoiceMemoEntry {
                id: format!("Memo #{}", idx),
                filename: fname,
                duration_ms: dur,
                transcript: None,
                title: None,
            });
            self.audio_index.save(&self.pdf_path);
            self.preload_asr_model();
        }
    }

    fn ensure_asr_model(&mut self, ctx: &egui::Context, retry: AsrRetry) -> Option<PathBuf> {
        if let Some(p) = &self.asr_model_dir {
            return Some(p.clone());
        }
        if let Some(p) = audio::find_asr_model() {
            return Some(p);
        }
        // Open the folder picker without blocking; transcription resumes in
        // `handle_dialog_result` after the model is selected.
        self.dialogs.open(
            ctx,
            DialogKind::PickAsrModelDir { retry },
            DialogSpec::pick_folder()
                .title("Select the Parakeet model folder (vocab.txt + encoder/decoder .onnx)"),
        );
        None
    }

    fn render_lecture_picker(&mut self, ui: &mut egui::Ui) {
        self.audio_index.ensure_lecture_ids();

        let ix = &self.audio_index;

        let mut entries: Vec<(
            Option<usize>,
            String,
            Option<(u32, u32)>
        )> = Vec::new();

        let active_title = if ix.lecture_title.trim().is_empty() {
            format!("Lecture {}", ix.lecture_id)
        } else {
            format!("Lecture {} — {}", ix.lecture_id, ix.lecture_title)
        };

        entries.push((
            None,
            active_title,
            ix.page_range,
        ));

        for (i, lecture) in ix.other_lectures.iter().enumerate() {
            let title = if lecture.title.trim().is_empty() {
                format!("Lecture {}", lecture.id)
            } else {
                format!("Lecture {} — {}", lecture.id, lecture.title)
            };

            entries.push((
                Some(i),
                title,
                lecture.page_range,
            ));
        }

        let current_text = entries
            .iter()
            .find(|entry| entry.0.is_none())
            .map(|entry| {
                match entry.2 {
                    Some((a, b)) => {
                        format!("{}  (p.{}–{})", entry.1, a, b)
                    }
                    None => entry.1.clone(),
                }
            })
            .unwrap_or_default();

        let mut switch_to: Option<usize> = None;
        let mut new_lec = false;

        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("lecture_picker")
                .selected_text(current_text)
                .show_ui(ui, |ui| {
                    for (slot, title, range) in &entries {
                        let display_str = match range {
                            Some((a, b)) => {
                                format!("{}  (p.{}–{})", title, a, b)
                            }
                            None => title.clone(),
                        };

                        let selected = slot.is_none();

                        if ui.selectable_label(selected, display_str).clicked() {
                            if let Some(i) = slot {
                                switch_to = Some(*i);
                            }
                        }
                    }
                });

            if ui.small_button("󰐕")
                .on_hover_text("Add another lecture")
                .clicked()
            {
                new_lec = true;
            }
        });

        if switch_to.is_some() || new_lec {
            self.audio_engine.stop_and_release();

            match switch_to {
                Some(i) => {
                    self.audio_index.switch_to_other(i);
                }
                None => {
                    self.audio_index.start_new_lecture();
                }
            }

            self.transcript_last_seg = None;
            self.show_audio_trimmer = false;

            self.audio_index.save(&self.pdf_path);
        }
    }

    fn render_align_panel(&mut self, ui: &mut egui::Ui) {
        let total = self.total_pages.max(1);
        let (mut run, mut close, mut use_pins) = (false, false, false);
        egui::Frame::NONE
            .fill(rgb(self.theme.bg_canvas))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::symmetric(8, 8))
            .show(ui, |ui| {
                ui.label(egui::RichText::new("󰌷 Align transcript to slides").strong().size(11.5));
                ui.horizontal_wrapped(|ui| {
                    ui.label("From");
                    ui.add(egui::DragValue::new(&mut self.align_from).range(1..=total));
                    ui.label("to");
                    ui.add(egui::DragValue::new(&mut self.align_to).range(1..=total));
                    if ui.small_button("Current → from").clicked() { self.align_from = self.current_page; }
                    if ui.small_button("Current → to").clicked() { self.align_to = self.current_page; }
                });
                ui.horizontal(|ui| {
                    if ui.button("Align").clicked() { run = true; }
                    if !self.audio_index.timeline.is_empty() && ui.button("Use pins").on_hover_text("Assign segment pages from existing pins").clicked() { use_pins = true; }
                    if ui.button("Close").clicked() { close = true; }
                });
            });
        if self.align_from > self.align_to { self.align_to = self.align_from; }
        if run { self.show_align_panel = false; self.start_alignment(ui.ctx()); }
        if use_pins {
            self.show_align_panel = false;
            self.audio_index.assign_pages_from_timeline();
            self.audio_index.timeline_is_live = true;
            self.transcript_all = false;
            self.audio_index.save(&self.pdf_path);
            self.audio_toast("󰄬 Assigned segment pages from pins");
        }
        if close { self.show_align_panel = false; }
    }

    fn start_lecture_transcription(&mut self, wav_path: PathBuf, ctx: &egui::Context) {
        if self.is_transcribing {
            return;
        }
        let Some(model_dir) = self.ensure_asr_model(ctx, AsrRetry::Lecture { wav_path: wav_path.clone() }) else {
            return;
        };

        self.is_transcribing = true;
        self.asr_cancel
            .store(false, std::sync::atomic::Ordering::Relaxed);
        self.asr_progress = Some((0, self.audio_index.lecture_duration_ms.max(1)));

        let tx = self.asr_tx.clone();
        let cancel = self.asr_cancel.clone();
        let w_ctx = ctx.clone();

        thread::spawn(move || {
            audio::transcribe_lecture(&model_dir, &wav_path, &cancel, &mut |msg| {
                let _ = tx.send(msg);
                w_ctx.request_repaint();
            });
        });
    }

    fn start_alignment(&mut self, ctx: &egui::Context) {
        if self.is_transcribing || self.audio_index.segments.is_empty() { return; }
        let (lo, hi) = (self.align_from.max(1), self.align_to.min(self.total_pages));
        if lo > hi { self.audio_toast("󰅚 Invalid page range"); return; }
        self.is_transcribing = true;      // riusa il flag "busy" del dock
        self.asr_progress = None;
        let mut idx = self.audio_index.clone();
        let (pdf, total, notes) = (self.pdf_path.clone(), self.total_pages, self.notes.clone());
        let (tx, ctx) = (self.asr_tx.clone(), ctx.clone());
        thread::spawn(move || {
            let texts = extract_slide_texts(&pdf, total, &notes);
            idx.align_to_slides(&texts, (lo, hi));
            let _ = tx.send(audio::AsrJobResult::Aligned { segments: idx.segments, timeline: idx.timeline, range: (lo, hi) });
            ctx.request_repaint();
        });
    }

    fn start_memo_transcription(
        &mut self,
        page: u32,
        memo_idx: usize,
        wav_path: PathBuf,
        ctx: &egui::Context,
    ) {
        if self.is_transcribing {
            return;
        }
        let Some(model_dir) = self.ensure_asr_model(ctx, AsrRetry::Memo { page, memo_idx, wav_path: wav_path.clone() }) else {
            return;
        };

        self.is_transcribing = true;
        self.asr_progress = None;
        self.audio_toast("󱑚 Transcribing voice memo...");

        let tx = self.asr_tx.clone();
        let w_ctx = ctx.clone();
        thread::spawn(move || {
            let msg = audio::transcribe_memo(&model_dir, &wav_path, page, memo_idx);
            let _ = tx.send(msg);
            w_ctx.request_repaint();
        });
    }

    fn page_has_study_content(&self, page: u32) -> bool {
        let has_note = self
            .notes
            .get(&page)
            .map(|n| !n.trim().is_empty())
            .unwrap_or(false)
            || (page == self.note_page && !self.current_note.trim().is_empty());

        let has_anns = self
            .annotations
            .get(&page)
            .map(|a| !a.is_empty())
            .unwrap_or(false);

        let has_transcript = self.audio_index.segments.iter().any(|s| s.page == page)
            || self.audio_index.other_lectures.iter().any(|l| l.segments.iter().any(|s| s.page == page));

        has_note || has_anns || has_transcript
    }

    /// Searches backwards from `page - 1` down to `1` for the most recent non-empty note.
    fn find_previous_note(&self, page: u32) -> Option<(u32, String)> {
        let mut p = page.saturating_sub(1);
        while p >= 1 {
            if let Some(n) = self.notes.get(&p) {
                if !n.trim().is_empty() {
                    return Some((p, n.clone()));
                }
            }
            p -= 1;
        }
        None
    }

    /// Searches forwards from `page + 1` up to `total_pages` for the next non-empty note.
    fn find_next_note(&self, page: u32) -> Option<(u32, String)> {
        for p in (page + 1)..=self.total_pages {
            if let Some(n) = self.notes.get(&p) {
                if !n.trim().is_empty() {
                    return Some((p, n.clone()));
                }
            }
        }
        None
    }
    fn switch_note_page(&mut self, target_note_page: u32) {
    let p = target_note_page.clamp(1, self.total_pages.max(1));
    if p == self.note_page {
        return;
    }
    self.sync_and_save();
    self.note_page = p;
    self.current_note = self.notes.get(&p).cloned().unwrap_or_default();
    self.note_is_dirty = false;
    }
    fn apply_custom_font(&self, ctx: &egui::Context) {
        let mut fonts = egui::FontDefinitions::default();

        // Always keep JetBrainsMono Nerd Font as base/fallback for Nerd Icons!
        fonts.font_data.insert(
            "JetBrainsMono".to_owned(),
            std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
                "../assets/JetBrainsMonoNerdFont-Regular.ttf"
            ))),
        );

        let mut primary_loaded = false;
        if let Some(custom_path) = &self.app_state.settings.custom_font_path {
            if let Ok(bytes) = std::fs::read(custom_path) {
                fonts.font_data.insert(
                    "UserCustomFont".to_owned(),
                    std::sync::Arc::new(egui::FontData::from_owned(bytes)),
                );
                fonts
                    .families
                    .entry(egui::FontFamily::Proportional)
                    .or_default()
                    .insert(0, "UserCustomFont".to_owned());
                fonts
                    .families
                    .entry(egui::FontFamily::Monospace)
                    .or_default()
                    .insert(0, "UserCustomFont".to_owned());
                primary_loaded = true;
            }
        }

        // Put JetBrainsMono right after UserCustomFont (or first if no custom font) so icons always render!
        let pos = if primary_loaded { 1 } else { 0 };
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(pos, "JetBrainsMono".to_owned());
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .insert(pos, "JetBrainsMono".to_owned());

        ctx.set_fonts(fonts);
    }
    fn ensure_pdf_search_index(&mut self) {
        if !self.pdf_full_text_index.is_empty() || !self.has_pdf() {
            return;
        }
        if let Ok(doc) = Document::open(self.pdf_path.to_str().unwrap_or_default()) {
            for idx in 0..(self.total_pages as i32) {
                if let Ok(page) = doc.load_page(idx) {
                    if let Ok(tp) = page.to_text_page(mupdf::TextPageFlags::empty()) {
                        let mut page_str = String::new();
                        for block in tp.blocks() {
                            for line in block.lines() {
                                for ch in line.chars() {
                                    if let Some(c) = ch.char() {
                                        page_str.push(c);
                                    }
                                }
                                page_str.push('\n');
                            }
                        }
                        if !page_str.trim().is_empty() {
                            self.pdf_full_text_index.insert((idx + 1) as u32, page_str);
                        }
                    }
                }
            }
        }
    }
    
fn insert_blank_page_after(&mut self, after_page: u32, style: notes::BlankPageStyle, ctx: &egui::Context) {
        if !self.has_pdf() {
            return;
        }
        self.finish_sketch_edit();
        self.sync_and_save();
        // Release worker file handles before rewriting the PDF.
        self.render_tx = channel::<RenderRequest>().0;
        self.thumb_tx = channel::<RenderRequest>().0;

        match notes::insert_blank_page_with_style_into_pdf(&self.pdf_path, after_page, style) {
            Ok(new_page_num) => {
                notes::shift_doc_data_on_insert(
                    &mut self.notes,
                    &mut self.annotations,
                    &mut self.blank_pages,
                    &mut self.sketches,
                    new_page_num,
                );
                self.sketch.reset();
                if !self.blank_pages.contains(&new_page_num) {
                    self.blank_pages.insert(new_page_num);
                }

                self.hidden_pages = self
                    .hidden_pages
                    .iter()
                    .map(|&p| if p >= new_page_num { p + 1 } else { p })
                    .collect();
                for bm in &mut self.user_bookmarks {
                    if bm.page >= new_page_num {
                        bm.page += 1;
                    }
                }
                self.audio_index.for_each_page_mut(|p| if *p >= new_page_num { *p += 1 });
                self.audio_index.page_memos = std::mem::take(&mut self.audio_index.page_memos)
                    .into_iter()
                    .map(|(p, memos)| (if p >= new_page_num { p + 1 } else { p }, memos))
                    .collect();
                self.audio_index.save(&self.pdf_path);

                // Keep the current note aligned with the updated PDF.
                self.note_is_dirty = false;
                self.current_note = self.notes.get(&self.note_page).cloned().unwrap_or_default();

                // Save page data before restarting workers.
                let updated_notes = self.notes.clone();
                let updated_annotations = self.annotations.clone();
                let updated_blank_pages = self.blank_pages.clone();

                let _ = notes::save_doc_data(
                    &self.pdf_path,
                    &updated_notes,
                    &updated_annotations,
                    updated_blank_pages.clone(),
                    &self.sketches,
                );
                self.save_session_state();

                self.notes = updated_notes;
                self.annotations = updated_annotations;
                self.blank_pages = updated_blank_pages;
                let _ = notes::save_doc_data(
                    &self.pdf_path,
                    &self.notes,
                    &self.annotations,
                    self.blank_pages.clone(),
                    &self.sketches,
                );
                if self.current_page >= new_page_num { self.current_page += 1; }
                if self.target_page >= new_page_num { self.target_page += 1; }
                if self.note_page >= new_page_num { self.note_page += 1; }
                if self.split_secondary_page >= new_page_num { self.split_secondary_page += 1; }
                if self.continuous_anchor_page >= new_page_num { self.continuous_anchor_page += 1; }
                self.reanchor_page = self.continuous_anchor_page;
                self.current_note = self.notes.get(&self.note_page).cloned().unwrap_or_default();
                self.note_is_dirty = false;
                self.refresh_pdf_after_page_structure_change(ctx);
                self.current_page = new_page_num;
                self.target_page = new_page_num;
                self.note_page = new_page_num;
                self.current_note = self.notes.get(&new_page_num).cloned().unwrap_or_default();
                self.continuous_scroll_to_page = Some(new_page_num);
                self.save_session_state();
                self.toast_message = Some((
                    format!("󰄬 Blank page added"),
                    std::time::Instant::now(),
                ));
            }
            Err(e) => {
                self.refresh_pdf_after_page_structure_change(ctx);
                self.toast_message = Some((
                    format!("󰅚 Insert page error: {}", e),
                    std::time::Instant::now(),
                ));
            }
        }
    }

fn is_disposable_blank_page(&self, page: u32) -> bool {
    self.blank_pages.contains(&page)
        && self.annotations.get(&page).map_or(true, |a| a.is_empty())
        && self.notes.get(&page).map_or(true, |n| n.trim().is_empty())
}

fn request_delete_page(&mut self, page: u32, ctx: &egui::Context) {
    if !self.has_pdf() {
        return;
    }
    if self.total_pages <= 1
        || self.is_disposable_blank_page(page)
        || self.pending_delete_page == Some(page)
    {
        self.pending_delete_page = None;
        self.delete_pdf_page_and_shift_data(page, ctx);
    } else {
        self.pending_delete_page = Some(page);
        self.toast_message = Some((
            format!("Delete page {page}? Run the command again to confirm (Esc to cancel)"),
            std::time::Instant::now(),
        ));
    }
}

fn delete_pdf_page_and_shift_data(&mut self, page_to_del: u32, ctx: &egui::Context) {
    if !self.has_pdf() || self.total_pages <= 1 || page_to_del == 0 || page_to_del > self.total_pages {
        self.toast_message = Some(("󰅚 Cannot delete this page.".to_string(), std::time::Instant::now()));
        return;
    }

    self.finish_sketch_edit();
    self.sync_and_save();
    // Close worker channels that may still hold the PDF open.
    self.render_tx = channel::<RenderRequest>().0;
    self.thumb_tx = channel::<RenderRequest>().0;
    let old_anchor_page = self.continuous_anchor_page;
    let old_anchor_frac = self.continuous_anchor_frac;
    let old_current = self.current_page;
    let old_note_page = self.note_page;
    let old_split_secondary = self.split_secondary_page;

    match notes::delete_page_from_pdf(&self.pdf_path, page_to_del) {
        Ok(()) => {
            let shift = |p: u32| -> Option<u32> {
                if p < page_to_del { Some(p) }
                else if p > page_to_del { Some(p - 1) }
                else { None }
            };
            self.notes = std::mem::take(&mut self.notes).into_iter()
                .filter_map(|(p, v)| shift(p).map(|np| (np, v))).collect();
            self.annotations = std::mem::take(&mut self.annotations).into_iter()
                .filter_map(|(p, v)| shift(p).map(|np| (np, v))).collect();
            self.blank_pages = self.blank_pages.iter().copied().filter_map(shift).collect();
            self.sketches = std::mem::take(&mut self.sketches).into_iter()
                .filter_map(|(p, v)| shift(p).map(|np| (np, v))).collect();
            self.sketch.reset();
            self.hidden_pages = self.hidden_pages.iter().copied().filter_map(shift).collect();
            self.user_bookmarks.retain_mut(|bm| match shift(bm.page) {
                Some(np) => { bm.page = np; true }, None => false,
            });
            self.audio_index.for_each_page_mut(|p| if *p > page_to_del { *p -= 1 });
            self.audio_index.page_memos = std::mem::take(&mut self.audio_index.page_memos)
                .into_iter().filter_map(|(p, v)| {
                    if p == page_to_del { None } else { Some((if p > page_to_del { p - 1 } else { p }, v)) }
                }).collect();
            self.audio_index.save(&self.pdf_path);

            let new_total = self.total_pages.saturating_sub(1).max(1);
            let keep_page = |p: u32| -> u32 {
                if p > page_to_del { p - 1 }
                else if p == page_to_del { page_to_del.saturating_sub(1).max(1).min(new_total) }
                else { p.min(new_total).max(1) }
            };
            self.current_page = keep_page(old_current);
            self.target_page = self.current_page;
            self.continuous_anchor_page = keep_page(old_anchor_page);
            self.reanchor_page = self.continuous_anchor_page;
            self.reanchor_frac = old_anchor_frac;
            self.reanchor_ref_y = 0.0;
            self.reanchor_left = 6;
            self.scroll_epoch = self.scroll_epoch.wrapping_add(1);
            self.note_page = keep_page(old_note_page);
            self.split_secondary_page = keep_page(old_split_secondary);
            self.current_note = self.notes.get(&self.note_page).cloned().unwrap_or_default();
            self.note_is_dirty = false;
            self.undo_stack.clear();
            self.redo_stack.clear();
            self.revealed_tapes.clear();
            self.selected_image = None;
            self.pending_delete_page = None;
            let _ = notes::save_doc_data(&self.pdf_path, &self.notes, &self.annotations, self.blank_pages.clone(), &self.sketches);
            self.refresh_pdf_after_page_structure_change(ctx);
            self.toast_message = Some((format!("󰆴 Deleted page {}", page_to_del), std::time::Instant::now()));
        }
        Err(e) => {
            self.refresh_pdf_after_page_structure_change(ctx);
            self.toast_message = Some((format!("󰅚 Delete page failed: {e}"), std::time::Instant::now()));
        }
    }
}

fn reorder_pdf_page(&mut self, from_page: u32, to_page: u32, ctx: &egui::Context) {
    if !self.has_pdf() || from_page == to_page || from_page == 0 || to_page == 0
        || from_page > self.total_pages || to_page > self.total_pages { return; }

    self.finish_sketch_edit();
    self.sync_and_save();
    self.render_tx = channel::<RenderRequest>().0;
    self.thumb_tx = channel::<RenderRequest>().0;
    let old_anchor_page = self.continuous_anchor_page;
    let old_anchor_frac = self.continuous_anchor_frac;
    match notes::reorder_pages_in_pdf(&self.pdf_path, from_page, to_page) {
        Ok(()) => {
            let remap = |page: u32| -> u32 {
                if page == from_page { to_page }
                else if from_page < to_page && page > from_page && page <= to_page { page - 1 }
                else if from_page > to_page && page >= to_page && page < from_page { page + 1 }
                else { page }
            };
            self.notes = std::mem::take(&mut self.notes).into_iter().map(|(p,v)| (remap(p),v)).collect();
            self.annotations = std::mem::take(&mut self.annotations).into_iter().map(|(p,v)| (remap(p),v)).collect();
            self.blank_pages = self.blank_pages.iter().copied().map(remap).collect();
            self.sketches = std::mem::take(&mut self.sketches).into_iter().map(|(p,v)| (remap(p),v)).collect();
            self.sketch.reset();
            self.hidden_pages = self.hidden_pages.iter().copied().map(remap).collect();
            for bm in &mut self.user_bookmarks { bm.page = remap(bm.page); }
            self.audio_index.for_each_page_mut(|p| *p = remap(*p));
            self.audio_index.page_memos = std::mem::take(&mut self.audio_index.page_memos)
                .into_iter().map(|(p,v)| (remap(p),v)).collect();
            self.audio_index.save(&self.pdf_path);

            self.current_page = remap(self.current_page);
            self.target_page = remap(self.target_page);
            self.continuous_anchor_page = remap(old_anchor_page);
            self.reanchor_page = self.continuous_anchor_page;
            self.reanchor_frac = old_anchor_frac;
            self.reanchor_ref_y = 0.0;
            self.reanchor_left = 6;
            self.scroll_epoch = self.scroll_epoch.wrapping_add(1);
            self.note_page = remap(self.note_page);
            self.split_secondary_page = remap(self.split_secondary_page);
            self.nav_back_stack = self.nav_back_stack.iter().copied().map(remap).collect();
            self.nav_forward_stack = self.nav_forward_stack.iter().copied().map(remap).collect();
            self.revealed_tapes = std::mem::take(&mut self.revealed_tapes).into_iter()
                .map(|(p,i)| (remap(p),i)).collect();
            if let Some((p, i)) = self.selected_image { self.selected_image = Some((remap(p), i)); }
            if let Some((p, a, b)) = self.active_snip_rect { self.active_snip_rect = Some((remap(p), a, b)); }
            self.current_note = self.notes.get(&self.note_page).cloned().unwrap_or_default();
            self.note_is_dirty = false;
            let _ = notes::save_doc_data(&self.pdf_path, &self.notes, &self.annotations, self.blank_pages.clone(), &self.sketches);
            self.save_session_state();
            self.refresh_pdf_after_page_structure_change(ctx);
            self.toast_message = Some((format!("󰄬 Moved page {from_page} to position {to_page}"), std::time::Instant::now()));
        }
        Err(e) => {
            self.refresh_pdf_after_page_structure_change(ctx);
            self.toast_message = Some((format!("󰅚 Page reorder failed: {e}"), std::time::Instant::now()));
        }
    }
}

/// Restart workers for the updated file without reloading the UI session.
/// Preserve the current page, zoom, view, panels, and scroll position.
fn refresh_pdf_after_page_structure_change(&mut self, ctx: &egui::Context) {
    let path = self.pdf_path.clone();
    let (total_pages, outline, current_page_pt) = match Document::open(path.to_str().unwrap_or_default()) {
        Ok(doc) => {
            let total = doc.page_count().unwrap_or(1).max(1) as u32;
            let mut items = Vec::new();
            if let Ok(nodes) = doc.outlines() {
                for node in nodes {
                    let page_idx = node.dest.as_ref().map(|d| d.loc.page_number as i32).unwrap_or(0);
                    items.push(TocItem { title: node.title.clone(), page: (page_idx.max(0) as u32) + 1 });
                }
            }
            let page_idx = self.current_page.saturating_sub(1).min(total - 1) as i32;
            let pt = doc.load_page(page_idx).and_then(|p| p.bounds())
                .map(|b| (b.x1 - b.x0, b.y1 - b.y0)).unwrap_or((0.0, 0.0));
            (total, items, pt)
        }
        Err(_) => (self.total_pages, self.outline.clone(), self.page_pt),
    };

    // Replace workers without calling open_pdf or changing UI state.
    self.render_tx = channel::<RenderRequest>().0;
    self.thumb_tx = channel::<RenderRequest>().0;
    self.total_pages = total_pages;
    self.current_page = self.current_page.clamp(1, total_pages);
    self.target_page = self.current_page;
    self.note_page = self.note_page.clamp(1, total_pages);
    self.outline = outline;
    self.page_pt = current_page_pt;
    self.page_cache.clear();
    self.thumb_cache.clear();
    self.page_scale.clear();
    self.cache_order.clear();
    self.page_text.clear();
    self.page_links.clear();
    self.pdf_full_text_index.clear();
    self.last_page_req = None;
    self.last_thumb_req = None;
    self.visible_pages = (0, 0);
    self.texture_handle = None;
    self.is_rendering = true;
    self.current_note = self.notes.get(&self.note_page).cloned().unwrap_or_default();
    let (page_tx, thumb_tx, res_rx) = spawn_render_workers(path, ctx.clone());
    self.render_tx = page_tx;
    self.thumb_tx = thumb_tx;
    self.render_rx = res_rx;
    self.save_session_state();
    ctx.request_repaint();
}

fn push_undo_snapshot(&mut self, page: u32) {
        // A slide annotation is now the last thing drawn on: Ctrl+Z goes back to the slides.
        self.sketch.focus = false;
        let current_state = self.annotations.get(&page).cloned().unwrap_or_default();
        let stack = self.undo_stack.entry(page).or_default();
        // Cap history to 40 steps per page to keep RAM negligible
        if stack.len() >= 40 {
            stack.remove(0);
        }
        stack.push(current_state);
        // Any new drawing action invalidates the redo branch for that page
        self.redo_stack.remove(&page);
    }

    fn undo_annotation(&mut self, page: u32) {
        if self.sketch_receives_undo() {
            if self.sketch.undo(self.note_page, &mut self.sketches) {
                self.save_sketch_data();
                self.toast_message = Some(("󰕌 Undo sketch".to_string(), std::time::Instant::now()));
            }
            return;
        }
        if let Some(stack) = self.undo_stack.get_mut(&page) {
            if let Some(prev_state) = stack.pop() {
                let current_state = self.annotations.get(&page).cloned().unwrap_or_default();
                self.redo_stack.entry(page).or_default().push(current_state);
                if prev_state.is_empty() {
                    self.annotations.remove(&page);
                } else {
                    self.annotations.insert(page, prev_state);
                }
                self.selected_textbox = None;
                let _ = notes::save_doc_data(&self.pdf_path, &self.notes, &self.annotations, self.blank_pages.clone(), &self.sketches);
                self.toast_message = Some((
                    "󰕌 Undo annotation".to_string(),
                    std::time::Instant::now(),
                ));
            }
        }
    }

    fn redo_annotation(&mut self, page: u32) {
        if self.sketch_receives_undo() {
            if self.sketch.redo(self.note_page, &mut self.sketches) {
                self.save_sketch_data();
                self.toast_message = Some(("󰑎 Redo sketch".to_string(), std::time::Instant::now()));
            }
            return;
        }
        if let Some(stack) = self.redo_stack.get_mut(&page) {
            if let Some(next_state) = stack.pop() {
                let current_state = self.annotations.get(&page).cloned().unwrap_or_default();
                self.undo_stack.entry(page).or_default().push(current_state);
                self.annotations.insert(page, next_state);
                self.selected_textbox = None;
                let _ = notes::save_doc_data(&self.pdf_path, &self.notes, &self.annotations, self.blank_pages.clone(), &self.sketches);
                self.toast_message = Some((
                    "󰑎 Redo annotation".to_string(),
                    std::time::Instant::now(),
                ));
            }
        }
    }
    fn jump_with_history(&mut self, target_page: u32, ctx: &egui::Context) {
        if target_page != self.current_page && (1..=self.total_pages).contains(&target_page) {
            self.nav_back_stack.push(self.current_page);
            if self.nav_back_stack.len() > 50 {
                self.nav_back_stack.remove(0);
            }
            self.nav_forward_stack.clear();
            self.go_to_page(target_page, ctx);
        }
    }

    fn nav_back(&mut self, ctx: &egui::Context) {
        if let Some(prev) = self.nav_back_stack.pop() {
            self.nav_forward_stack.push(self.current_page);
            self.go_to_page(prev, ctx);
        }
    }

    fn nav_forward(&mut self, ctx: &egui::Context) {
        if let Some(next) = self.nav_forward_stack.pop() {
            self.nav_back_stack.push(self.current_page);
            self.go_to_page(next, ctx);
        }
    }
    
    fn render_sub_topbar_tools(&mut self, ui: &mut egui::Ui) {
    egui::Panel::top("sub_topbar_tools")
        .exact_size(36.0)
        .frame(
            egui::Frame::NONE
                .fill(rgb(self.theme.bg_elevated))
                .inner_margin(egui::Margin::symmetric(12, 4))
                .stroke(egui::Stroke::new(1.0, rgb(self.theme.border))),
        )
        .show(ui, |ui| {
            // Center the full control group in the toolbar.
            ui.with_layout(
                egui::Layout::left_to_right(egui::Align::Center)
                    .with_main_align(egui::Align::Center),
                |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;

                    let btn_sz = egui::vec2(28.0, 26.0);

                    let mut bar_btn =
                        |ui: &mut egui::Ui,
                         icon: &str,
                         tool: AnnotationTool,
                         tip: &str|
                         -> bool {
                            let clicked = ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new(icon).size(14.5),
                                    )
                                    .selected(self.annotation_tool == tool)
                                    .min_size(btn_sz),
                                )
                                .on_hover_text(tip)
                                .clicked();

                            if clicked {
                                self.annotation_tool = tool;
                            }

                            clicked
                        };

                    // Primary Tools
                    bar_btn(ui, "󰇀", AnnotationTool::Cursor, "Cursor / Select PDF Text");
                    bar_btn(ui, "", AnnotationTool::Highlight, "Highlighter");
                    bar_btn(ui, "󰊇", AnnotationTool::Underline, "Underline");
                    bar_btn(ui, "󱦹", AnnotationTool::Draw, "Freehand Smooth Pen");
                    bar_btn(ui, "󰠱", AnnotationTool::Shape, "Geometric Shapes");
                    bar_btn(ui, "󰊄", AnnotationTool::TextBox, "Text Box");
                    bar_btn(ui, "󰝤", AnnotationTool::Tape, "Occlusion Tape (Active Recall)");
                    bar_btn(ui, "󰆞", AnnotationTool::Snip, "Snip Slide Region to Notes");
                    bar_btn(ui, "󰐾", AnnotationTool::Laser, "Laser Pointer");
                    bar_btn(ui, "󰇾", AnnotationTool::Eraser, "Eraser");

                    ui.separator();

                    // Inline Color Swatches
                    for (col, name) in ANNOTATION_PALETTE {
                        let is_sel = self.annotation_color == col;

                        let (sw_rect, sw_resp) = ui.allocate_exact_size(
                            egui::vec2(20.0, 20.0),
                            egui::Sense::click(),
                        );

                        ui.painter()
                            .circle_filled(sw_rect.center(), 7.5, rgb(col));

                        if is_sel || sw_resp.hovered() {
                            ui.painter().circle_stroke(
                                sw_rect.center(),
                                9.0,
                                egui::Stroke::new(
                                    if is_sel { 2.0 } else { 1.2 },
                                    rgb(self.theme.fg_primary),
                                ),
                            );
                        }

                        if sw_resp.on_hover_text(name).clicked() {
                            self.annotation_color = col;
                        }
                    }

                    ui.separator();

                    // Undo / Redo
                    let can_undo = self
                        .undo_stack
                        .get(&self.current_page)
                        .map(|s| !s.is_empty())
                        .unwrap_or(false);

                    let can_redo = self
                        .redo_stack
                        .get(&self.current_page)
                        .map(|s| !s.is_empty())
                        .unwrap_or(false);

                    if ui
                        .add_enabled(
                            can_undo,
                            egui::Button::new("󰕌").min_size(btn_sz),
                        )
                        .on_hover_text("Undo Annotation [Ctrl+Z]")
                        .clicked()
                    {
                        self.undo_annotation(self.current_page);
                    }

                    if ui
                        .add_enabled(
                            can_redo,
                            egui::Button::new("󰑎").min_size(btn_sz),
                        )
                        .on_hover_text("Redo Annotation [Ctrl+Y]")
                        .clicked()
                    {
                        self.redo_annotation(self.current_page);
                    }

                    // Contextual inline options
                    if self.annotation_tool == AnnotationTool::Shape {
                        ui.separator();

                        for (sk, lbl) in [
                            (ShapeKind::Rect, "󰝣 Rect"),
                            (ShapeKind::Circle, "󰝥 Circle"),
                            (ShapeKind::Line, "󰕞 Line"),
                            (ShapeKind::Arrow, "󰁔 Arrow"),
                        ] {
                            if ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new(lbl).size(12.0),
                                    )
                                    .selected(self.shape_kind == sk)
                                    .min_size(egui::vec2(52.0, 24.0)),
                                )
                                .clicked()
                            {
                                self.shape_kind = sk;
                            }
                        }
                    }
                    // Draw Tool: Stroke Thickness
                    if matches!(
                        self.annotation_tool,
                        AnnotationTool::Draw
                            | AnnotationTool::Underline
                            | AnnotationTool::Shape
                            | AnnotationTool::Tape
                            | AnnotationTool::Laser
                    ) {
                        ui.separator();

                        ui.label("Thickness:");

                        ui.add(
                            egui::Slider::new(&mut self.stroke_width, 1.0..=18.0)
                                .suffix(" px")
                                .show_value(true),
                        );

                        if ui
                            .small_button("↺")
                            .on_hover_text("Reset thickness to default (2.5 px)")
                            .clicked()
                        {
                            self.stroke_width = notes::DEFAULT_ANNOTATION_STROKE_WIDTH;
                        }
                    }
                },
            );
        });
}

    fn render_radial_tools_menu(&mut self, ui: &mut egui::Ui) {
        let Some(raw_center) = self.radial_menu_pos else {
            return;
        };
        if !self.app_state.settings.radial_menu_enabled {
            self.radial_menu_pos = None;
            return;
        }
        // Escape closes the menu unless a text field is being edited.
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) && !ui.ctx().egui_wants_keyboard_input() {
            self.radial_menu_pos = None;
            return;
        }

        // Clamp center so the 105px radius circle never goes off-screen
        let screen = ui.ctx().content_rect();
        let margin = 110.0;
        let center = egui::pos2(
            raw_center.x.clamp(screen.left() + margin, screen.right() - margin),
            raw_center.y.clamp(screen.top() + margin, screen.bottom() - margin),
        );

        let mut close_menu = false;

        // 1. Radial circle
        let pick_id = ui.make_persistent_id("radial_shape_pick");
        let shape_pick: bool = ui.data(|d| d.get_temp::<egui::Pos2>(pick_id)) == Some(raw_center);

        egui::Area::new(ui.make_persistent_id("radial_pie_tools_menu"))
            .fixed_pos(center - egui::vec2(105.0, 105.0))
            .order(egui::Order::Tooltip)
            .show(ui.ctx(), |ui| {
                // 210 × 210 circle plus 30 px for the hover label.
                let (box_rect, bg_resp) =
                    ui.allocate_exact_size(egui::vec2(210.0, 240.0), egui::Sense::click());
                let c = box_rect.min + egui::vec2(105.0, 105.0);
                let painter = ui.painter();

                // Soft circular backdrop halo + ring guides
                painter.circle_filled(
                    c,
                    102.0,
                    egui::Color32::from_rgba_unmultiplied(
                        self.theme.bg_panel[0],
                        self.theme.bg_panel[1],
                        self.theme.bg_panel[2],
                        235,
                    ),
                );
                painter.circle_stroke(c, 102.0, egui::Stroke::new(1.5, rgb(self.theme.accent)));
                painter.circle_stroke(c, 52.0, egui::Stroke::new(1.0, rgb(self.theme.border)));

                let hover_pos = ui.input(|i| i.pointer.hover_pos());
                let clicked = bg_resp.clicked();
                let mut label_hover: Option<String> = None;

                // Central hub: Undo
                let hub_r = 18.0;
                let can_undo = self
                    .undo_stack
                    .get(&self.current_page)
                    .map(|s| !s.is_empty())
                    .unwrap_or(false);

                let in_hub = hover_pos.map(|p| p.distance(c) <= hub_r).unwrap_or(false);

                painter.circle_filled(c, hub_r, rgb(self.theme.bg_elevated));
                painter.circle_stroke(c, hub_r, egui::Stroke::new(1.0, rgb(self.theme.border)));
                let undo_pos = c;
                if in_hub && can_undo {
                    painter.circle_filled(undo_pos, 18.0, rgb(self.theme.bg_active));
                }

                let fg = rgb(self.theme.fg_primary);
                let fg_dim = fg.gamma_multiply(0.35);
                painter.text(
                    undo_pos,
                    egui::Align2::CENTER_CENTER,
                    "󰕌",
                    egui::FontId::proportional(16.0),
                    if can_undo { fg } else { fg_dim },
                );

                if in_hub {
                    label_hover = Some("Undo".to_string());
                    if clicked && can_undo {
                        self.undo_annotation(self.current_page);
                    }
                }

                // Intenal colors ring
                                if shape_pick {
                    // Choose a shape instead of a color.
                    let kinds = [
                        (ShapeKind::Rect, "󰝣", "Rectangle"),
                        (ShapeKind::Circle, "󰝥", "Circle"),
                        (ShapeKind::Line, "󰕞", "Line"),
                        (ShapeKind::Arrow, "󰁔", "Arrow"),
                    ];
                    for (i, (sk, icon, lbl)) in kinds.iter().enumerate() {
                        let angle = (i as f32) * std::f32::consts::TAU / 4.0 - std::f32::consts::FRAC_PI_2;
                        let pt = c + egui::vec2(angle.cos(), angle.sin()) * 36.0;
                        let is_sel = self.shape_kind == *sk;
                        let is_hov = hover_pos.map(|p| p.distance(pt) <= 13.0).unwrap_or(false);
                
                        painter.circle_filled(
                            pt,
                            13.0,
                            if is_sel {
                                rgb(self.theme.accent)
                            } else if is_hov {
                                rgb(self.theme.bg_active)
                            } else {
                                rgb(self.theme.bg_elevated)
                            },
                        );
                        painter.circle_stroke(pt, 13.0, egui::Stroke::new(1.2, rgb(self.theme.border)));
                        painter.text(
                            pt,
                            egui::Align2::CENTER_CENTER,
                            icon,
                            egui::FontId::proportional(14.0),
                            if is_sel {
                                egui::Color32::from_rgb(18, 20, 26)
                            } else {
                                rgb(self.theme.fg_primary)
                            },
                        );
                        if is_hov {
                            label_hover = Some(lbl.to_string());
                        }
                        if clicked && is_hov {
                            self.shape_kind = *sk;
                            self.annotation_tool = AnnotationTool::Shape;
                            close_menu = true;
                        }
                    }
                } else 
                {let n_cols = ANNOTATION_PALETTE.len();
                let col_ring_r = 36.0;
                for (i, (col, name)) in ANNOTATION_PALETTE.iter().copied().enumerate() {
                    let angle = (i as f32) * std::f32::consts::TAU / (n_cols as f32)
                        - std::f32::consts::FRAC_PI_2;
                    let pt = c + egui::vec2(angle.cos(), angle.sin()) * col_ring_r;
                    let is_sel = self.annotation_color == col;
                    let is_hov = hover_pos.map(|p| p.distance(pt) <= 10.0).unwrap_or(false);

                    painter.circle_filled(pt, if is_hov { 9.5 } else { 8.0 }, rgb(col));
                    if is_sel || is_hov {
                        painter.circle_stroke(
                            pt,
                            10.0,
                            egui::Stroke::new(1.8, egui::Color32::WHITE),
                        );
                    }
                    if is_hov {
                        label_hover = Some(name.to_string());
                    }
                    if clicked && is_hov {
                        self.annotation_color = col;
                        close_menu = true;
                    }
                }
            }
                // Outer ring: tools (77 px radius).
                let tools: &[(AnnotationTool, &str, &str)] = &[
                    (AnnotationTool::Cursor, "󰇀", "Cursor"),
                    (AnnotationTool::Highlight, "", "Highlight"),
                    (AnnotationTool::Underline, "󰊇", "Underline"),
                    (AnnotationTool::Draw, "󱦹", "Pen"),
                    (AnnotationTool::Shape, "󰠱", "Shapes"),
                    (AnnotationTool::TextBox, "󰊄", "Text"),
                    (AnnotationTool::Tape, "󰝤", "Tape"),
                    (AnnotationTool::Snip, "󰆞", "Snip"),
                    (AnnotationTool::Laser, "󰐾", "Laser"),
                    (AnnotationTool::Eraser, "󰇾", "Eraser"),
                ];
                let outer_r = 77.0;
                let node_r = 18.0;

                for (i, &(tool, icon, label)) in tools.iter().enumerate() {
                    let angle = (i as f32) * std::f32::consts::TAU / (tools.len() as f32)
                        - std::f32::consts::FRAC_PI_2;
                    let pt = c + egui::vec2(angle.cos(), angle.sin()) * outer_r;
                    let is_active = self.annotation_tool == tool;
                    let is_hov = hover_pos.map(|p| p.distance(pt) <= node_r).unwrap_or(false);

                    // The shape icon reflects the current selection.
                    let icon = if tool == AnnotationTool::Shape {
                        match self.shape_kind {
                            ShapeKind::Rect => "󰝣",
                            ShapeKind::Circle => "󰝥",
                            ShapeKind::Line => "󰕞",
                            ShapeKind::Arrow => "󰁔",
                        }
                    } else {
                        icon
                    };

                    let fill = if is_active {
                        rgb(self.theme.accent)
                    } else if is_hov {
                        rgb(self.theme.bg_active)
                    } else {
                        rgb(self.theme.bg_elevated)
                    };
                    painter.circle_filled(pt, node_r, fill);
                    painter.circle_stroke(
                        pt,
                        node_r,
                        egui::Stroke::new(
                            if is_active || is_hov { 1.8 } else { 1.0 },
                            if is_active {
                                egui::Color32::WHITE
                            } else {
                                rgb(self.theme.border)
                            },
                        ),
                    );
                    painter.text(
                        pt,
                        egui::Align2::CENTER_CENTER,
                        icon,
                        egui::FontId::proportional(15.0),
                        if is_active {
                            egui::Color32::from_rgb(18, 20, 26)
                        } else {
                            rgb(self.theme.fg_primary)
                        },
                    );

                    if is_hov {
                        label_hover = Some(label.to_string());
                    }
                     
                    if clicked && is_hov {
                        self.annotation_tool = tool;
                        if tool == AnnotationTool::Shape {
                            // First click opens the shape picker and keeps the menu open.
                            ui.data_mut(|d| d.insert_temp(pick_id, raw_center));
                        } else {
                            close_menu = true;
                        }
                    }
                }

                // Label for the node under the pointer.
                if let Some(txt) = &label_hover {
                    let pos = c + egui::vec2(0.0, 122.0);
                    let w = txt.chars().count() as f32 * 7.0 + 20.0;
                    let r = egui::Rect::from_center_size(pos, egui::vec2(w, 20.0));
                    painter.rect_filled(
                        r,
                        egui::CornerRadius::same(8),
                        rgb(self.theme.bg_elevated),
                    );
                    painter.text(
                        pos,
                        egui::Align2::CENTER_CENTER,
                        txt,
                        egui::FontId::proportional(12.0),
                        rgb(self.theme.fg_primary),
                    );
                }
            });

        // ====================================================================
        // 3. Close when the pointer is pressed outside the circle and panel.
        // Use press rather than click so drawing drags also close the menu.
        // ====================================================================
        
        let press_pos = ui.input(|i| {
            if i.pointer.any_pressed() {
                i.pointer.interact_pos()
            } else {
                None
            }
        });
        if let Some(p) = press_pos {
            if p.distance(center) > 106.0 {
                close_menu = true;
            }
        }
        if close_menu {
            ui.data_mut(|d| d.remove::<egui::Pos2>(pick_id));
            self.radial_menu_pos = None;
        }
    }

    fn render_vertical_tools_rail(&mut self, ui: &mut egui::Ui) {
    let mut shape_btn_rect: Option<egui::Rect> = None;
    let mut color_btn_rect: Option<egui::Rect> = None;

    let shape_popup_id = ui.make_persistent_id("rail_shape_flyout_open");
    let color_popup_id = ui.make_persistent_id("rail_color_flyout_open");
    let text_popup_id = ui.make_persistent_id("rail_text_flyout_open");

    let mut shape_open =
        ui.data_mut(|d| *d.get_temp_mut_or_default::<bool>(shape_popup_id));

    let mut color_open =
        ui.data_mut(|d| *d.get_temp_mut_or_default::<bool>(color_popup_id));

    let mut text_open =
        ui.data_mut(|d| *d.get_temp_mut_or_default::<bool>(text_popup_id));

    egui::Panel::right("vertical_tools_rail")
        .resizable(false)
        .exact_size(44.0)
        .frame(
            egui::Frame::NONE
                .fill(rgb(self.theme.bg_panel))
                .inner_margin(egui::Margin::symmetric(5, 8))
                .stroke(egui::Stroke::new(1.0, rgb(self.theme.border))),
        )
        .show(ui, |ui| {
            // Center the full button column in the rail.
            ui.with_layout(
                egui::Layout::top_down(egui::Align::Center)
                    .with_main_align(egui::Align::Center),
                |ui| {
                    ui.spacing_mut().item_spacing.y = 5.0;

                    let btn_sz = egui::vec2(32.0, 30.0);

                    let mut rail_btn = |
                        ui: &mut egui::Ui,
                        icon: &str,
                        tool: AnnotationTool,
                        tip: &str,
                    | -> egui::Response {
                        let resp = ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new(icon).size(15.5),
                                )
                                .selected(self.annotation_tool == tool)
                                .min_size(btn_sz),
                            )
                            .on_hover_text(tip);

                        if resp.clicked() {
                            self.annotation_tool = tool;
                        }

                        resp
                    };

                    // Primary Tools
                    rail_btn(
                        ui,
                        "󰇀",
                        AnnotationTool::Cursor,
                        "Cursor / Select PDF Text & Click Links",
                    );

                    rail_btn(
                        ui,
                        "",
                        AnnotationTool::Highlight,
                        "Highlighter",
                    );

                    rail_btn(
                        ui,
                        "󰊇",
                        AnnotationTool::Underline,
                        "Underline",
                    );

                    rail_btn(
                        ui,
                        "󱦹",
                        AnnotationTool::Draw,
                        "Freehand Smooth Pen",
                    );

                    // Shapes button
                    let s_icon = match self.shape_kind {
                        ShapeKind::Rect => "󰝣",
                        ShapeKind::Circle => "󰝥",
                        ShapeKind::Line => "󰕞",
                        ShapeKind::Arrow => "󰁔",
                    };

                    let s_resp = rail_btn(
                        ui,
                        s_icon,
                        AnnotationTool::Shape,
                        "Geometric Shapes (click to pick Rect, Circle, Line, Arrow)",
                    );

                    shape_btn_rect = Some(s_resp.rect);

                    if s_resp.clicked() {
                        shape_open = !shape_open;
                        color_open = false;
                        text_open = false;
                    }

                    // Text Box
                    rail_btn(
                        ui,
                        "󰊄",
                        AnnotationTool::TextBox,
                        "Insert or Modify Text boxes in PDF pages",
                    );

                    // Tape
                    rail_btn(
                        ui,
                        "󰝤",
                        AnnotationTool::Tape,
                        "Occlusion Tape (mask formulas; click tape to peek)",
                    );

                    // Snip
                    rail_btn(
                        ui,
                        "󰆞",
                        AnnotationTool::Snip,
                        "Snip / Crop Slide Region into Notes",
                    );

                    // Laser
                    rail_btn(
                        ui,
                        "󰐾",
                        AnnotationTool::Laser,
                        "Laser Pointer",
                    );

                    // Eraser
                    rail_btn(
                        ui,
                        "󰇾",
                        AnnotationTool::Eraser,
                        "Eraser",
                    );

                    ui.separator();

                    // Color Swatch Button
                    let (c_rect, c_resp) =
                        ui.allocate_exact_size(btn_sz, egui::Sense::click());

                    color_btn_rect = Some(c_rect);

                    ui.painter().circle_filled(
                        c_rect.center(),
                        10.0,
                        rgb(self.annotation_color),
                    );

                    ui.painter().circle_stroke(
                        c_rect.center(),
                        10.0,
                        egui::Stroke::new(
                            if color_open || c_resp.hovered() {
                                2.0
                            } else {
                                1.2
                            },
                            rgb(self.theme.fg_primary),
                        ),
                    );

                    if c_resp.on_hover_text("Annotation Color").clicked() {
                        color_open = !color_open;
                        shape_open = false;
                    }

                    ui.separator();

                    // Undo / Redo
                    let can_undo = self
                        .undo_stack
                        .get(&self.current_page)
                        .map(|s| !s.is_empty())
                        .unwrap_or(false);

                    let can_redo = self
                        .redo_stack
                        .get(&self.current_page)
                        .map(|s| !s.is_empty())
                        .unwrap_or(false);

                    if ui
                        .add_enabled(
                            can_undo,
                            egui::Button::new("󰕌").min_size(btn_sz),
                        )
                        .on_hover_text("Undo Annotation [Ctrl+Z]")
                        .clicked()
                    {
                        self.undo_annotation(self.current_page);
                    }

                    if ui
                        .add_enabled(
                            can_redo,
                            egui::Button::new("󰑎").min_size(btn_sz),
                        )
                        .on_hover_text("Redo Annotation [Ctrl+Y]")
                        .clicked()
                    {
                        self.redo_annotation(self.current_page);
                    }
                    // Draw Tool: Vertical Stroke Thickness
                    if matches!(
                        self.annotation_tool,
                        AnnotationTool::Draw
                            | AnnotationTool::Underline
                            | AnnotationTool::Shape
                            | AnnotationTool::Tape
                            | AnnotationTool::Laser
                    ) {
                        ui.separator();

                        ui.label(format!("{:.1}px", self.stroke_width));

                        ui.add(
                            egui::Slider::new(&mut self.stroke_width, 1.0..=18.0)
                                .vertical()
                                .show_value(false),
                        )
                        .on_hover_text("Stroke Thickness");

                        if ui
                            .small_button("↺")
                            .on_hover_text("Reset thickness to default (2.5 px)")
                            .clicked()
                        {
                            self.stroke_width = notes::DEFAULT_ANNOTATION_STROKE_WIDTH;
                        }
                    }
                },
            );
        });

    // ================================================================
    // FLYOUT POPUP 1: SHAPE SELECTOR
    // ================================================================
    if shape_open && self.annotation_tool == AnnotationTool::Shape {
        if let Some(btn_r) = shape_btn_rect {
            let pos = egui::pos2(btn_r.left() - 8.0, btn_r.center().y);

            egui::Area::new(ui.make_persistent_id("rail_shape_popup_area"))
                .fixed_pos(pos)
                .pivot(egui::Align2::RIGHT_CENTER)
                .order(egui::Order::Foreground)
                .show(ui.ctx(), |ui| {
                    egui::Frame::menu(ui.style())
                        .fill(rgb(self.theme.bg_elevated))
                        .stroke(egui::Stroke::new(1.0, rgb(self.theme.accent)))
                        .inner_margin(egui::Margin::symmetric(8, 6))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                for (sk, icon, lbl) in [
                                    (ShapeKind::Rect, "󰝣", "Rectangle"),
                                    (ShapeKind::Circle, "󰝥", "Circle"),
                                    (ShapeKind::Line, "󰕞", "Line"),
                                    (ShapeKind::Arrow, "󰁔", "Arrow"),
                                ] {
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                egui::RichText::new(icon).size(15.0),
                                            )
                                            .selected(self.shape_kind == sk)
                                            .min_size(egui::vec2(30.0, 28.0)),
                                        )
                                        .on_hover_text(lbl)
                                        .clicked()
                                    {
                                        self.shape_kind = sk;
                                        shape_open = false;
                                    }
                                }
                            });
                        });
                });
        }
    }

    // ================================================================
    // FLYOUT POPUP 2: COLOR PALETTE
    // ================================================================
    if color_open {
        if let Some(btn_r) = color_btn_rect {
            let pos = egui::pos2(btn_r.left() - 8.0, btn_r.center().y);

            egui::Area::new(ui.make_persistent_id("rail_color_popup_area"))
                .fixed_pos(pos)
                .pivot(egui::Align2::RIGHT_CENTER)
                .order(egui::Order::Foreground)
                .show(ui.ctx(), |ui| {
                    egui::Frame::menu(ui.style())
                        .fill(rgb(self.theme.bg_elevated))
                        .stroke(egui::Stroke::new(1.0, rgb(self.theme.accent)))
                        .inner_margin(egui::Margin::symmetric(8, 6))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                for (col, name) in ANNOTATION_PALETTE {
                                    let is_sel = self.annotation_color == col;

                                    let (sw_rect, sw_resp) = ui.allocate_exact_size(
                                        egui::vec2(22.0, 22.0),
                                        egui::Sense::click(),
                                    );

                                    ui.painter()
                                        .circle_filled(sw_rect.center(), 8.5, rgb(col));

                                    if is_sel || sw_resp.hovered() {
                                        ui.painter().circle_stroke(
                                            sw_rect.center(),
                                            10.0,
                                            egui::Stroke::new(
                                                1.8,
                                                rgb(self.theme.fg_primary),
                                            ),
                                        );
                                    }

                                    if sw_resp.on_hover_text(name).clicked() {
                                        self.annotation_color = col;

                                        if let Some(idx) = self.selected_textbox {
                                            if let Some(ann) = self
                                                .annotations
                                                .get_mut(&self.current_page)
                                                .and_then(|v| v.get_mut(idx))
                                            {
                                                ann.color = col;
                                            }

                                            let _ = notes::save_doc_data(
                                                &self.pdf_path,
                                                &self.notes,
                                                &self.annotations,
                                                self.blank_pages.clone(),
                                                &self.sketches,
                                            );
                                        }

                                        color_open = false;
                                    }
                                }
                            });
                        });
                });
        }
    }

    // ================================================================
    // ESCAPE: CLOSE POPUPS
    // ================================================================
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        shape_open = false;
        color_open = false;
    }

    ui.data_mut(|d| {
        *d.get_temp_mut_or_default::<bool>(shape_popup_id) = shape_open;
        *d.get_temp_mut_or_default::<bool>(color_popup_id) = color_open;
    });
}

    fn open_flashcards_trainer(&mut self) {
        self.sync_and_save();
        self.flashcards_deck = notes::extract_flashcards_from_notes(&self.notes);
        self.flashcard_idx = 0;
        self.flashcard_revealed = false;
        self.flashcard_stats = (0, 0, 0);
        self.show_flashcards_modal = true;
    }
    fn render_flashcards_modal(&mut self, ui: &mut egui::Ui) {
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.show_flashcards_modal = false;
            return;
        }

        // Dimmed backdrop
        let screen_rect = ui.ctx().content_rect();
        egui::Area::new(ui.make_persistent_id("flashcards_backdrop"))
            .fixed_pos(screen_rect.min)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                let (rect, resp) =
                    ui.allocate_exact_size(screen_rect.size(), egui::Sense::click());
                ui.painter().rect_filled(
                    rect,
                    egui::CornerRadius::ZERO,
                    egui::Color32::from_black_alpha(155),
                );
                if resp.clicked() {
                    self.show_flashcards_modal = false;
                }
            });

        let mut jump_to_slide: Option<u32> = None;

        egui::Area::new(ui.make_persistent_id("flashcards_trainer_card"))
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .order(egui::Order::Tooltip)
            .show(ui.ctx(), |ui| {
                egui::Frame::window(ui.style())
                    .fill(rgb(self.theme.bg_elevated))
                    .stroke(egui::Stroke::new(1.5, rgb(self.theme.accent)))
                    .corner_radius(egui::CornerRadius::same(10))
                    .inner_margin(egui::Margin::same(18))
                    .show(ui, |ui| {
                        ui.set_width(540.0);
                        ui.set_min_height(330.0);

                        // Top Header Bar
                        ui.horizontal(|ui| {
                            ui.heading("󰘸  Active Recall & Flashcards");
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .add_sized([28.0, 26.0], egui::Button::new("󰅖"))
                                        .on_hover_text("Close [Esc]")
                                        .clicked()
                                    {
                                        self.show_flashcards_modal = false;
                                    }

                                    if !self.flashcards_deck.is_empty()
                                        && ui
                                            .button("󰈇 Export Anki (.tsv)")
                                            .on_hover_text("Export deck for Anki Desktop / AnkiDroid")
                                            .clicked()
                                    {
                                        self.export_anki_dialog(ui.ctx());
                                    }

                                    if ui
                                        .small_button("󰑐 Refresh")
                                        .on_hover_text("Re-scan notes for new cards")
                                        .clicked()
                                    {
                                        self.open_flashcards_trainer();
                                    }
                                },
                            );
                        });
                        ui.separator();

                        if self.flashcards_deck.is_empty() {
                            ui.add_space(24.0);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    egui::RichText::new("No Flashcards Found in Notes Yet")
                                        .strong()
                                        .size(16.0),
                                );
                                ui.add_space(10.0);
                                ui.label(
                                    egui::RichText::new(
                                        "Write flashcards directly inside any slide's Markdown notes using either syntax:",
                                    )
                                    .color(rgb(self.theme.fg_muted)),
                                );
                                ui.add_space(12.0);
                                egui::Frame::NONE
                                    .fill(rgb(self.theme.bg_panel))
                                    .corner_radius(egui::CornerRadius::same(6))
                                    .inner_margin(egui::Margin::same(12))
                                    .show(ui, |ui| {
                                        ui.label(
                                            egui::RichText::new(
                                                "1) Inline syntax:\n   Page Fault :: Exception raised by MMU when PTE valid bit is 0\n\n2) Multi-line Q/A syntax:\n   Q: What is the time complexity of Heapify?\n   A: $O(\\log n)$ worst-case per node, $O(n)$ bottom-up build.",
                                            )
                                            .monospace()
                                            .size(12.5),
                                        );
                                    });
                            });
                            return;
                        }

                        // Check if user finished reviewing all cards in the deck
                        if self.flashcard_idx >= self.flashcards_deck.len() {
                            ui.add_space(30.0);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    egui::RichText::new("󰄬  Deck Review Complete!")
                                        .strong()
                                        .size(18.0)
                                        .color(rgb(self.theme.success)),
                                );
                                ui.add_space(12.0);
                                let (hard, good, easy) = self.flashcard_stats;
                                ui.label(format!(
                                    "Session stats:   Again/Hard: {}   |   Good: {}   |   Easy: {}",
                                    hard, good, easy
                                ));
                                ui.add_space(20.0);
                                if ui
                                    .add_sized(
                                        [180.0, 34.0],
                                        egui::Button::new("󰑐  Restart Session"),
                                    )
                                    .clicked()
                                {
                                    self.open_flashcards_trainer();
                                }
                            });
                            return;
                        }

                        let total_cards = self.flashcards_deck.len();
                        let card = self.flashcards_deck[self.flashcard_idx].clone();

                        // Progress & Slide Sync Bar
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "Card {} / {}",
                                    self.flashcard_idx + 1,
                                    total_cards
                                ))
                                .strong()
                                .color(rgb(self.theme.accent)),
                            );

                            ui.separator();

                            if ui
                                .small_button(format!("󰈙 Slide {}", card.page))
                                .on_hover_text("Jump PDF to this card's slide")
                                .clicked()
                            {
                                jump_to_slide = Some(card.page);
                            }

                            if ui
                                .checkbox(&mut self.flashcard_sync_slide, "Auto-sync PDF slide")
                                .changed()
                                && self.flashcard_sync_slide
                            {
                                jump_to_slide = Some(card.page);
                            }
                        });

                        ui.add_space(8.0);

                        // Question Card Box (Front)
                        egui::Frame::NONE
                            .fill(rgb(self.theme.bg_panel))
                            .stroke(egui::Stroke::new(1.0, rgb(self.theme.border)))
                            .corner_radius(egui::CornerRadius::same(8))
                            .inner_margin(egui::Margin::same(14))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.label(
                                    egui::RichText::new("QUESTION")
                                        .size(10.5)
                                        .strong()
                                        .color(rgb(self.theme.fg_muted)),
                                );
                                ui.add_space(4.0);
                                let q_md = notes::preprocess_markdown_view(&card.front);
                                egui_commonmark::CommonMarkViewer::new().show(
                                    ui,
                                    &mut self.commonmark_cache,
                                    &q_md,
                                );
                            });

                        ui.add_space(10.0);

                        // Answer Card Box (Back) or Reveal Button
                        if !self.flashcard_revealed {
                            let flip_key = ui.input(|i| {
                                i.key_pressed(egui::Key::Space) || i.key_pressed(egui::Key::Enter)
                            });
                            ui.vertical_centered(|ui| {
                                ui.add_space(18.0);
                                if ui
                                    .add_sized(
                                        [240.0, 38.0],
                                        egui::Button::new(
                                            egui::RichText::new("󰈈  Show Answer [Space]")
                                                .strong()
                                                .size(14.0),
                                        ),
                                    )
                                    .clicked()
                                    || flip_key
                                {
                                    self.flashcard_revealed = true;
                                }
                            });
                        } else {
                            egui::Frame::NONE
                                .fill(rgb(self.theme.bg_canvas))
                                .stroke(egui::Stroke::new(1.2, rgb(self.theme.success)))
                                .corner_radius(egui::CornerRadius::same(8))
                                .inner_margin(egui::Margin::same(14))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.label(
                                        egui::RichText::new("ANSWER")
                                            .size(10.5)
                                            .strong()
                                            .color(rgb(self.theme.success)),
                                    );
                                    ui.add_space(4.0);
                                    let a_md = notes::preprocess_markdown_view(&card.back);
                                    egui_commonmark::CommonMarkViewer::new().show(
                                        ui,
                                        &mut self.commonmark_cache,
                                        &a_md,
                                    );
                                });

                            ui.add_space(14.0);

                            // Self-grading buttons (1 = Again/Requeue, 2 = Good, 3 = Easy)
                            let (k1, k2, k3) = ui.input(|i| {
                                (
                                    i.key_pressed(egui::Key::Num1),
                                    i.key_pressed(egui::Key::Num2)
                                        || i.key_pressed(egui::Key::ArrowRight),
                                    i.key_pressed(egui::Key::Num3),
                                )
                            });

                            ui.columns(3, |cols| {
                                if cols[0]
                                    .add_sized(
                                        [cols[0].available_width(), 32.0],
                                        egui::Button::new(
                                            egui::RichText::new("󰑐 Again (Re-queue) [1]")
                                                .color(rgb(self.theme.error)),
                                        ),
                                    )
                                    .clicked()
                                    || k1
                                {
                                    self.flashcard_stats.0 += 1;
                                    // Re-queue card at the end of the session until mastered!
                                    self.flashcards_deck.push(card.clone());
                                    self.flashcard_idx += 1;
                                    self.flashcard_revealed = false;
                                    if self.flashcard_sync_slide {
                                        if let Some(next_c) =
                                            self.flashcards_deck.get(self.flashcard_idx)
                                        {
                                            jump_to_slide = Some(next_c.page);
                                        }
                                    }
                                }

                                if cols[1]
                                    .add_sized(
                                        [cols[1].available_width(), 32.0],
                                        egui::Button::new(
                                            egui::RichText::new("󰄬 Good [2 / →]")
                                                .color(rgb(self.theme.accent)),
                                        ),
                                    )
                                    .clicked()
                                    || k2
                                {
                                    self.flashcard_stats.1 += 1;
                                    self.flashcard_idx += 1;
                                    self.flashcard_revealed = false;
                                    if self.flashcard_sync_slide {
                                        if let Some(next_c) =
                                            self.flashcards_deck.get(self.flashcard_idx)
                                        {
                                            jump_to_slide = Some(next_c.page);
                                        }
                                    }
                                }

                                if cols[2]
                                    .add_sized(
                                        [cols[2].available_width(), 32.0],
                                        egui::Button::new(
                                            egui::RichText::new("󰄬 Easy [3]")
                                                .color(rgb(self.theme.success)),
                                        ),
                                    )
                                    .clicked()
                                    || k3
                                {
                                    self.flashcard_stats.2 += 1;
                                    self.flashcard_idx += 1;
                                    self.flashcard_revealed = false;
                                    if self.flashcard_sync_slide {
                                        if let Some(next_c) =
                                            self.flashcards_deck.get(self.flashcard_idx)
                                        {
                                            jump_to_slide = Some(next_c.page);
                                        }
                                    }
                                }
                            });
                        }
                    });
            });

        if let Some(target_p) = jump_to_slide {
            self.go_to_page(target_p, ui.ctx());
        }
    }

    fn prompt_download_original_pdf(&mut self, ctx: &egui::Context) {
        if !self.has_pdf() {
            return;
        }
        let stem = self
            .pdf_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("document");
        let default_name = format!("{}_original.pdf", stem);

        self.dialogs.open(
            ctx,
            DialogKind::ExportOriginalPdf,
            DialogSpec::save_file()
                .file_name(default_name)
                .filter("PDF Document", &["pdf"]),
        );
    }

    fn finish_download_original_pdf(&mut self, dest: PathBuf) {
        if !self.has_pdf() {
            return;
        }
        match notes::export_original_pdf(&self.pdf_path, &dest) {
            Ok(()) => {
                self.toast_message = Some((
                    format!("󰇚 Original PDF saved to {}", dest.display()),
                    std::time::Instant::now(),
                ));
            }
            Err(e) => {
                self.toast_message = Some((
                    format!("󰅚 Failed to save original PDF: {}", e),
                    std::time::Instant::now(),
                ));
            }
        }
    }

    fn modal_glass_frame(&self, alpha: u8) -> egui::Frame {
        let [r, g, b] = self.theme.bg_elevated;
        egui::Frame::NONE
            .fill(egui::Color32::from_rgba_unmultiplied(r, g, b, alpha))
            .stroke(egui::Stroke::new(1.0, rgb(self.theme.border)))
            .corner_radius(egui::CornerRadius::same(12))
            .inner_margin(egui::Margin::same(16))
            .shadow(egui::Shadow {
                offset: [0, 8],
                blur: 24,
                spread: 2,
                color: egui::Color32::from_black_alpha(140),
            })
    }
    fn rebuild_spellchecker(&mut self) {
        let mut sc = spellcheck::SpellChecker::new(&self.app_state.settings.extra_dictionaries);
        sc.enabled = self.app_state.settings.spellcheck_enabled;
        self.spellchecker = sc;
    }
    fn add_dictionary_file(&mut self, src: &std::path::Path) -> Result<String, String> {
    let file_name = src
        .file_name()
        .ok_or_else(|| "Invalid file name".to_string())?
        .to_string_lossy()
        .to_string();
 
    let dir = spellcheck::dict_dir();
    let mut dest = dir.join(&file_name);
    let mut n = 1;
    while dest.exists() {
        // Do not duplicate an existing entry for the same file.
        if std::fs::read(&dest).ok() == std::fs::read(src).ok() {
            return Err(format!("{} is already installed", file_name));
        }
        n += 1;
        dest = dir.join(format!("{}_{}", n, file_name));
    }
 
    std::fs::copy(src, &dest).map_err(|e| format!("Copy failed: {}", e))?;
    let name = match self.spellchecker.load_dictionary_file(&dest) {
        Ok(name) => name,
        Err(e) => {
            let _ = std::fs::remove_file(&dest);
            return Err(format!("Could not read dictionary: {}", e));
        }
    };
 
    self.app_state.settings.extra_dictionaries.push(dest);
    Ok(name)
}


    // Screenshots from the clipboard or files become image annotations or notes.
 
    /// Return the page under `pos` and its normalized point.
    /// Fall back to the current page center.
    fn page_at_pos(&self, pos: Option<egui::Pos2>) -> (u32, (f32, f32)) {
        if let Some(p) = pos {
            if let Some((page, r)) = self.last_slide_rects.iter().find(|(_, r)| r.contains(p)) {
                let nx = ((p.x - r.left()) / r.width()).clamp(0.0, 1.0);
                let ny = ((p.y - r.top()) / r.height()).clamp(0.0, 1.0);
                return (*page, (nx, ny));
            }
        }
        (self.current_page, (0.5, 0.5))
    }
 
    /// Call each frame after text-drag and focus updates.
       fn process_screenshots(&mut self, ctx: &egui::Context, any_focused: bool, in_slideshow: bool) {
        // 1. Completed job result.
        if let Some((target, shot)) = self.shots.poll() {
            self.finish_screenshot(target, shot);
        }
        if !self.has_pdf() || in_slideshow {
            return;
        }
        let pointer = ctx.input(|i| i.pointer.hover_pos().or(i.pointer.latest_pos()));
 
        // 2. Ctrl+V on the PDF.
        let over_canvas = pointer.map_or(false, |p| {
            self.canvas_rect.contains(p)
                || self.last_slide_rects.iter().any(|(_, r)| r.contains(p))
        });

        // Clear note focus when the PDF canvas is clicked.
        if over_canvas && ctx.input(|i| i.pointer.primary_pressed()) {
            if let Some(focused_id) = ctx.memory(|m| m.focused()) {
                ctx.memory_mut(|m| m.surrender_focus(focused_id));
            }
        }

        let ctrl_recent = screenshots::ctrl_recently_held(ctx);
        let ctrl_v = ctx.input(|i| {
            let ctrl = i.modifiers.ctrl || i.modifiers.command || ctrl_recent;
            let paste_ev = i.events.iter().any(|e| matches!(e, egui::Event::Paste(_)));
            let v_press = ctrl && i.key_pressed(egui::Key::V);
            let v_release = i.events.iter().any(|e| {
                matches!(
                    e,
                    egui::Event::Key { key: egui::Key::V, pressed: false, modifiers, .. }
                        if modifiers.ctrl || modifiers.command || ctrl
                )
            });
            paste_ev || v_press || v_release
        });

        if ctrl_v && !any_focused && over_canvas {
            let (page, center) = self.page_at_pos(pointer);
            self.shots.request_clipboard(ctx, ShotTarget::Pdf { page, center });
        }
 
        // 3. Dropped image files go to the PDF page or current note.
let dropped: Option<PathBuf> = ctx.input(|i: &InputState| {
    i.raw
        .dropped_files
        .iter()
        .find(|f| screenshots::is_image_path(f.path())) // Call path() to get &Path
        .map(|f| f.path().to_path_buf()) // Convert &Path to owned PathBuf
});
        if let Some(path) = dropped {
            let over_page = pointer.map_or(false, |p| self.last_slide_rects.iter().any(|(_, r)| r.contains(p)));
            let target = if over_page {
                let (page, center) = self.page_at_pos(pointer);
                ShotTarget::Pdf { page, center }
            } else {
                ShotTarget::Note {
                    page: self.note_page,
                    cursor: self.current_note.chars().count(),
                }
            };
            self.shots.request_file(ctx, target, path);
        }
    }

 
    fn finish_screenshot(&mut self, target: ShotTarget, shot: Option<screenshots::Shot>) {
        // Ignore clipboard values that are not images.
        let Some(shot) = shot.filter(|s| !s.is_empty()) else {
            return;
        };
        match target {
            ShotTarget::Pdf { page, center } => self.add_screenshot_annotation(page, center, shot),
            ShotTarget::Note { page, cursor } => self.insert_screenshot_in_note(page, cursor, shot),
        }
    }
 
    /// Create an image annotation using normalized top-left and bottom-right points.
    /// `text` stores the PNG filename in `<media dir>/screenshots/`.
    fn add_screenshot_annotation(&mut self, page: u32, center: (f32, f32), shot: screenshots::Shot) {
        if !self.has_pdf() {
            return;
        }
        let dir = audio::AudioDocIndex::media_dir_for_pdf(&self.pdf_path).join("screenshots");
        let shot = shot.downscaled(1600);
        let (iw, ih) = (shot.width as f32, shot.height as f32);
        let png = match screenshots::save_png(&shot, &dir) {
            Ok(p) => p,
            Err(e) => {
                self.toast_message = Some((format!("Screenshot failed: {e}"), std::time::Instant::now()));
                return;
            }
        };
        let Some(name) = png.file_name().map(|n| n.to_string_lossy().to_string()) else {
            return;
        };
 
        // Use 40% of page width and preserve the image aspect ratio.
        let page_aspect = self
            .last_slide_rects
            .iter()
            .find(|(p, _)| *p == page)
            .map(|(_, r)| r.width() / r.height())
            .or_else(|| (self.page_pt.1 > 0.0).then(|| self.page_pt.0 / self.page_pt.1))
            .unwrap_or(4.0 / 3.0);
        let mut nw = 0.4_f32;
        let mut nh = nw * page_aspect * ih / iw;
        if nh > 0.9 {
            nw *= 0.9 / nh;
            nh = 0.9;
        }
        let x0 = (center.0 - nw / 2.0).clamp(0.0, 1.0 - nw);
        let y0 = (center.1 - nh / 2.0).clamp(0.0, 1.0 - nh);
 
        self.push_undo_snapshot(page);
        let list = self.annotations.entry(page).or_default();
        list.push(notes::PageAnnotation {
            kind: notes::AnnotationKind::Image,
            color: [255, 255, 255],
            points: vec![(x0, y0), (x0 + nw, y0 + nh)],
            text: name,
            font_size: self.textbox_font_size,
            bold: false,
            italic: false,
            stroke_width: self.stroke_width
        });
        self.selected_image = Some((page, list.len() - 1));
        let _ = notes::save_doc_data(
            &self.pdf_path,
            &self.notes,
            &self.annotations,
            self.blank_pages.clone(),
            &self.sketches,
        );
        self.toast_message = Some((
            format!("Screenshot added to page {page}"),
            std::time::Instant::now(),
        ));
    }
 
    fn insert_screenshot_in_note(&mut self, page: u32, cursor: usize, shot: screenshots::Shot) {
        if !self.has_pdf() {
            return;
        }
        let dir = audio::AudioDocIndex::media_dir_for_pdf(&self.pdf_path).join("screenshots");
        let msg = match screenshots::save_png(&shot.downscaled(1600), &dir) {
            Ok(png) => {
                let md = format!("\n{}\n", screenshots::markdown_image(&png));
                if page == self.note_page {
                    let at = self
                        .current_note
                        .char_indices()
                        .nth(cursor)
                        .map(|(i, _)| i)
                        .unwrap_or(self.current_note.len());
                    self.current_note.insert_str(at, &md);
                    self.note_is_dirty = true;
                } else {
                    self.notes.entry(page).or_default().push_str(&md);
                    self.sync_and_save();
                }
                "Screenshot added to the note".to_string()
            }
            Err(e) => format!("Screenshot failed: {e}"),
        };
        self.toast_message = Some((msg, std::time::Instant::now()));
    }
 
    /// Draw page image annotations and handle selection and movement.
    /// resizing with fixed aspect ratio, and deletion with Delete or Backspace.
    /// Interactions are enabled only with the Cursor tool so other tools
    /// can draw over the image.
    fn image_annotations_ui(&mut self, ui: &mut egui::Ui, page_num: u32, slide_rect: egui::Rect) {
        let img_idx: Vec<usize> = match self.annotations.get(&page_num) {
            Some(list) => list
                .iter()
                .enumerate()
                .filter(|(_, a)| a.kind == notes::AnnotationKind::Image && a.points.len() >= 2)
                .map(|(i, _)| i)
                .collect(),
            None => return,
        };
        if img_idx.is_empty() {
            return;
        }
        let cursor_tool = matches!(self.annotation_tool, AnnotationTool::Cursor);
        if !cursor_tool && matches!(self.selected_image, Some((p, _)) if p == page_num) {
            self.selected_image = None;
        }
        if let Some((p, i)) = self.selected_image {
            if p == page_num && !img_idx.contains(&i) {
                self.selected_image = None;
            }
        }
 
        let dir = audio::AudioDocIndex::media_dir_for_pdf(&self.pdf_path).join("screenshots");
        let (sw, sh) = (slide_rect.width().max(1.0), slide_rect.height().max(1.0));
        let to_screen = |(nx, ny): (f32, f32)| {
            egui::pos2(slide_rect.left() + nx * sw, slide_rect.top() + ny * sh)
        };
        let to_norm = |p: egui::Pos2| ((p.x - slide_rect.left()) / sw, (p.y - slide_rect.top()) / sh);
 
        let mut hovered_any = false;
        let mut new_drag: Option<ImageDrag> = None;
        let mut cursor_icon: Option<egui::CursorIcon> = None;
 
        // Draw each image and its hit area.
        for &i in &img_idx {
            let (p0, p1, name) = {
                let a = &self.annotations[&page_num][i];
                (a.points[0], a.points[1], a.text.clone())
            };
            let rect = egui::Rect::from_two_pos(to_screen(p0), to_screen(p1));
            let uri = paths::file_uri(&dir.join(&name));
            egui::Image::new(uri).paint_at(ui, rect);
 
            if !cursor_tool {
                continue;
            }
            let resp = ui.interact(rect, ui.make_persistent_id(("img_ann", page_num, i)), egui::Sense::click_and_drag());
            if resp.hovered() || resp.dragged() {
                hovered_any = true;
                cursor_icon = Some(if resp.dragged() {
                    egui::CursorIcon::Grabbing
                } else {
                    egui::CursorIcon::Grab
                });
            }
            if resp.clicked() || resp.drag_started() {
                self.selected_image = Some((page_num, i));
            }
            if resp.drag_started() {
                // Start dragging from the exact press position.
                let origin = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos());
                if let Some(pos) = origin {
                    let (px, py) = to_norm(pos);
                    new_drag = Some(ImageDrag {
                        page: page_num,
                        idx: i,
                        mode: ImageDragMode::Move {
                            grab: (px - p0.0.min(p1.0), py - p0.1.min(p1.1)),
                        },
                    });
                }
            }
        }
 
        // Selected image border and corner handles.
        let mut delete_clicked = false;
        if let Some((sp, si)) = self.selected_image {
            if cursor_tool && sp == page_num {
                let (p0, p1) = {
                    let a = &self.annotations[&page_num][si];
                    (a.points[0], a.points[1])
                };
                let (x0, x1) = (p0.0.min(p1.0), p0.0.max(p1.0));
                let (y0, y1) = (p0.1.min(p1.1), p0.1.max(p1.1));
                let rect = egui::Rect::from_two_pos(to_screen((x0, y0)), to_screen((x1, y1)));
                ui.painter().rect_stroke(
                    rect,
                    egui::CornerRadius::ZERO,
                    egui::Stroke::new(1.5_f32, rgb(self.theme.accent)),
                    egui::StrokeKind::Outside,
                );

                // Corners are TL, TR, BR, BL; the opposite corner is (k + 2) % 4.
                let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
                for (k, &c) in corners.iter().enumerate() {
                    let pos = to_screen(c);
                    let hrect = egui::Rect::from_center_size(pos, egui::vec2(12.0, 12.0));
                    let resp = ui.interact(
                        hrect,
                        ui.make_persistent_id(("img_ann_handle", page_num, si, k)),
                        egui::Sense::drag(),
                    );
                    ui.painter().rect_filled(hrect, egui::CornerRadius::same(2), rgb(self.theme.accent));
                    ui.painter().rect_stroke(
                        hrect,
                        egui::CornerRadius::same(2),
                        egui::Stroke::new(1.0_f32, egui::Color32::WHITE),
                        egui::StrokeKind::Inside,
                    );
                    if resp.hovered() || resp.dragged() {
                        hovered_any = true;
                        cursor_icon = Some(if k % 2 == 0 {
                            egui::CursorIcon::ResizeNwSe
                        } else {
                            egui::CursorIcon::ResizeNeSw
                        });
                    }
                    if resp.drag_started() {
                        new_drag = Some(ImageDrag {
                            page: page_num,
                            idx: si,
                            mode: ImageDragMode::Resize {
                                anchor: corners[(k + 2) % 4],
                            },
                        });
                    }
                }

                // Compact floating delete button in the top-right corner.
                let pad = 6.0;
                let radius = 10.0; // 20 px diameter.
                let btn_center = egui::pos2(
                    (rect.right() - pad - radius).max(rect.left() + radius),
                    (rect.top() + pad + radius).min(rect.bottom() - radius),
                );
                let btn_rect = egui::Rect::from_center_size(btn_center, egui::vec2(radius * 2.0, radius * 2.0));
                let del_resp = ui.interact(
                    btn_rect,
                    ui.make_persistent_id(("img_ann_delete_btn", page_num, si)),
                    egui::Sense::click(),
                );

                let (bg_color, border_color, icon_color) = if del_resp.hovered() {
                    hovered_any = true;
                    cursor_icon = Some(egui::CursorIcon::PointingHand);
                    (
                        rgb(self.theme.bg_active),
                        rgb(self.theme.border),
                        egui::Color32::WHITE,
                    )
                } else {
                    (
                        rgb(self.theme.bg_hover),
                        rgb(self.theme.border),
                        rgb(self.theme.fg_muted),
                    )
                };

                // Circular background.
                ui.painter().circle_filled(btn_center, radius, bg_color);
                ui.painter().circle_stroke(btn_center, radius, egui::Stroke::new(1.0, border_color));

                // Compact delete icon.
                let c = btn_center;
                let s = egui::Stroke::new(1.2, icon_color);
                let s_thin = egui::Stroke::new(1.0, icon_color);

                // Horizontal lid.
                ui.painter().line_segment(
                    [c + egui::vec2(-4.2, -2.6), c + egui::vec2(4.2, -2.6)],
                    s,
                );
                // Top handle.
                ui.painter().line_segment(
                    [c + egui::vec2(-1.5, -4.0), c + egui::vec2(1.5, -4.0)],
                    s_thin,
                );
                ui.painter().line_segment(
                    [c + egui::vec2(-1.5, -4.0), c + egui::vec2(-1.5, -2.6)],
                    s_thin,
                );
                ui.painter().line_segment(
                    [c + egui::vec2(1.5, -4.0), c + egui::vec2(1.5, -2.6)],
                    s_thin,
                );
                // Bin body.
                let body = vec![
                    c + egui::vec2(-3.2, -2.6),
                    c + egui::vec2(-2.6, 4.0),
                    c + egui::vec2(2.6, 4.0),
                    c + egui::vec2(3.2, -2.6),
                ];
                ui.painter().add(egui::Shape::line(body, s));
                // Due linee verticali interne
                ui.painter().line_segment(
                    [c + egui::vec2(-1.0, -0.8), c + egui::vec2(-0.9, 2.4)],
                    s_thin,
                );
                ui.painter().line_segment(
                    [c + egui::vec2(1.0, -0.8), c + egui::vec2(0.9, 2.4)],
                    s_thin,
                );

                if del_resp.clicked() {
                    delete_clicked = true;
                }
            }
        }
        if let Some(icon) = cursor_icon {
            ui.ctx().set_cursor_icon(icon);
        }
 
        // Drag in progress.
        if let Some(d) = new_drag {
            self.push_undo_snapshot(page_num);
            self.image_drag = Some(d);
        }
        if let Some(d) = self.image_drag {
            if d.page == page_num {
                let (down, pointer) = ui.input(|i| {
                    (
                        i.pointer.primary_down(),
                        i.pointer.interact_pos().or(i.pointer.hover_pos()),
                    )
                });
                if down {
                    if let (Some(pos), Some(ann)) = (
                        pointer,
                        self.annotations.get_mut(&page_num).and_then(|v| v.get_mut(d.idx)),
                    ) {
                        if ann.kind == notes::AnnotationKind::Image && ann.points.len() >= 2 {
                            let (px, py) = to_norm(pos);
                            let (a0, a1) = (ann.points[0], ann.points[1]);
                            let (x0, x1) = (a0.0.min(a1.0), a0.0.max(a1.0));
                            let (y0, y1) = (a0.1.min(a1.1), a0.1.max(a1.1));
                            let (w, h) = (x1 - x0, y1 - y0);
                            match d.mode {
                                ImageDragMode::Move { grab } => {
                                    let nx = (px - grab.0).clamp(0.0, (1.0 - w).max(0.0));
                                    let ny = (py - grab.1).clamp(0.0, (1.0 - h).max(0.0));
                                    ann.points = vec![(nx, ny), (nx + w, ny + h)];
                                }
                                ImageDragMode::Resize { anchor } => {
                                    let aspect = (w * sw) / (h * sh).max(1.0); // Pixel aspect ratio.
                                    let sx = if px >= anchor.0 { 1.0 } else { -1.0 };
                                    let sy = if py >= anchor.1 { 1.0 } else { -1.0 };
                                    let max_w_x = if sx > 0.0 { 1.0 - anchor.0 } else { anchor.0 };
                                    let max_h_y = if sy > 0.0 { 1.0 - anchor.1 } else { anchor.1 };
                                    // h_n = w_n * sw / aspect / sh
                                    let max_w = max_w_x.min(max_h_y * aspect * sh / sw);
                                    let min_w = 24.0 / sw;
                                    let new_w = (px - anchor.0).abs().clamp(min_w.min(max_w), max_w.max(0.001));
                                    let new_h = new_w * sw / aspect / sh;
                                    let c = (anchor.0 + sx * new_w, anchor.1 + sy * new_h);
                                    ann.points = vec![
                                        (anchor.0.min(c.0), anchor.1.min(c.1)),
                                        (anchor.0.max(c.0), anchor.1.max(c.1)),
                                    ];
                                }
                            }
                        }
                    }
                    ui.ctx().request_repaint();
                } else {
                    self.image_drag = None;
                    let _ = notes::save_doc_data(
                        &self.pdf_path,
                        &self.notes,
                        &self.annotations,
                        self.blank_pages.clone(),
                        &self.sketches,
                    );
                }
            }
        }
 
        // Clear selection when clicking outside all images.
        let clicked_elsewhere = ui.input(|i| {
            i.pointer.primary_clicked() && i.pointer.interact_pos().map_or(false, |p| slide_rect.contains(p))
        });
        if cursor_tool && clicked_elsewhere && !hovered_any && self.image_drag.is_none() {
            if matches!(self.selected_image, Some((p, _)) if p == page_num) {
                self.selected_image = None;
            }
        }
 
        // Delete the selected image with Delete, Backspace, or the close icon.
        if let Some((sp, si)) = self.selected_image {
            let typing = ui.ctx().memory(|m| m.focused().is_some());
            let key_del = !typing
                && ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace));
            if cursor_tool && sp == page_num && (key_del || delete_clicked) && self.image_drag.is_none() {
                self.push_undo_snapshot(page_num);
                if let Some(list) = self.annotations.get_mut(&page_num) {
                    if si < list.len() && list[si].kind == notes::AnnotationKind::Image {
                        list.remove(si);
                    }
                }
                self.selected_image = None;
                let _ = notes::save_doc_data(
                    &self.pdf_path,
                    &self.notes,
                    &self.annotations,
                    self.blank_pages.clone(),
                    &self.sketches,
                );
            }
        }
    }
    fn render_markdown_preview_with_arrow(
        &mut self,
        ui: &mut egui::Ui,
        scroll_id_str: &'static str,
        md_source: &str,
        header_ui: impl FnOnce(&mut egui::Ui),
    ) {
        let scroll_id = ui.make_persistent_id(scroll_id_str);
        let anim_scroll_id = scroll_id.with("anim_target");
        let current_offset_y = ui.data(|d| d.get_temp::<f32>(scroll_id.with("last_offset")).unwrap_or(0.0));

        let mut scroll_area = egui::ScrollArea::vertical()
            .id_salt(scroll_id_str)
            .auto_shrink([false, false]);

        if let Some(target) = ui.data(|d| d.get_temp::<f32>(anim_scroll_id)) {
            let diff = target - current_offset_y;
            if diff.abs() > 1.0 {
                scroll_area = scroll_area.vertical_scroll_offset(current_offset_y + diff * 0.2);
                ui.ctx().request_repaint();
            } else {
                ui.data_mut(|d| d.remove::<f32>(anim_scroll_id));
            }
        }

        let note_font_sz = self.app_state.settings.note_font_size.clamp(10.0, 28.0);

        let scroll_out = scroll_area.show(ui, |ui| {
            // Keep the Markdown viewport and any embedded images within the panel width
            // on every frame while the side-panel splitter is moving.
            let preview_width = ui.available_width().max(1.0);
            ui.take_available_width();
            header_ui(ui);
            ui.scope(|ui| {
                ui.take_available_width();
                let style = ui.style_mut();
                style.visuals.override_text_color = None;
                style.visuals.widgets.noninteractive.fg_stroke =
                    egui::Stroke::new(1.0, rgb(self.theme.fg_primary));
                style.visuals.widgets.active.fg_stroke =
                    egui::Stroke::new(2.0, rgb(self.theme.accent));
                style.visuals.hyperlink_color = rgb(self.theme.accent);
                style.visuals.code_bg_color = rgb(self.theme.bg_widget);
                style.visuals.extreme_bg_color = rgb(self.theme.bg_canvas);
                style.spacing.item_spacing.y = 7.0;
                style.interaction.selectable_labels = true;

                style.text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(note_font_sz));
                style.text_styles.insert(egui::TextStyle::Monospace, egui::FontId::monospace(note_font_sz * 0.92));
                style.text_styles.insert(egui::TextStyle::Heading, egui::FontId::proportional(note_font_sz * 1.35));

                let pre_defs = notes::preprocess_definitions(md_source);
                let rendered_md = notes::preprocess_markdown_view(&pre_defs);

                egui_commonmark::CommonMarkViewer::new()
                    .max_image_width(Some(preview_width.ceil() as usize))
                    .show(
                        ui,
                        &mut self.commonmark_cache,
                        &rendered_md,
                    );
                ui.add_space(28.0);
            });

            let copied_texts = ui.ctx().output_mut(|output| {
                let mut copied = Vec::new();
                output.commands.retain(|command| {
                    if let egui::OutputCommand::CopyText(text) = command {
                        copied.push(text.clone());
                        false
                    } else {
                        true
                    }
                });
                copied
            });
            for text in copied_texts {
                self.clipboard.set_text(&text);
            }
        });

        ui.data_mut(|d| {
            d.insert_temp(scroll_id.with("last_offset"), scroll_out.state.clamped_offset().y);
        });

        // Draw the fade gradient and arrow badge.
        let viewport_rect = scroll_out.inner_rect;
        let content_h = scroll_out.content_size.y;
        let view_h = viewport_rect.height();
        let offset_y = scroll_out.state.clamped_offset().y;
        let remaining_px = content_h - (offset_y + view_h);

        if remaining_px > 8.0 {
            let alpha = (remaining_px / 35.0).clamp(0.25, 0.95);
            let elevated = rgb(self.theme.bg_elevated);
            let accent = rgb(self.theme.accent);
            let foreground = rgb(self.theme.fg_primary);

            let bg_col = egui::Color32::from_rgba_unmultiplied(elevated.r(), elevated.g(), elevated.b(), (alpha * 235.0) as u8);
            let border_col = egui::Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), (alpha * 180.0) as u8);
            let text_col = egui::Color32::from_rgba_unmultiplied(foreground.r(), foreground.g(), foreground.b(), (alpha * 255.0) as u8);

            let fade_h = 24.0_f32.min(view_h * 0.25);
            let fade_rect = egui::Rect::from_min_max(
                egui::pos2(viewport_rect.left(), viewport_rect.bottom() - fade_h),
                viewport_rect.right_bottom(),
            );
            let mut mesh = egui::Mesh::default();
            let top_col = egui::Color32::TRANSPARENT;
            let bot_col = egui::Color32::from_rgba_unmultiplied(elevated.r(), elevated.g(), elevated.b(), (alpha * 200.0) as u8);
            mesh.colored_vertex(fade_rect.left_top(), top_col);
            mesh.colored_vertex(fade_rect.right_top(), top_col);
            mesh.colored_vertex(fade_rect.right_bottom(), bot_col);
            mesh.colored_vertex(fade_rect.left_bottom(), bot_col);
            mesh.add_triangle(0, 1, 2);
            mesh.add_triangle(0, 2, 3);
            ui.painter().add(egui::Shape::Mesh(std::sync::Arc::new(mesh)));

            let pill_rect = egui::Rect::from_center_size(
                egui::pos2(viewport_rect.center().x, viewport_rect.bottom() - 15.0),
                egui::vec2(34.0, 22.0),
            );
            let resp = ui.interact(pill_rect, scroll_id.with("more_below_btn"), egui::Sense::click());
            let pill_bg = if resp.hovered() {
                let active = rgb(self.theme.bg_active);
                egui::Color32::from_rgba_unmultiplied(active.r(), active.g(), active.b(), 245)
            } else {
                bg_col
            };

            ui.painter().rect(
                pill_rect,
                egui::CornerRadius::same(11),
                pill_bg,
                egui::Stroke::new(1.0, border_col),
                egui::epaint::StrokeKind::Inside,
            );
            ui.painter().text(
                pill_rect.center(),
                egui::Align2::CENTER_CENTER,
                "󰁅",
                egui::FontId::proportional(14.0),
                text_col,
            );

            if resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if resp.clicked() {
                let target = (offset_y + view_h * 0.7).min(content_h - view_h);
                ui.data_mut(|d| d.insert_temp(anim_scroll_id, target));
                ui.ctx().request_repaint();
            }
        }
    }

    fn render_notes_panel_contents(
        &mut self,
        ui: &mut egui::Ui,
        text_edit_id: egui::Id,
        use_bottom: bool,
    ) {
        // A resizable side panel must keep using its full assigned width; otherwise its
        // contents can keep the previous frame's narrower min-rect while the splitter moves.
        ui.take_available_width();
        let panel_bg = ui.visuals().panel_fill;
        let page = self.current_page;
        let media_dir = AudioDocIndex::media_dir_for_pdf(&self.pdf_path);

        // Full panel rectangle, including the user-selected height.
        let panel_rect = ui.max_rect();

        let memo_count = if self.app_state.settings.audio_notes {
            self.audio_index
                .page_memos
                .get(&page)
                .map(|v| v.len())
                .unwrap_or(0)
        } else {
            0
        };
        let has_page_memos = memo_count > 0;

        // 1. Determine which bottom controls are visible.
        let playing = self.audio_engine.is_playing();
        let pos_ms = self.audio_engine.current_pos_ms();
        let has_wav = self.audio_index.lecture_wav.is_some();
        let has_transcript = !self.audio_index.segments.is_empty();

        let has_relevant_segment = if playing || pos_ms > 0 {
            self.audio_index
                .segment_index_at(pos_ms)
                .and_then(|i| self.audio_index.segments.get(i))
                .is_some()
                || self.audio_index.segments.iter().any(|s| s.page == page)
        } else {
            self.audio_index.segments.iter().any(|s| s.page == page)
        };

        let show_ticker = self.app_state.settings.audio_dock
            && !self.audio_dock_open
            && (has_relevant_segment || self.is_transcribing);

        let memo_h = if has_page_memos {
            let visible_rows = memo_count.min(3);
            32.0 + visible_rows as f32 * 24.0
        } else {
            0.0
        };

        // Use panel_rect.height() for stable sizing.
        let max_h = (panel_rect.height() * 0.55).max(140.0);

        let default_h: f32 = if has_transcript {
            220.0
        } else if has_wav {
            150.0
        } else {
            125.0
        };

        let dock_h = if !self.app_state.settings.audio_dock {
            0.0
        } else if self.audio_dock_open {
            if self.audio_dock_height <= 0.0 {
                self.audio_dock_height = default_h;
            }
            // Do not overwrite the saved dock height when the panel is constrained.
            // Clamp only the height used for this frame.
            self.audio_dock_height.clamp(100.0, max_h)
        } else if show_ticker {
            32.0
        } else {
            0.0
        };

        let top_handle_offset = if use_bottom {
            let handle_rect = egui::Rect::from_center_size(
                egui::pos2(panel_rect.center().x, panel_rect.top() + 6.0),
                egui::vec2(42.0, 4.0),
            );

            let pointer_in_handle = ui.rect_contains_pointer(handle_rect.expand(4.0));
            let color = if pointer_in_handle {
                rgb(self.theme.accent)
            } else {
                rgb(self.theme.fg_muted)
            };

            ui.painter().rect_filled(
                handle_rect,
                egui::CornerRadius::same(2),
                color,
            );
            12.0
        } else {
            0.0
        };

        let full_panel_rect = egui::Rect::from_min_max(
            egui::pos2(panel_rect.left(), panel_rect.top() + top_handle_offset),
            panel_rect.max,
        );

        let total_bottom_h = memo_h + dock_h;
        let min_notes_h = 65.0;
        let notes_max_h = (full_panel_rect.height() - total_bottom_h).max(min_notes_h);

        // Split rectangles for notes and audio.
        let notes_rect = egui::Rect::from_min_size(
            full_panel_rect.min,
            egui::vec2(full_panel_rect.width(), notes_max_h),
        );
        // Keep the audio area anchored to the same panel edges in every notes mode.
        // The audio controls apply their own inset, so Sketch must not shrink this rect.
        let side_pad = 0.0;
        let bottom_rect = egui::Rect::from_min_max(
            egui::pos2(full_panel_rect.left() + side_pad, notes_rect.bottom()),
            egui::pos2(full_panel_rect.right() - side_pad, full_panel_rect.bottom()),
        );

        // ---------------------------------------------------------
        // A. Notes area, constrained to `notes_rect`.
        // ---------------------------------------------------------
        let mut notes_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(notes_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        notes_ui.set_clip_rect(notes_rect);

        {
            let ui = &mut notes_ui;
            ui.take_available_width();

            // A compact, frameless single row: page navigation on the left, mode controls fill the rest.
            let sk_on = self.sketch_enabled();
            let button_radius = egui::CornerRadius::same(self.theme.corner_radius.saturating_sub(1));

            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(2.0, 0.0);
                let full_width = ui.available_width();
                let compact = full_width < 320.0;

                // Page navigation stays compact and left aligned.
                let lock_icon = if self.lock_note_page { "󰌾" } else { "󰌿" };
                if ui
                    .add_sized(
                        [18.0, 23.0],
                        egui::Button::new(egui::RichText::new(lock_icon).size(11.0))
                            .selected(self.lock_note_page)
                            .corner_radius(button_radius),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text("Lock or unlock the note page while scrolling the PDF")
                    .clicked()
                {
                    self.lock_note_page = !self.lock_note_page;
                    if !self.lock_note_page && self.note_page != self.current_page {
                        self.switch_note_page(self.current_page);
                    }
                }

                if ui
                    .add_enabled(
                        self.note_page > 1,
                        egui::Button::new(egui::RichText::new("\u{F438}").size(11.0))
                            .min_size(egui::vec2(17.0, 23.0))
                            .corner_radius(button_radius),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text("Previous notes page")
                    .clicked()
                {
                    self.lock_note_page = true;
                    self.switch_note_page(self.note_page - 1);
                }

                let page_label = if compact {
                    self.note_page.to_string()
                } else {
                    format!("p. {}", self.note_page)
                };
                if ui
                    .add_sized(
                        [if compact { 23.0 } else { 35.0 }, 23.0],
                        egui::Button::new(egui::RichText::new(page_label).size(10.0).strong())
                            .corner_radius(button_radius),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text("Return notes to the current PDF page")
                    .clicked()
                {
                    self.note_page = self.current_page;
                    self.lock_note_page = false;
                }

                if ui
                    .add_enabled(
                        self.note_page < self.total_pages,
                        egui::Button::new(egui::RichText::new("\u{F44A}").size(11.0))
                            .min_size(egui::vec2(17.0, 23.0))
                            .corner_radius(button_radius),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text("Next notes page")
                    .clicked()
                {
                    self.lock_note_page = true;
                    self.switch_note_page(self.note_page + 1);
                }

                ui.add_space(3.0);
                ui.separator();
                ui.add_space(4.0);

                // Sketch stays a small icon tab. Edit and Preview divide the remaining row.
                // Switch to icon-only controls while there is still room, rather than letting
                // the Preview label become squeezed or clipped at narrow panel widths.
                let mode_available = ui.available_width().max(0.0);
                let sketch_width = if sk_on { 23.0 } else { 0.0 };
                let mode_gap_total = if sk_on { 4.0 } else { 2.0 };
                let primary_width = ((mode_available - sketch_width - mode_gap_total) / 2.0)
                    .max(18.0);
                let show_labels = primary_width >= 84.0;
                let edit_label = if show_labels { "󰏫  Edit" } else { "󰏫" };
                let preview_label = if show_labels { "󰈈  Preview" } else { "󰈈" };
                let mode_font_size = if show_labels { 10.0 } else { 11.0 };

                if sk_on {
                    if ui
                        .add_sized(
                            [sketch_width, 22.0],
                            egui::Button::new(egui::RichText::new("󰠱").size(11.0).strong())
                                .selected(self.note_mode == NoteMode::Sketch)
                                .corner_radius(button_radius),
                        )
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text("Sketch: a separate scratchpad for this page (Alt+S)")
                        .clicked()
                    {
                        self.note_mode = NoteMode::Sketch;
                    }
                }

                if ui
                    .add_sized(
                        [primary_width, 22.0],
                        egui::Button::new(egui::RichText::new(edit_label).size(mode_font_size).strong())
                            .selected(self.note_mode == NoteMode::Edit)
                            .corner_radius(button_radius),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text("Edit your Markdown and LaTeX notes")
                    .clicked()
                {
                    self.note_mode = NoteMode::Edit;
                    self.focus_note_editor = true;
                }

                if ui
                    .add_sized(
                        [primary_width, 22.0],
                        egui::Button::new(egui::RichText::new(preview_label).size(mode_font_size).strong())
                            .selected(self.note_mode == NoteMode::Preview)
                            .corner_radius(button_radius),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text("Preview the formatted notes")
                    .clicked()
                {
                    self.note_mode = NoteMode::Preview;
                }
            });

            let sketch_open = self.note_mode == NoteMode::Sketch && sk_on;
            ui.add_space(8.0);

            let is_edit = matches!(self.note_mode, NoteMode::Edit);

            // Keep the text origin and usable width identical in Edit and Preview.
            // The Edit field has zero internal margin, so this shared frame inset is
            // also the exact origin used by the Markdown viewer.
            let text_content_margin = egui::Margin::symmetric(14, 8);
            let (notes_bg, notes_stroke, notes_margin) = if is_edit {
                (
                    rgb(self.theme.bg_widget),
                    egui::Stroke::new(1.0, rgb(self.theme.border)),
                    text_content_margin,
                )
            } else if sketch_open {
                // The canvas fills the whole panel below the toolbar.
                (
                    egui::Color32::TRANSPARENT,
                    egui::Stroke::NONE,
                    egui::Margin::ZERO,
                )
            } else {
                // Preview sits directly on the notes panel background, without a card surface.
                (
                    egui::Color32::TRANSPARENT,
                    egui::Stroke::NONE,
                    text_content_margin,
                )
            };
            // Main Notes Area
            egui::Frame::NONE
                .fill(notes_bg)
                .stroke(notes_stroke)
                .corner_radius(egui::CornerRadius::same(self.theme.corner_radius))
                .inner_margin(notes_margin)
                .show(ui, |ui| {
                    // Reclaim the panel's current width after a live resize; do not keep a
                    // narrower min-rect from the previous frame.
                    ui.take_available_width();
                    if self.note_mode == NoteMode::Sketch {
                        let sketch_page = self.note_page;
                        if sketch::show(ui, &mut self.sketch, sketch_page, &mut self.sketches) {
                            self.save_sketch_data();
                        }
                        return;
                    }

                    let scroll_id = ui.make_persistent_id("notes_scroll");
                    let anim_scroll_id = ui.make_persistent_id("notes_anim_scroll_target");

                    let current_offset_y = ui.data(|d| {
                        d.get_temp::<f32>(scroll_id.with("last_offset")).unwrap_or(0.0)
                    });

                    let preview_view_width = ui.available_width().max(1.0);
                    ui.take_available_width();
                    let mut scroll_area = egui::ScrollArea::vertical()
                        .id_salt("notes_scroll")
                        .auto_shrink([false, false])
                        .max_width(preview_view_width);

                    if let Some(target) = ui.data(|d| d.get_temp::<f32>(anim_scroll_id)) {
                        let diff = target - current_offset_y;
                        if diff.abs() > 1.0 {
                            let next_offset = current_offset_y + diff * 0.2;
                            scroll_area = scroll_area.vertical_scroll_offset(next_offset);
                            ui.ctx().request_repaint();
                        } else {
                            ui.data_mut(|d| d.remove::<f32>(anim_scroll_id));
                        }
                    }

                    let note_font_sz = self.app_state.settings.note_font_size.clamp(10.0, 28.0);
                    let edit_bottom_pad = note_font_sz * 3.5;
                    let mut pending_scroll_px = 0.0_f32;

                    let scroll_out = scroll_area.show(ui, |ui| match self.note_mode {
                        NoteMode::Edit => {
                            if self.focus_note_editor {
                                ui.ctx().memory_mut(|m| m.request_focus(text_edit_id));
                                self.focus_note_editor = false;
                            }
                            // Do not claim or rewrite the clipboard when the editor gains focus.
                            // Ctrl+V must read the current system clipboard.

                            let mut copied_or_cut_text: Option<String> = None;
                            if ui.ctx().memory(|m| m.has_focus(text_edit_id)) {
                                let mut has_paste_event = false;
                                let mut fallback_v_release = false;
                                let mut has_copy_or_cut = false;
                                let mut trigger_save = false;

                                let ctrl_recent = screenshots::ctrl_recently_held(ui.ctx());
                                ui.input(|i| {
                                    let ctrl = i.modifiers.ctrl || i.modifiers.command || ctrl_recent;
                                    for ev in &i.events {
                                        match ev {
                                            egui::Event::Paste(_) => {
                                                has_paste_event = true;
                                                self.paste_handled_on_press = true;
                                            }
                                            egui::Event::Copy | egui::Event::Cut => {
                                                has_copy_or_cut = true;
                                            }
                                            egui::Event::Key {
                                                key: egui::Key::V,
                                                pressed: false,
                                                modifiers,
                                                ..
                                            } if (modifiers.ctrl || modifiers.command || ctrl) => {
                                                if self.paste_handled_on_press {
                                                    self.paste_handled_on_press = false;
                                                } else {
                                                    fallback_v_release = true;
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                    if (i.modifiers.ctrl || i.modifiers.command) && i.key_pressed(egui::Key::S) {
                                        trigger_save = true;
                                    }
                                });

                                let selected_slice: Option<String> = if has_copy_or_cut {
                                    egui::text_edit::TextEditState::load(ui.ctx(), text_edit_id)
                                        .and_then(|state| state.cursor.char_range())
                                        .and_then(|range| {
                                            let min = range.primary.index.min(range.secondary.index);
                                            let max = range.primary.index.max(range.secondary.index);
                                            if min != max {
                                                Some(
                                                    self.current_note
                                                        .chars()
                                                        .skip(min.into())
                                                        .take((max - min).into())
                                                        .collect::<String>(),
                                                )
                                            } else {
                                                None
                                            }
                                        })
                                } else {
                                    None
                                };

                                if has_paste_event || fallback_v_release {
                                    // 1. Read text immediately through arboard when available.
                                    let sys_text = self.clipboard.get_text();
                                    let is_klipper_img = sys_text
                                        .as_deref()
                                        .map_or(false, screenshots::is_image_placeholder);

                                    if let Some(sys_text) = sys_text.filter(|_| !is_klipper_img) {
                                        // Plain text: use the original fast path.
                                        ui.input_mut(|i| {
                                            if has_paste_event {
                                                for ev in &mut i.events {
                                                    if let egui::Event::Paste(t) = ev {
                                                        *t = sys_text.clone();
                                                    }
                                                }
                                            } else {
                                                i.events.push(egui::Event::Paste(sys_text));
                                            }
                                        });
                                    } else {
                                        // If no text is available, try to paste an image in the background.
                                        ui.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Paste(_))));
                                        let cursor = egui::text_edit::TextEditState::load(ui.ctx(), text_edit_id)
                                            .and_then(|s| s.cursor.char_range())
                                            .map(|r| r.primary.index)
                                            .unwrap_or_else(|| eframe::egui::text::CharIndex(self.current_note.chars().count()));
                                        self.shots.request_clipboard(
                                            ui.ctx(),
                                            ShotTarget::Note {
                                                page: self.note_page,
                                                cursor: cursor.into(),
                                            },
                                        );
                                    }
                                }

                                if has_copy_or_cut {
                                    copied_or_cut_text = selected_slice;
                                }

                                if trigger_save {
                                    self.sync_and_save();
                                    self.toast_message = Some((
                                        "󰄬 Saved".to_string(),
                                        std::time::Instant::now(),
                                    ));
                                }
                            }

                            let text_col = rgb(self.theme.fg_primary);
                            let err_stroke = egui::Stroke::new(
                                1.3,
                                egui::Color32::from_rgb(240, 85, 85),
                            );
                            let checker_ref = &self.spellchecker;

                            // A finite desired width is essential here: INFINITY disables
                            // automatic wrapping and causes the editor to be clipped when the
                            // resizable panel becomes narrower.
                            let editor_width = ui.available_width().max(1.0);

                            let mut layouter =
                                |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap_width: f32| {
                                    let s = text.as_str();
                                    let mut job = egui::text::LayoutJob::default();
                                    job.wrap.max_width = wrap_width.max(1.0);
                                    // Match Preview's proportional body font so ordinary
                                    // paragraphs wrap at nearly the same points in both modes.
                                    let font_id = egui::FontId::proportional(note_font_sz);

                                    let mut last_end = 0;
                                    for (idx, ch) in s.char_indices() {
                                        if !ch.is_alphabetic() && ch != '\'' {
                                            if idx > last_end {
                                                let word = &s[last_end..idx];
                                                let ok = checker_ref.check_word(word);
                                                job.append(
                                                    word,
                                                    0.0,
                                                    egui::TextFormat {
                                                        font_id: font_id.clone(),
                                                        color: text_col,
                                                        underline: if ok {
                                                            egui::Stroke::NONE
                                                        } else {
                                                            err_stroke
                                                        },
                                                        ..Default::default()
                                                    },
                                                );
                                            }
                                            let ch_end = idx + ch.len_utf8();
                                            job.append(
                                                &s[idx..ch_end],
                                                0.0,
                                                egui::TextFormat {
                                                    font_id: font_id.clone(),
                                                    color: text_col,
                                                    ..Default::default()
                                                },
                                            );
                                            last_end = ch_end;
                                        }
                                    }
                                    if last_end < s.len() {
                                        let word = &s[last_end..];
                                        let ok = checker_ref.check_word(word);
                                        job.append(
                                            word,
                                            0.0,
                                            egui::TextFormat {
                                                font_id,
                                                color: text_col,
                                                underline: if ok {
                                                    egui::Stroke::NONE
                                                } else {
                                                    err_stroke
                                                },
                                                ..Default::default()
                                            },
                                        );
                                    }
                                    ui.fonts_mut(|f| f.layout_job(job))
                                };

                            let text_edit = egui::TextEdit::multiline(&mut self.current_note)
                                .id(text_edit_id)
                                .desired_width(editor_width)
                                .desired_rows(4)
                                .frame(egui::Frame::NONE)
                                .margin(egui::Margin::ZERO)
                                .layouter(&mut layouter)
                                .font(egui::FontId::proportional(note_font_sz))
                                .lock_focus(true)
                                .hint_text("Write your Markdown & $LaTeX$ notes here...");

                            let output = text_edit.show(ui);

                            // Spazio extra sotto il TextEdit per mantenere lo scrolloff anche sull'ultima riga
                            ui.add_space(edit_bottom_pad);

                            if output.response.changed() {
                                self.note_is_dirty = true;
                                self.sync_and_save();
                            }

                            // Scrolloff: mantiene ~2.5 righe visibili sopra e sotto il cursore
                            let nav_or_edit = output.response.changed()
                                || ui.input(|i| {
                                    i.key_pressed(egui::Key::ArrowDown)
                                        || i.key_pressed(egui::Key::ArrowUp)
                                        || i.key_pressed(egui::Key::ArrowLeft)
                                        || i.key_pressed(egui::Key::ArrowRight)
                                        || i.key_pressed(egui::Key::Enter)
                                        || i.key_pressed(egui::Key::PageDown)
                                        || i.key_pressed(egui::Key::PageUp)
                                });

                            if output.response.has_focus() && nav_or_edit {
                                if let Some(range) = output.cursor_range {
                                    let local_caret = output.galley.pos_from_cursor(range.primary);
                                    let row_h = local_caret.height().max(note_font_sz * 1.35);
                                    let caret_screen = local_caret.translate(output.galley_pos.to_vec2());

                                    let margin_y = row_h * 2.5;
                                    let padded_rect = egui::Rect::from_min_max(
                                        egui::pos2(caret_screen.min.x, caret_screen.min.y - margin_y),
                                        egui::pos2(caret_screen.max.x, caret_screen.max.y + margin_y),
                                    );
                                    let clip_bot = ui.clip_rect().bottom();
                                    if padded_rect.max.y > clip_bot {
                                        pending_scroll_px = padded_rect.max.y - clip_bot;
                                    }
                                    ui.scroll_to_rect(padded_rect, None);
                                }
                            }

                            let ctx_id = ui.make_persistent_id("spell_ctx_word");

                            if output.response.secondary_clicked() {
                                let picked: Option<(usize, usize, String)> = ui
                                    .input(|i| i.pointer.interact_pos())
                                    .and_then(|p| {
                                        let rel = p - output.galley_pos;
                                        let idx = output.galley.cursor_from_pos(rel).index;
                                        word_at_char_index(&self.current_note, idx.into())
                                    })
                                    .filter(|(_, _, w)| !self.spellchecker.check_word(w));

                                ui.data_mut(|d| match &picked {
                                    Some(v) => {
                                        d.insert_temp(ctx_id, v.clone());
                                    }
                                    None => {
                                        d.remove::<(usize, usize, String)>(ctx_id);
                                    }
                                });

                                if let Some((s, e, _)) = &picked {
                                    let mut st = output.state.clone();
                                    st.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                                        egui::text::CCursor::new(*s),
                                        egui::text::CCursor::new(*e),
                                    )));
                                    st.store(ui.ctx(), text_edit_id);
                                }
                            }

                            let ctx_word: Option<(usize, usize, String)> =
                                ui.data(|d| d.get_temp(ctx_id));
                            let mut spell_action: Option<SpellAction> = None;

                            if let Some((_, _, word)) = &ctx_word {
                                output.response.context_menu(|ui| {
                                    ui.set_min_width(170.0);
                                    ui.label(
                                        egui::RichText::new(format!("“{}”", word)).weak().size(12.0),
                                    );
                                    ui.separator();

                                    let cache_id = ui.make_persistent_id("spell_sugg_cache");
                                    let cached: Option<(String, Vec<String>)> =
                                        ui.data(|d| d.get_temp(cache_id));
                                    let sugg = match cached {
                                        Some((w, v)) if w == *word => v,
                                        _ => {
                                            let v = self.spellchecker.suggest(word, 5);
                                            ui.data_mut(|d| {
                                                d.insert_temp(cache_id, (word.clone(), v.clone()))
                                            });
                                            v
                                        }
                                    };

                                    if sugg.is_empty() {
                                        ui.weak("No suggestions");
                                    }
                                    for s in &sugg {
                                        if ui.button(egui::RichText::new(s).strong()).clicked() {
                                            spell_action = Some(SpellAction::Replace(s.clone()));
                                            ui.close();
                                        }
                                    }

                                    ui.separator();
                                    if ui.button("󰐕  Add to dictionary").clicked() {
                                        spell_action = Some(SpellAction::AddToDict);
                                        ui.close();
                                    }
                                    if ui.button("󰈉  Ignore (this session)").clicked() {
                                        spell_action = Some(SpellAction::Ignore);
                                        ui.close();
                                    }
                                });
                            }

                            if let (Some(act), Some((s, e, word))) = (spell_action, ctx_word) {
                                match act {
                                    SpellAction::Replace(new_word) => {
                                        replace_char_range(&mut self.current_note, s, e, &new_word);
                                        self.note_is_dirty = true;
                                        self.sync_and_save();

                                        let mut st = egui::text_edit::TextEditState::load(
                                            ui.ctx(),
                                            text_edit_id,
                                        )
                                        .unwrap_or_default();
                                        st.cursor.set_char_range(Some(
                                            egui::text::CCursorRange::one(
                                                egui::text::CCursor::new(
                                                    s + new_word.chars().count(),
                                                ),
                                            ),
                                        ));
                                        st.store(ui.ctx(), text_edit_id);
                                    }
                                    SpellAction::AddToDict => {
                                        self.spellchecker.add_user_word(&word)
                                    }
                                    SpellAction::Ignore => self.spellchecker.ignore_word(&word),
                                }
                                ui.data_mut(|d| d.remove::<(usize, usize, String)>(ctx_id));
                                ui.ctx().memory_mut(|m| m.request_focus(text_edit_id));
                                ui.ctx().request_repaint();
                            }

                            if let Some(text) = copied_or_cut_text {
                                self.clipboard.set_text(&text);
                            }
                        }

                        // Handled before the scroll area (the sheet has its own scrolling).
                        NoteMode::Sketch => {}

                        NoteMode::Preview => {
                            if self.current_note.trim().is_empty() {
                                ui.vertical_centered(|ui| {
                                    ui.add_space(40.0);
                                    ui.label(
                                        egui::RichText::new("󰦨")
                                            .size(28.0)
                                            .color(rgb(self.theme.fg_muted)),
                                    );
                                    ui.add_space(6.0);
                                    ui.weak(
                                        "No notes for this page. Switch to Edit to start writing.",
                                    );
                                });
                            } else {
                                // Reflow Markdown and scale embedded images to the live panel
                                // width so they do not retain a wider layout while resizing.
                                let preview_width = ui.available_width().max(1.0);
                                ui.take_available_width();
                                ui.scope(|ui| {
                                    let style = ui.style_mut();
                                    style.visuals.override_text_color = None;
                                    style.visuals.widgets.noninteractive.fg_stroke =
                                        egui::Stroke::new(1.0, rgb(self.theme.fg_primary));
                                    style.visuals.widgets.active.fg_stroke =
                                        egui::Stroke::new(2.0, rgb(self.theme.accent));
                                    style.visuals.hyperlink_color = rgb(self.theme.accent);
                                    style.visuals.code_bg_color = rgb(self.theme.bg_widget);
                                    style.visuals.extreme_bg_color = rgb(self.theme.bg_canvas);
                                    style.spacing.item_spacing.y = 7.0;
                                    style.interaction.selectable_labels = true;

                                    style.text_styles.insert(
                                        egui::TextStyle::Body,
                                        egui::FontId::proportional(note_font_sz),
                                    );
                                    style.text_styles.insert(
                                        egui::TextStyle::Monospace,
                                        egui::FontId::monospace(note_font_sz * 0.92),
                                    );
                                    style.text_styles.insert(
                                        egui::TextStyle::Heading,
                                        egui::FontId::proportional(note_font_sz * 1.35),
                                    );

                                    let pre_defs = notes::preprocess_definitions(&self.current_note);
                                    let rendered_md = notes::preprocess_markdown_view(&pre_defs);

                                    egui_commonmark::CommonMarkViewer::new()
                                        .max_image_width(Some(preview_width.ceil() as usize))
                                        .show(
                                            ui,
                                            &mut self.commonmark_cache,
                                            &rendered_md,
                                        );

                                    ui.add_space(28.0);
                                });

                                let copied_texts = ui.ctx().output_mut(|output| {
                                    let mut copied = Vec::new();
                                    output.commands.retain(|command| {
                                        if let egui::OutputCommand::CopyText(text) = command {
                                            copied.push(text.clone());
                                            false
                                        } else {
                                            true
                                        }
                                    });
                                    copied
                                });

                                for text in copied_texts {
                                    self.clipboard.set_text(&text);
                                }
                            }
                        }
                    });

                    ui.data_mut(|d| {
                        d.insert_temp(
                            scroll_id.with("last_offset"),
                            scroll_out.state.clamped_offset().y,
                        );
                    });

                    let viewport_rect = scroll_out.inner_rect;
                    let content_h = scroll_out.content_size.y;
                    let view_h = viewport_rect.height();
                    let offset_y = scroll_out.state.clamped_offset().y;

                    let extra_pad = if is_edit { edit_bottom_pad } else { 0.0 };
                    let extra_pad_audio = if self.app_state.settings.audio_dock || self.app_state.settings.audio_dock {Self::AUDIO_BTN_SIZE} else {0.0}; //add floating buttons size
                    let remaining_px = content_h - (offset_y + pending_scroll_px + view_h + extra_pad) + extra_pad_audio; 
                    if remaining_px > note_font_sz/2.0 {
                        let alpha = (remaining_px+note_font_sz / 35.0).clamp(0.25, 0.95);
                        let elevated = rgb(self.theme.bg_elevated);
                        let accent = rgb(self.theme.accent);
                        let foreground = rgb(self.theme.fg_primary);

                        let bg_col = egui::Color32::from_rgba_unmultiplied(
                            elevated.r(),
                            elevated.g(),
                            elevated.b(),
                            (alpha * 235.0) as u8,
                        );

                        let border_col = egui::Color32::from_rgba_unmultiplied(
                            accent.r(),
                            accent.g(),
                            accent.b(),
                            (alpha * 180.0) as u8,
                        );

                        let text_col = egui::Color32::from_rgba_unmultiplied(
                            foreground.r(),
                            foreground.g(),
                            foreground.b(),
                            (alpha * 255.0) as u8,
                        );

                        let fade_h = 24.0_f32.min(view_h * 0.25);
                        let fade_rect = egui::Rect::from_min_max(
                            egui::pos2(viewport_rect.left(), viewport_rect.bottom() - fade_h),
                            viewport_rect.right_bottom(),
                        );
                        let mut mesh = egui::Mesh::default();
                        let top_col = egui::Color32::TRANSPARENT;
                        let fade_base = if is_edit {
                            rgb(self.theme.bg_widget)
                        } else {
                            rgb(self.theme.bg_elevated)
                        };

                        let bot_col = egui::Color32::from_rgba_unmultiplied(
                            fade_base.r(),
                            fade_base.g(),
                            fade_base.b(),
                            (alpha * 200.0) as u8,
                        );
                        mesh.colored_vertex(fade_rect.left_top(), top_col);
                        mesh.colored_vertex(fade_rect.right_top(), top_col);
                        mesh.colored_vertex(fade_rect.right_bottom(), bot_col);
                        mesh.colored_vertex(fade_rect.left_bottom(), bot_col);
                        mesh.add_triangle(0, 1, 2);
                        mesh.add_triangle(0, 2, 3);
                        ui.painter().add(egui::Shape::Mesh(std::sync::Arc::new(mesh)));

                        let pill_size = egui::vec2(34.0, 22.0);
                        let pill_center = egui::pos2(
                            viewport_rect.center().x,
                            viewport_rect.bottom() - 15.0,
                        );
                        let pill_rect = egui::Rect::from_center_size(pill_center, pill_size);

                        let resp = ui.interact(
                            pill_rect,
                            scroll_id.with("more_below_btn"),
                            egui::Sense::click(),
                        );

                        let pill_bg = if resp.hovered() {
                            let active = rgb(self.theme.bg_active);
                            egui::Color32::from_rgba_unmultiplied(
                                active.r(),
                                active.g(),
                                active.b(),
                                245,
                            )
                        } else {
                            bg_col
                        };

                        ui.painter().rect(
                            pill_rect,
                            egui::CornerRadius::same(11),
                            pill_bg,
                            egui::Stroke::new(1.0, border_col),
                            egui::epaint::StrokeKind::Inside,
                        );

                        ui.painter().text(
                            pill_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "󰁅",
                            egui::FontId::proportional(14.0),
                            text_col,
                        );

                        if resp.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }

                        if resp.clicked() {
                            let target = (offset_y + view_h * 0.7).min(content_h - view_h);
                            ui.data_mut(|d| d.insert_temp(anim_scroll_id, target));
                            ui.ctx().request_repaint();
                        }
                    }
                });
        }

        // ---------------------------------------------------------
        // B. Bottom area: voice memos and lesson audio.
        // ---------------------------------------------------------
        if total_bottom_h > 0.0 {
            let mut bottom_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(bottom_rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            bottom_ui.set_clip_rect(bottom_rect);
            bottom_ui.spacing_mut().item_spacing.y = 0.0;

            // 1. Voice memos, when available.
            if has_page_memos {
                let memo_rect = egui::Rect::from_min_size(
                    bottom_rect.min,
                    egui::vec2(bottom_rect.width(), memo_h),
                );
                let mut memo_ui = bottom_ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(memo_rect)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                egui::Frame::NONE
                    .fill(panel_bg)
                    .stroke(egui::Stroke::NONE)
                    .inner_margin(egui::Margin::symmetric(8, 2))
                    .show(&mut memo_ui, |u| {
                        self.render_page_voice_memos_strip(u);
                    });
            }

            // 2. Audio Dock o Ticker
            if dock_h > 0.0 {
                let dock_top = bottom_rect.bottom() - dock_h;

                let dock_rect = egui::Rect::from_min_max(
                    egui::pos2(bottom_rect.left(), dock_top),
                    bottom_rect.max,
                );
                let mut dock_ui = bottom_ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(dock_rect)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                self.render_audio_dock_in_rect(&mut dock_ui, show_ticker);
            }
        }

        // ---------------------------------------------------------
        // C. Floating buttons when no voice memos are shown.
        // ---------------------------------------------------------
        if !has_page_memos {
            self.render_floating_audio_fabs(ui.ctx(), bottom_rect, &media_dir);
        }

        // ---------------------------------------------------------
        // D. Final panel UI allocation.
        // ---------------------------------------------------------
        // Panel::bottom saves `ui.min_rect()` to PanelState at frame end.
        // `new_child` does not move the parent cursor
        // or add item spacing, so expand min_rect to panel_rect.
        // This preserves the height selected by dragging
        // without automatic growth or reset to min_content_h.
        ui.allocate_rect(panel_rect, egui::Sense::hover());
    }

    fn render_slideshow_notes_contents(&mut self, ui: &mut egui::Ui, active_slide_note: String) {
        ui.take_available_width();
        egui::Frame::NONE
            .fill(rgb(self.theme.bg_elevated))
            .stroke(egui::Stroke::new(1.0, rgb(self.theme.border)))
            .corner_radius(egui::CornerRadius::same(self.theme.corner_radius.saturating_add(2)))
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("SLIDE NOTES")
                            .size(11.5)
                            .strong()
                            .color(rgb(self.theme.fg_muted)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let lock_icon = if self.lock_note_page { "󰌾" } else { "󰌿" };
                        let lock_button = egui::Button::new(
                            egui::RichText::new(lock_icon).size(11.5),
                        )
                        .selected(self.lock_note_page)
                        .fill(if self.lock_note_page {
                            rgb(self.theme.bg_active)
                        } else {
                            rgb(self.theme.bg_widget)
                        })
                        .stroke(egui::Stroke::new(
                            1.0,
                            if self.lock_note_page {
                                rgb(self.theme.accent)
                            } else {
                                rgb(self.theme.border)
                            },
                        ))
                        .corner_radius(egui::CornerRadius::same(
                            self.theme.corner_radius.saturating_sub(1),
                        ));
                        if ui
                            .add_sized([28.0, 27.0], lock_button)
                            .on_hover_text("Pin or unpin the current note while browsing slides")
                            .clicked()
                        {
                            self.lock_note_page = !self.lock_note_page;
                        }
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(format!(
                                "{} / {}",
                                self.current_page, self.total_pages
                            ))
                            .size(12.5)
                            .strong()
                            .color(rgb(self.theme.fg_primary)),
                        );
                    });
                });
            });
        ui.add_space(8.0);

                    // Case 1: pinned note.
                    if self.lock_note_page && !self.current_note.trim().is_empty() {
                        let note_clone = self.current_note.clone();
                        let pinned_p = self.note_page;
                        let accent_col = rgb(self.theme.accent);
                        self.render_markdown_preview_with_arrow(ui, "slideshow_notes_scroll", &note_clone, |ui| {
                            ui.label(egui::RichText::new(format!("󰌾 Pinned Note from Slide {}", pinned_p)).size(12.0).color(accent_col));
                            ui.add_space(6.0);
                        });
                        return;
                    }

                    // Case 2: the current slide has a Markdown note.
                    if !active_slide_note.is_empty() {
                        self.render_markdown_preview_with_arrow(ui, "slideshow_notes_scroll", &active_slide_note, |_| {});
                        return;
                    }

                            // Case 3: handle an empty slide using `SlideshowEmptyMode`.
                            match self.app_state.settings.slideshow_empty_mode {
                                SlideshowEmptyMode::AutoHold => {
                                if let Some((prev_p, prev_note)) = self.find_previous_note(self.current_page) {
                                    let bg_elev = rgb(self.theme.bg_elevated);
                                    let accent_col = rgb(self.theme.accent);
                                    self.render_markdown_preview_with_arrow(ui, "slideshow_notes_scroll", &prev_note, |ui| {
                                        egui::Frame::NONE
                                            .fill(bg_elev)
                                            .corner_radius(egui::CornerRadius::same(6))
                                            .inner_margin(egui::Margin::symmetric(10, 6))
                                            .show(ui, |ui| {
                                                ui.label(
                                                    egui::RichText::new(format!("󰌾 Auto-holding notes from Slide {} (current slide has no notes)", prev_p))
                                                        .size(11.5)
                                                        .color(accent_col),
                                                );
                                            });
                                        ui.add_space(8.0);
                                    });
                                } else {
                                    ui.weak("No previous notes to hold yet.");
                                }
                            }

                                SlideshowEmptyMode::ContextDashboard => {
                                    // 3B. Show audio transcript for this slide + Up Next preview
                                    let slide_segs: Vec<_> = self
                                        .audio_index
                                        .segments
                                        .iter()
                                        .filter(|s| s.page == self.current_page)
                                        .cloned()
                                        .collect();

                                    if !slide_segs.is_empty() {
                                        ui.label(
                                            egui::RichText::new("󰗊 Lecture Audio Transcript")
                                                .strong()
                                                .size(13.5)
                                                .color(rgb(self.theme.accent)),
                                        );
                                        ui.add_space(4.0);
                                        for seg in &slide_segs {
                                            let lbl = format!(
                                                "[{}]  {}",
                                                audio::format_ms(seg.start_ms),
                                                seg.text
                                            );
                                            if ui
                                                .add(
                                                    egui::Button::new(
                                                        egui::RichText::new(lbl).size(12.0),
                                                    )
                                                    .frame(false),
                                                )
                                                .clicked()
                                            {
                                                self.audio_engine.seek_to_ms(seg.start_ms);
                                            }
                                        }
                                        ui.add_space(12.0);
                                        ui.separator();
                                    }

                                    if let Some((next_p, next_note)) =
                                        self.find_next_note(self.current_page)
                                    {
                                        ui.add_space(6.0);
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "󰒭 Up Next: Notes on Slide {}",
                                                next_p
                                            ))
                                            .strong()
                                            .size(13.0)
                                            .color(rgb(self.theme.fg_muted)),
                                        );
                                        ui.add_space(4.0);
                                        let preview_lines: String = next_note
                                            .lines()
                                            .take(6)
                                            .collect::<Vec<_>>()
                                            .join("\n");
                                        let rendered =
                                            notes::preprocess_markdown_view(&preview_lines);
                                        egui_commonmark::CommonMarkViewer::new().show(
                                            ui,
                                            &mut self.commonmark_cache,
                                            &rendered,
                                        );
                                    } else if slide_segs.is_empty() {
                                        ui.weak("No notes or audio transcript for this slide.");
                                    }
                                }

                                SlideshowEmptyMode::SkipEmpty
                                | SlideshowEmptyMode::StaticEmpty
                                | SlideshowEmptyMode::ExpandFull => {
                                    ui.weak("No notes for this slide.");
                                }
                            }
                            ui.allocate_space(ui.available_size());
                }
}

impl eframe::App for Lapis {
    fn on_exit(&mut self) {
        self.sync_and_save();
        self.save_session_state();    
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let win_rect = ui.ctx().content_rect();
        let use_bottom_notes = self.app_state.settings.bottom_notes && win_rect.height() > win_rect.width();
        if let Some((kind, result)) = self.dialogs.poll() {
            let dialog_ctx = ui.ctx().clone();
            self.handle_dialog_result(kind, result, &dialog_ctx);
        }
        if self.pending_delete_page.is_some() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.pending_delete_page = None;
        }
        let lesson_poll_pending = self.audio_engine.poll();
        let memo_poll_pending = self.memo_audio_engine.poll();
        let audio_active = self.audio_engine.is_playing()
            || self.audio_engine.is_recording()
            || lesson_poll_pending
            || self.memo_audio_engine.is_playing()
            || memo_poll_pending;
        if audio_active {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));

            if self.audio_engine.is_playing() && self.audio_engine.follow_pdf {
                let pos = self.audio_engine.current_pos_ms();
                if let Some(synced_page) = self.audio_index.page_at_time(pos) {
                    if synced_page != self.current_page
                        && (1..=self.total_pages).contains(&synced_page)
                    {
                        self.go_to_page(synced_page, ui.ctx());
                    }
                }
            }
        }
        if let Ok(result) = self.export_rx.try_recv() {
            self.is_exporting = false;
            match result {
                Ok(path) => {
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    self.toast_message = Some((
                        format!("󰄬 Exported: {}", name),
                        std::time::Instant::now(),
                    ));
                }
                Err(err) => {
                    self.toast_message = Some((
                        format!("󰅚 Export error: {}", err),
                        std::time::Instant::now(),
                    ));
                }
            }
        }
        while let Ok(msg) = self.asr_rx.try_recv() {
            match msg {
                audio::AsrJobResult::Lecture { segments, done_ms, total_ms, finished } => {
                    let n = segments.len();
                    self.audio_index.segments = segments;
                    if self.audio_index.timeline_is_live { self.audio_index.assign_pages_from_timeline(); }
                    if finished {
                        self.is_transcribing = false;
                        self.asr_progress = None;
                        if !self.audio_index.timeline_is_live { self.transcript_all = true; }
                        self.audio_index.save(&self.pdf_path);
                        self.audio_toast(format!("󰄬 Transcribed {n} segments. Use Align to map them to slides"));
                    } else {
                        self.asr_progress = Some((done_ms, total_ms));
                    }
                }
                audio::AsrJobResult::Aligned { segments, timeline, range } => {
                    self.is_transcribing = false;
                    let n = segments.len();
                    self.audio_index.segments = segments;
                    self.audio_index.timeline = timeline;
                    self.audio_index.timeline_is_live = false;
                    self.audio_index.page_range = Some(range);
                    self.transcript_all = false;
                    self.audio_index.save(&self.pdf_path);
                    self.audio_toast(format!("󰄬 Aligned {n} segments to pages {}–{}", range.0, range.1));
                }
                audio::AsrJobResult::Memo {
                    page,
                    memo_idx,
                    text,
                } => {
                    self.is_transcribing = false;
                    if let Some(memo) = self
                        .audio_index
                        .page_memos
                        .get_mut(&page)
                        .and_then(|list| list.get_mut(memo_idx))
                    {
                        memo.transcript = Some(text);
                        self.audio_index.save(&self.pdf_path);
                    }
                    self.audio_toast("󰄬 Voice memo transcribed!");
                }
                audio::AsrJobResult::Cancelled => {
                    self.is_transcribing = false;
                    self.asr_progress = None;
                    self.audio_index.save(&self.pdf_path);
                    self.audio_toast("󰅖 Transcription cancelled (partial transcript kept)");
                }
                audio::AsrJobResult::Error(err) => {
                    self.is_transcribing = false;
                    self.asr_progress = None;
                    self.audio_toast(format!("󰅚 Transcription: {}", err));
                }
            }
        }

        // Upload at most 2 textures per frame so a burst of results never stalls the UI.
        let mut uploads = 0;
        while uploads < 2 {
            let Ok(msg) = self.render_rx.try_recv() else {
                break;
            };
            let result = match msg {
                WorkerMsg::Text { page_idx, text } => {
                    self.page_text.insert((page_idx + 1) as u32, text);
                    continue; // no GPU upload: does not count against the per-frame budget
                }
                WorkerMsg::Page(r) => r,
            };
            uploads += 1;
            let page_num = (result.page_idx + 1) as u32;
            let (w, h) = (result.image.size[0] as u32, result.image.size[1] as u32);
            self.page_links.insert(page_num, result.links);

            if result.is_thumb {
                let handle = ui.ctx().load_texture(
                    format!("pdf_thumb_{}", page_num),
                    result.image,
                    egui::TextureOptions::LINEAR,
                );
                self.thumb_cache.insert(page_num, (handle, (w, h)));
                continue;
            }

            let handle = ui.ctx().load_texture(
                format!("pdf_page_{}", page_num),
                result.image,
                egui::TextureOptions::LINEAR,
            );
            self.page_cache
                .insert(page_num, (handle.clone(), (w, h)));
            self.page_scale.insert(page_num, result.scale_key * 2 + (result.dark_mode as u32));

            self.touch_cache(page_num);
            self.evict_cache();

            if page_num == self.current_page {
                self.texture_handle = Some(handle);
                self.page_dims = (w, h);
                self.page_pt = result.page_pt;
                self.is_rendering = false;
            }
        }
        if uploads >= 2 {
            ui.ctx().request_repaint(); // more results may be waiting
        }

        let sel_ctx = ui.ctx().clone();
        self.update_text_drag(&sel_ctx);

        // Global Hotkeys & Zoom handling (processed before widgets so they work reliably)
        let text_edit_id = ui.make_persistent_id("note_text_edit");
        let any_focused = ui.ctx().memory(|m| m.focused().is_some());
        let note_editor_focused = ui.ctx().memory(|m| m.has_focus(text_edit_id));

        // Leaving the Sketch tab (or hiding the notes) finishes an open text box.
        if !self.sketch_enabled() && self.note_mode == NoteMode::Sketch {
            self.note_mode = NoteMode::Edit;
        }
        let sketch_visible = self.show_notes && self.note_mode == NoteMode::Sketch;
        if !sketch_visible {
            self.sketch.hovered = false;
            self.finish_sketch_edit();
        }
        // Pinch / Ctrl + wheel over the sheet zooms the sheet, not the PDF.
        let sketch_hover = sketch_visible && self.sketch.hovered;

        let mut act_prev_page = false;
        let mut act_next_page = false;
        let mut act_save = false;
        let mut act_toggle_notes = false;
        let mut act_toggle_mode = false;
        let mut act_toggle_sketch = false;
        let mut act_toggle_toc = false;
        let mut act_toggle_fullscreen = false;
        let mut copy_triggered = false;

        if self.rebinding_action.is_none() {
        ui.input_mut(|i| {
            // 1. Trackpad pinch or Ctrl + wheel zoom
            let z_delta = i.zoom_delta();
            if sketch_hover {
                // Handled by the sketch sheet.
            } else if (z_delta - 1.0).abs() > 1e-4 {
                self.enter_custom_zoom();
                let old = self.zoom_scale;
                self.zoom_scale = (self.zoom_scale * z_delta).clamp(0.3, 3.0);
                if (self.zoom_scale - old).abs() > 1e-4 {
                    self.zoom_anchor = i.pointer.hover_pos().or_else(|| i.pointer.latest_pos());
                }
                i.smooth_scroll_delta = egui::Vec2::ZERO;
            } else if (i.modifiers.ctrl || i.modifiers.command)
                && i.smooth_scroll_delta.y.abs() > 0.0
            {
                self.enter_custom_zoom();
                let old = self.zoom_scale;
                let step = (i.smooth_scroll_delta.y * 0.002).clamp(-0.15, 0.15);
                self.zoom_scale = (self.zoom_scale + step).clamp(0.3, 3.0);
                if (self.zoom_scale - old).abs() > 1e-4 {
                    self.zoom_anchor = i.pointer.hover_pos().or_else(|| i.pointer.latest_pos());
                }
                i.smooth_scroll_delta = egui::Vec2::ZERO;
            }

            // 2. Ctrl + Plus / Minus / 0 for PDF Zoom
            if i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::Plus,
            )) || i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::Equals,
            )) {
                self.enter_custom_zoom();
                self.zoom_scale = (self.zoom_scale + 0.1).min(3.0);
            }
            if i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::Minus,
            )) {
                self.enter_custom_zoom();
                self.zoom_scale = (self.zoom_scale - 0.1).max(0.3);
            }
            if i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::Num0,
            )) {
                self.zoom_scale = 1.0;
                self.zoom_mode = ZoomMode::Custom;
            }

            // 3. Global Ctrl shortcuts (Ctrl+S, Ctrl+E, Ctrl+N, Ctrl+T)
            let sc = &self.app_state.shortcuts;
            if sc.save_notes.consume(i) {
                    act_save = true;
                }
            if sc.toggle_edit_preview.consume(i) {
                    act_toggle_mode = true;
                }
            if sc.toggle_notes.consume(i) {
                    act_toggle_notes = true;
                }
            if sc.toggle_sketch.consume(i) {
                    act_toggle_sketch = true;
                }
            if sc.toggle_outline.consume(i) {
                    act_toggle_toc = true;
                }
            // 4. Slide navigation, including while editing notes.
            //    Alt + Left/Right, Alt + K/J, Ctrl + PageUp/PageDown, or PageUp/PageDown
            if sc.prev_slide_global.consume(i)
                    || i.consume_shortcut(&egui::KeyboardShortcut::new(
                        egui::Modifiers::NONE,
                        egui::Key::PageUp,
                    ))
                {
                    act_prev_page = true;
                }
                if sc.next_slide_global.consume(i)
                    || i.consume_shortcut(&egui::KeyboardShortcut::new(
                        egui::Modifiers::NONE,
                        egui::Key::PageDown,
                    ))
                {
                    act_next_page = true;
                }

            // 5. Standard Left/Right or K/J when not typing in a text box and annotation tools
            if !any_focused {
                if i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::K) {
                    act_prev_page = true;
                }
                if i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::J) {      
                    act_next_page = true;
                }
                if self.app_state.settings.radial_menu_enabled
                    && self.app_state.shortcuts.radial_tools_menu.consume(i)
                {
                    self.radial_menu_pos = if self.radial_menu_pos.is_some() {
                        None
                    } else {
                        i.pointer.hover_pos().or_else(|| Some(i.content_rect().center()))
                    };
                }
                            }
            // Fullscreen: F11 always works (also while typing notes); a rebinding made of a
            // plain key only works when no text field has the focus.
                {
                    let b = &self.app_state.shortcuts.toggle_fullscreen;
                    let plain = !b.ctrl && !b.alt && !b.shift && b.extra_key.is_none();
                    let by_binding = (!plain || !any_focused) && b.consume(i);
                    let by_f11 = i.consume_key(egui::Modifiers::NONE, egui::Key::F11);
                    if by_binding || by_f11 {
                        act_toggle_fullscreen = true;
                    }
                }
            // 6. View Mode Shortcuts: Alt+1 (Single), Alt+2 (Continuous), Alt+3 (TwoPage), Alt+4 (Slideshow), Alt+4 (Slideshow+Notes), Esc (Exit Slideshow)
                if sc.view_single.consume(i) {
                    self.view_mode = ViewMode::SinglePage;
                }
                if sc.view_continuous.consume(i) {
                    self.view_mode = ViewMode::Continuous;
                    self.continuous_scroll_to_page = Some(self.current_page);
                }
                if sc.view_twopage.consume(i) {
                    self.view_mode = ViewMode::TwoPage;
                }
                if sc.view_slideshow.consume(i) {
                    if self.view_mode != ViewMode::Slideshow && self.view_mode != ViewMode::SlideshowWithNotes {
                        self.prev_view_mode = self.view_mode;
                    }
                    self.view_mode = ViewMode::Slideshow;
                }
                if sc.view_slideshow_notes.consume(i) {
                    if self.view_mode != ViewMode::Slideshow && self.view_mode != ViewMode::SlideshowWithNotes {
                        self.prev_view_mode = self.view_mode;
                    }
                    self.view_mode = ViewMode::SlideshowWithNotes;
                }
                
                if self.view_mode == ViewMode::Slideshow || self.view_mode == ViewMode::SlideshowWithNotes {
                    if i.key_pressed(egui::Key::Escape) {
                        self.view_mode = self.prev_view_mode;
                    }
                }
            // Toggle Continuous <-> Single page (default: C). Plain key: only when not typing,
            // and never while a Ctrl/Alt chord (e.g. Ctrl+C copy) is held.
            let in_show = matches!(self.view_mode, ViewMode::Slideshow | ViewMode::SlideshowWithNotes);
            if !any_focused && !in_show && self.has_pdf() {
                let b = &self.app_state.shortcuts.toggle_continuous_single;
                let chord_free = b.ctrl || !i.modifiers.command;
                if chord_free && b.consume(i) {
                    if self.view_mode == ViewMode::Continuous {
                        self.view_mode = ViewMode::SinglePage;
                    } else {
                        self.view_mode = ViewMode::Continuous;
                        self.continuous_scroll_to_page = Some(self.current_page);
                    }
                }
            }
            // 7. Continuous Viewmode arrows shortcuts
            if !any_focused && self.view_mode == ViewMode::Continuous {
             if i.key_pressed(egui::Key::ArrowDown) {
                 self.continuous_scroll_delta -= 120.0;
             }
             if i.key_pressed(egui::Key::ArrowUp) {
                 self.continuous_scroll_delta += 120.0;
             }
         }
         // Copy selected PDF text when the note editor is not focused.
        if !note_editor_focused {
            let has_copy_ev = i.events.iter().any(|e| matches!(e, egui::Event::Copy));
            if has_copy_ev || i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::C)) {
                copy_triggered = true;
            }
        }
        // 9. CTRL+Z for tools annotations
        let mut do_undo = false;
        let mut do_redo = false;
        if !any_focused && self.rebinding_action.is_none() {
            let is_cmd = i.modifiers.command;
            if is_cmd && i.key_pressed(egui::Key::Z) {
                if i.modifiers.shift {
                    do_redo = true;
                } else {
                    do_undo = true;
                }
            } else if is_cmd && i.key_pressed(egui::Key::Y) {
                do_redo = true;
            }
            }

        if do_undo {
            self.undo_annotation(self.current_page);
        } else if do_redo {
            self.redo_annotation(self.current_page);
        }
         //10. Search
         if self.app_state.shortcuts.search_notes.consume(i) {
            self.show_search = !self.show_search;
            if self.show_search {
                self.focus_search_input = true;
            }
        }

        //11. Command palette
        //11. Command palette
        let toggle_cmd_palette = i.modifiers.command && (i.key_pressed(egui::Key::P) || i.key_pressed(egui::Key::K));
        if toggle_cmd_palette {
            self.show_command_palette = !self.show_command_palette;
            if self.show_command_palette {
                self.command_palette_query.clear();
                self.command_palette_selected = 0;
            }
        }

        });
        }
        if copy_triggered {
            if let Some(text) = self.selection_text() {
                self.clipboard.set_text(&text);
            }
        }

        if act_save {
            self.sync_and_save();
            self.toast_message = Some(("󰄬 Saved".to_string(), std::time::Instant::now()));
        }
        if act_toggle_toc {
            self.show_toc = !self.show_toc;
        }
        if act_toggle_notes {
            self.show_notes = !self.show_notes;
            if self.show_notes {
                self.note_mode = NoteMode::Edit;
                self.focus_note_editor = true;
            }
        }
        if act_toggle_sketch {
            self.toggle_sketch_tab();
        }
        if act_toggle_mode && self.show_notes {
            self.note_mode = match self.note_mode {
                NoteMode::Edit => NoteMode::Preview,
                NoteMode::Preview | NoteMode::Sketch => NoteMode::Edit,
            };
            if self.note_mode == NoteMode::Edit {
                self.focus_note_editor = true;
            }
        }
        if act_prev_page {
            self.prev_page(ui.ctx());
            if note_editor_focused {
                self.focus_note_editor = true;
            }
        }
        if act_next_page {
            self.next_page(ui.ctx());
            if note_editor_focused {
                self.focus_note_editor = true;
            }
        }
        

        // Top Panel
        let in_slideshow = matches!(
        self.view_mode,
        ViewMode::Slideshow | ViewMode::SlideshowWithNotes
        );
        let shot_ctx = ui.ctx().clone();
        self.process_screenshots(&shot_ctx, any_focused, in_slideshow);
        // Fullscreen is wanted in slideshow modes and whenever the user asked for it.
        // F11 does nothing in slideshow (Esc exits it); the user choice is kept for afterwards.
        if act_toggle_fullscreen && !in_slideshow {
            self.user_fullscreen = !self.user_fullscreen;
        }
        let wanted_fullscreen = in_slideshow || self.user_fullscreen;
        let actual_fullscreen = ui.input(|i| i.viewport().fullscreen);
        if self.fullscreen_sent != Some(wanted_fullscreen) {
            // Edge-triggered: send the command only when the wanted state changes.
            // Re-sending it every frame fights tiling compositors (e.g. Hyprland), which
            // report the new state asynchronously or let the user leave fullscreen themselves.
            if self.fullscreen_sent.is_some()
                || actual_fullscreen.unwrap_or(false) != wanted_fullscreen
            {
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Fullscreen(wanted_fullscreen));
                self.fullscreen_cmd_at = Some(std::time::Instant::now());
                ui.ctx().request_repaint();
            }
            self.fullscreen_sent = Some(wanted_fullscreen);
        } else if !in_slideshow {
            // Fullscreen changed from outside (compositor keybinding, window manager, ...):
            // follow the real state instead of forcing ours back.
            let settled = self
                .fullscreen_cmd_at
                .map_or(true, |t| t.elapsed() > std::time::Duration::from_millis(800));
            if let Some(actual) = actual_fullscreen {
                if settled && actual != self.user_fullscreen {
                    self.user_fullscreen = actual;
                    self.fullscreen_sent = Some(actual);
                }
            }
        }

        if !in_slideshow && self.has_pdf(){    
        egui::Panel::top("top_panel")
            .frame(
                egui::Frame::NONE
                    .fill(rgb(self.theme.bg_panel))
                    .inner_margin(egui::Margin::symmetric(8, 3)),
            )
            .show(ui, |ui| {
                const BAR_H: f32 = 27.0;
                let sq_btn = egui::vec2(BAR_H, BAR_H);


            let bar_rect = ui.max_rect();
                let bar_w = bar_rect.width();
                // Fixed-height toolbar row: left group, right group, and title.
                // are positioned independently at the row edges and center.
                let row_rect = egui::Rect::from_min_size(bar_rect.min, egui::vec2(bar_w, BAR_H));
 
                // Compactness stage (0 = expanded, 4 = minimal), based on the previous frame.
                let stage_id = ui.make_persistent_id("topbar_stage");
                let widths_id = ui.make_persistent_id("topbar_stage_widths");
                let stage = ui.data(|d| d.get_temp::<usize>(stage_id).unwrap_or(0)).min(3);
                let menu_id = ui.make_persistent_id("file_menu_open_state");
                let has_jump = !self.nav_back_stack.is_empty() || !self.nav_forward_stack.is_empty();
 
                let mut left_end_x = row_rect.left();
                let mut right_start_x = row_rect.right();
                let mut left_more_rect = egui::Rect::NOTHING;
                let mut bm_anchor = egui::Rect::NOTHING;
 
                // Left control group.
                ui.horizontal(|ui| {
                    ui.set_height(BAR_H);
 
                    // Outline toggle
                    let toc_arrow = if self.show_toc { "" } else { "" };
                    let toc_btn = egui::Button::new(format!(" {}", toc_arrow))
                        .selected(self.show_toc)
                        .min_size(egui::vec2(44.0, BAR_H));
                    if ui
                        .add(toc_btn)
                        .on_hover_text("Toggle Outline")
                        .clicked()
                    {
                        self.show_toc = !self.show_toc;
                    }
 
                    ui.separator();
 
                    // Zoom controls
                    if stage < 3 {
                        if ui
                            .add(egui::Button::new("󰍴").min_size(sq_btn))
                            .on_hover_text("Zoom Out [Ctrl+-]")
                            .clicked()
                        {
                            self.enter_custom_zoom();
                            self.zoom_scale = (self.zoom_scale - 0.1).max(0.3);
                        }
                    }
 
                    let zoom_label = match self.zoom_mode {
                        ZoomMode::FitWidth => "󰤼 Fit Width".to_string(),
                        ZoomMode::FitHeight => "󰤻 Fit Height".to_string(),
                        ZoomMode::FitPage => "󰊓 Fit Page".to_string(),
                        ZoomMode::Custom => format!("󰍉 {:.0}%", self.zoom_scale * 100.0),
                    };
 
                    let zoom_menu = ui.menu_button(zoom_label, |ui| {
                    ui.set_min_width(140.0);
                    if ui.button("Zoom In [Ctrl++]").clicked() {
                        self.enter_custom_zoom();
                        self.zoom_scale = (self.zoom_scale + 0.1).min(3.0);
                    }
                    if ui.button("Zoom Out [Ctrl+-]").clicked() {
                        self.enter_custom_zoom();
                        self.zoom_scale = (self.zoom_scale - 0.1).max(0.3);
                    }
                    ui.separator();
                    if ui
                        .selectable_label(self.zoom_mode == ZoomMode::FitWidth, "󰤼  Fit Width")
                        .clicked()
                    {
                        self.zoom_mode = ZoomMode::FitWidth;
                        ui.close();
                    }
                    if ui
                        .selectable_label(
                            self.zoom_mode == ZoomMode::FitHeight,
                            "󰤻  Fit Height",
                        )
                        .clicked()
                    {
                        self.zoom_mode = ZoomMode::FitHeight;
                        ui.close();
                    }
                    if ui
                        .selectable_label(self.zoom_mode == ZoomMode::FitPage, "󰊓  Fit Page")
                        .clicked()
                    {
                        self.zoom_mode = ZoomMode::FitPage;
                        ui.close();
                    }
                    ui.separator();
                    if ui
                        .selectable_label(
                            self.zoom_mode == ZoomMode::Custom
                                && (self.zoom_scale - 1.0).abs() < 0.01,
                            "󰑐  100% [Ctrl+0]",
                        )
                        .clicked()
                    {
                        self.zoom_scale = 1.0;
                        self.zoom_mode = ZoomMode::Custom;
                        ui.close();
                    }
                });

                let resp = zoom_menu.response.on_hover_text("Zoom Mode [Ctrl+Scroll] | Right Click: Fit Page");

                if resp.hovered() && ui.input(|i| i.pointer.button_clicked(egui::PointerButton::Secondary)) {
                    self.zoom_mode = ZoomMode::FitPage;
                }
 
                    if stage < 3 {
                        if ui
                            .add(egui::Button::new("󰐕").min_size(sq_btn))
                            .on_hover_text("Zoom In [Ctrl++]")
                            .clicked()
                        {
                            self.enter_custom_zoom();
                            self.zoom_scale = (self.zoom_scale + 0.1).min(3.0);
                        }
                    }
 
                    ui.separator();
 
                    if stage < 2 {
                        self.topbar_view_mode_menu(ui);
                        self.topbar_split_button(ui, sq_btn, false);
                        self.topbar_dark_button(ui, sq_btn, false);
                    } else {
                        let more = ui.menu_button("⋯", |ui| {
                            ui.set_min_width(200.0);
                            if stage >= 4 {
                                // At minimum width, the File menu moves here.
                                if ui
                                    .button("󰈦  File…")
                                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                                    .clicked()
                                {
                                    ui.data_mut(|d| {
                                        d.insert_temp(menu_id, true);
                                        d.insert_temp(ui.make_persistent_id("file_menu_skip_outside"), true);
                                    });
                                    ui.close();
                                }
                                ui.separator();
                            }
                            self.topbar_view_mode_menu(ui);
                            self.topbar_split_button(ui, sq_btn, true);
                            self.topbar_dark_button(ui, sq_btn, true);
                        });
                        left_more_rect = more.response.rect;
                        more.response.on_hover_text("More");
                    }
                    left_end_x = ui.min_rect().right();
                });
 
                // Right group: notes, tools, flashcards, pages, search, bookmarks, and new page.
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(row_rect)
                        .layout(egui::Layout::right_to_left(egui::Align::Center)),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        ui.spacing_mut().button_padding = egui::vec2(6.0, 3.0);

                        // Notes
                        let page_has_note = self.notes.get(&self.current_page).map(|s| !s.trim().is_empty()).unwrap_or(false) || !self.current_note.trim().is_empty();
 
                        let notes_arrow = if self.show_notes { "" } else { "" };
                        let notes_btn = egui::Button::new(format!("{} 󰠮 ", notes_arrow))
                            .selected(self.show_notes)
                            .min_size(egui::vec2(44.0, BAR_H));
 
                        let resp = ui.add(notes_btn);
 
                        if page_has_note {
                            let dot_center = egui::pos2(resp.rect.right_top().x - 2.0, resp.rect.right_top().y + 2.0);                            
                            let dot_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 255, 255));
 
                            ui.painter().circle(
                                dot_center,
                                3.5,
                                theme::rgb(self.theme.accent),
                                dot_stroke,
                            );
                        }
 
                        let has_notes_hover = if page_has_note {"\n(This page contains notes)"} else {""};
                        if resp.on_hover_text(format!("Toggle Notes Panel {}", has_notes_hover)).clicked() {
                            self.show_notes = !self.show_notes;
                            if self.show_notes && self.note_mode == NoteMode::Edit {
                                self.focus_note_editor = true;
                            }
                        }
                        ui.separator();
                        let tools_on = self.app_state.settings.tools_button;
                        let flash_on = self.app_state.settings.flash_button;
                        if stage < 4 {
                            self.topbar_tools_button(ui, sq_btn, false);
                        }
                        if stage < 3 {
                            self.topbar_flash_button(ui, sq_btn, false);
                        }
                        if (stage < 4 && tools_on) || (stage < 3 && flash_on) {
                            ui.separator();
                        }
 
                        if ui
                            .add(egui::Button::new("󰁔").min_size(sq_btn))
                            .on_hover_text("Next Page [→ / J / Alt+→]")
                            .clicked()
                        {
                            self.next_page(ui.ctx());
                        }
 
                        ui.label(
                            egui::RichText::new(format!("| {}", self.total_pages))
                                .color(ui.visuals().weak_text_color()),
                        );
 
                        let response = ui
                            .add_sized(
                                [42.0, BAR_H - 2.0],
                                egui::DragValue::new(&mut self.target_page)
                                    .range(1..=self.total_pages)
                                    .speed(0.1),
                            )
                            .on_hover_text("Current Page (Drag or click to type)");
 
                        if response.lost_focus() || (response.changed() && !response.has_focus()) {
                            if self.target_page != self.current_page {
                                self.go_to_page(self.target_page, ui.ctx());
                            }
                        }
 
                        if ui
                            .add(egui::Button::new("󰁍").min_size(sq_btn))
                            .on_hover_text("Previous Page [← / K / Alt+←]")
                            .clicked()
                        {
                            self.prev_page(ui.ctx());
                        }
                        if stage < 3 && has_jump {
                            self.topbar_jump_buttons(ui, sq_btn, false);
                        }
                        ui.separator();
 
                        if stage < 1 {
                            self.topbar_search_button(ui, sq_btn, false);
                            bm_anchor = self.topbar_bookmark_button(ui, sq_btn, false);
                            self.topbar_newpage_button(ui, sq_btn, false);
                        } else {
                            let more = ui.menu_button("⋯", |ui| {
                                ui.set_min_width(200.0);
                                self.topbar_search_button(ui, sq_btn, true);
                                self.topbar_bookmark_button(ui, sq_btn, true);
                                self.topbar_newpage_button(ui, sq_btn, true);
                                if stage >= 3 {
                                    self.topbar_flash_button(ui, sq_btn, true);
                                    if has_jump {
                                        self.topbar_jump_buttons(ui, sq_btn, true);
                                    }
                                }
                                if stage >= 4 {
                                    self.topbar_tools_button(ui, sq_btn, true);
                                }
                            });
                            bm_anchor = more.response.rect;
                            more.response.on_hover_text("More tools");
                        }
                        self.render_bookmark_popup(ui, bm_anchor);
                        right_start_x = ui.min_rect().left();
                    },
                );
 
                // Center the title and move File into the left overflow menu when needed.
                const TITLE_GAP: f32 = 8.0;
                const TITLE_COMFORT_CHARS: usize = 12;
                const TITLE_MAX_CHARS: usize = 60;
                const ALLOW_TITLE_SLIDE: bool = true; 
 
                let mut is_open = ui.data_mut(|d| *d.get_temp_mut_or_default::<bool>(menu_id));
                let arrow = if is_open { "▴" } else { "▾" };
 
                let font_id = egui::FontId::proportional(13.0);
                let make_label = |title: &str, arrow: &str| format!("  {}  {}", title, arrow);
                let label_w = |s: &str| -> f32 {
                    (ui.painter()
                        .layout_no_wrap(s.to_owned(), font_id.clone(), ui.visuals().text_color())
                        .size()
                        .x
                        + ui.spacing().button_padding.x * 2.5)
                        .ceil()
                };
                let title_for = |n: usize| -> String {
                    if self.pdf_name.is_empty() {
                        "[No PDF loaded]".to_string()
                    } else {
                        truncate_filename(&self.pdf_path, n)
                    }
                };
 
                // Measure both groups at the current compactness stage.
                let left_w = (left_end_x - row_rect.left()).max(0.0);
                let right_w = (row_rect.right() - right_start_x).max(0.0);
                let mut widths: Vec<f32> = ui
                    .data(|d| d.get_temp::<Vec<f32>>(widths_id))
                    .filter(|v| v.len() == 8)
                    .unwrap_or_else(|| vec![0.0; 8]);

                widths[stage] = left_w;
                widths[4 + stage] = right_w;
 
                // Estimate unmeasured stages from the current layout.
                const L_SAVE: [f32; 3] = [0.0, 160.0, 70.0]; // Left: stage s to s+1.
                const R_SAVE: [f32; 3] = [75.0, 0.0, 100.0]; // Right: stage s to s+1.
                let est = |s: usize| -> (f32, f32) {
                    if widths[s] > 0.0 && widths[4 + s] > 0.0 {
                        return (widths[s], widths[4 + s]);
                    }
                    let (mut l, mut r) = (left_w, right_w);
                    if s < stage {
                        for k in s..stage {
                            l += L_SAVE[k];
                            r += R_SAVE[k];
                        }
                    } else {
                        for k in stage..s {
                            l = (l - L_SAVE[k]).max(0.0);
                            r = (r - R_SAVE[k]).max(0.0);
                        }
                    }
                    (l, r)
                };
 
                let comfort_title_w = label_w(&make_label(&"n".repeat(TITLE_COMFORT_CHARS), "▾"));
                let need = |s: usize| -> f32 {
                    let (l, r) = est(s);

                    if ALLOW_TITLE_SLIDE {
                        l + r + comfort_title_w + 3.0 * TITLE_GAP
                    } else {
                        2.0 * l.max(r) + comfort_title_w + 2.0 * TITLE_GAP
                    }
                };

                let mut new_stage = stage;
                if stage < 3 && need(stage) > bar_w {
                    new_stage = stage + 1;
                } else if stage > 0 && need(stage - 1) + 24.0 <= bar_w {
                    new_stage = stage - 1; // 24 px hysteresis prevents layout oscillation.
                }
                ui.data_mut(|d| {
                    d.insert_temp(widths_id, widths.clone());
                    d.insert_temp(stage_id, new_stage);
                });
                if new_stage != stage {
                    ui.ctx().request_repaint();
                }
 
                // Use the longest title that fits; otherwise show only the icon.
                let center_x = row_rect.center().x;
                let sym_w = (2.0 * ((center_x - left_end_x).min(right_start_x - center_x) - TITLE_GAP)).max(0.0);
                let gap_w = (right_start_x - left_end_x - 2.0 * TITLE_GAP).max(0.0);
                let max_title_w = if ALLOW_TITLE_SLIDE { gap_w } else { sym_w };

                let chosen_title = (1..=TITLE_MAX_CHARS)
                    .rev()
                    .map(title_for)
                    .find(|t| label_w(&make_label(t.as_str(), "▾")) <= max_title_w)
                    .unwrap_or_default();
 
                let menu_label = make_label(&chosen_title, arrow);
                let measure_label = make_label(&chosen_title, "▾");
                let total_size = egui::vec2(label_w(&measure_label), BAR_H);
                let half = total_size.x / 2.0;
                let lo = left_end_x + TITLE_GAP + half;
                let hi = right_start_x - TITLE_GAP - half;
                let title_fits = total_size.x <= max_title_w;

                let title_cx = if ALLOW_TITLE_SLIDE && title_fits {
                    center_x.clamp(lo, hi)
                } else {
                    center_x
                };

                let centered_rect = egui::Rect::from_center_size(
                    egui::pos2(title_cx, row_rect.center().y),
                    total_size,
                );
 
                let mut button_rect = egui::Rect::NOTHING;

                if !title_fits {
                    button_rect = left_more_rect;
                } else {
                    ui.scope_builder(egui::UiBuilder::new().max_rect(centered_rect), |ui| {
                        let btn_resp = ui
                            .add_sized(
                                total_size,
                                egui::Button::new(egui::RichText::new(menu_label).font(font_id))
                                    .selected(is_open)
                                    .corner_radius(egui::CornerRadius::same(
                                        self.theme.corner_radius,
                                    )),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text(format!("File actions and settings\n{}", self.pdf_path.to_string_lossy()));

                        button_rect = btn_resp.rect;

                        if btn_resp.clicked() {
                            is_open = !is_open;
                        }
                    });
                }
 
                if is_open {
                    let mut close_requested = false;
                    let (popup_pos, popup_pivot) = if !title_fits {
                        (
                            egui::pos2(button_rect.left(), button_rect.bottom() + 6.0),
                            egui::Align2::LEFT_TOP,
                        )
                    } else {
                        (
                            egui::pos2(button_rect.center().x, button_rect.bottom() + 6.0),
                            egui::Align2::CENTER_TOP,
                        )
                    };
 
                    let area_resp = egui::Area::new(ui.make_persistent_id("file_menu_area"))
                        .fixed_pos(popup_pos)
                        .pivot(popup_pivot)
                        .order(egui::Order::Foreground)
                        .show(ui.ctx(), |ui| {
                            egui::Frame::menu(ui.style())
                                .inner_margin(egui::Margin::symmetric(6, 6))
                                .show(ui, |ui| {
                                    let menu_width = 232.0;
                                    let menu_text_muted = rgb(self.theme.fg_muted);
                                    let menu_radius = egui::CornerRadius::same(
                                        self.theme.corner_radius.saturating_sub(1),
                                    );
                                    ui.set_width(menu_width);
                                    ui.spacing_mut().item_spacing.y = 4.0;

                                    let menu_heading = |ui: &mut egui::Ui, label: &str| {
                                        ui.add_space(4.0);
                                        ui.label(
                                            egui::RichText::new(label)
                                                .size(10.0)
                                                .strong()
                                                .color(menu_text_muted),
                                        );
                                        ui.add_space(3.0);
                                    };
                                    let menu_item = |ui: &mut egui::Ui,
                                                     label: &str,
                                                     enabled: bool|
                                     -> egui::Response {
                                        let btn = egui::Button::new(
                                            egui::RichText::new(label).size(11.8),
                                        )
                                        .min_size(egui::vec2(menu_width, 26.0))
                                        .corner_radius(menu_radius);
                                        ui.add_enabled(enabled, btn)
                                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                                    };

                                    menu_heading(ui, "PREFERENCES");
                                    if menu_item(ui, "󰒓  Settings & customization...", true)
                                        .on_hover_text("Themes, export preferences and keyboard shortcuts")
                                        .clicked()
                                    {
                                        self.show_settings = true;
                                        self.rebinding_action = None;
                                        close_requested = true;
                                    }

                                    ui.separator();
                                    menu_heading(ui, "DOCUMENT");
                                    if menu_item(ui, "󰈦  Open PDF...", true).clicked() {
                                        self.dialogs.open(
                                            ui.ctx(),
                                            DialogKind::OpenPdf,
                                            DialogSpec::open_file().filter("PDF files", &["pdf"]),
                                        );
                                        close_requested = true;
                                    }

                                    if menu_item(ui, "󰅖  Close PDF · Return to Home", self.has_pdf())
                                        .on_hover_text("Close current document and return to Welcome Screen")
                                        .clicked()
                                    {
                                        self.close_pdf();
                                        close_requested = true;
                                    }

                                    if menu_item(ui, "󰆓  Save Notes", true)
                                        .on_hover_text("Save notes on disk")
                                        .clicked()
                                    {
                                        self.sync_and_save();
                                        self.toast_message = Some((
                                            "󰄬 Saved".to_string(),
                                            std::time::Instant::now(),
                                        ));
                                        close_requested = true;
                                    }

                                    if menu_item(ui, "󰇚  Save Original PDF...", true)
                                        .on_hover_text("Save a copy of the original, unmodified PDF")
                                        .clicked()
                                    {
                                        self.prompt_download_original_pdf(ui.ctx());
                                        close_requested = true;
                                    }

                                    ui.separator();
                                    menu_heading(ui, "NOTES & STUDY");
                                    if menu_item(ui, "󰈦  Import Notes (.txt / .md)...", true)
                                        .on_hover_text("Import notes mapped by slide headers (e.g. # Slide 1)")
                                        .clicked()
                                    {
                                        self.import_notes_dialog(ui.ctx());
                                        close_requested = true;
                                    }

                                    if menu_item(ui, "󰚥  Export Raw Notes (.md)", true)
                                        .on_hover_text("Export all notes into a single Markdown file")
                                        .clicked()
                                    {
                                        self.export_raw_notes_dialog(ui.ctx());
                                        close_requested = true;
                                    }

                                    ui.separator();
                                    menu_heading(ui, "EXPORT");
                                    let export_label = if self.is_exporting {
                                        "󱑚  Exporting...".to_string()
                                    } else {
                                        format!(
                                            "󰚥  Export PDF ({})",
                                            self.app_state.export_settings.content.short_label()
                                        )
                                    };

                                    if menu_item(ui, &export_label, !self.is_exporting).clicked() {
                                        self.start_export(ui.ctx());
                                        close_requested = true;
                                    }

                                });
                        });
 
                    let skip_outside = ui
                        .data_mut(|d| d.remove_temp::<bool>(ui.make_persistent_id("file_menu_skip_outside")))
                        .unwrap_or(false);
                    let clicked_outside = !skip_outside
                        && ui.input(|i| i.pointer.any_click())
                        && ui
                            .input(|i| i.pointer.interact_pos())
                            .map(|pos| {
                                !button_rect.contains(pos) && !area_resp.response.rect.contains(pos)
                            })
                            .unwrap_or(false);
 
                    if close_requested
                        || clicked_outside
                        || ui.input(|i| i.key_pressed(egui::Key::Escape))
                    {
                        is_open = false;
                    }
                }
 
                ui.data_mut(|d| *d.get_temp_mut_or_default::<bool>(menu_id) = is_open);

            });
        }

        if self.has_pdf() && self.view_mode != ViewMode::Slideshow {
         match self.app_state.settings.tools_layout {
             state::ToolsLayoutMode::SubTopbar => {
                 if self.show_tools_rail {
                     self.render_sub_topbar_tools(ui);
                 }
             }
             state::ToolsLayoutMode::VerticalRail => {
                if self.show_tools_rail {
                 self.render_vertical_tools_rail(ui);
                }
             }
         }
     }


        // Left Panel (Dual-Mode Sidebar: Thumbnails Grid & Outline/Bookmarks)
        if self.show_toc && self.has_pdf() && !in_slideshow{
            let scroll_sidebar_to_current =
                self.last_synced_sidebar_page != self.current_page;

            if scroll_sidebar_to_current {
                self.last_synced_sidebar_page = self.current_page;
            }
            egui::Panel::left("toc_panel")
                .resizable(true)
                .default_size(170.0)
                .show(ui, |ui| {
                    // Top switcher: thumbnails or outline.
                    ui.add_space(4.0);
                    ui.columns(2, |cols| {
                        if cols[0]
                            .add_sized(
                                [cols[0].available_width(), 26.0],
                                egui::Button::new(" 󰕰 ")
                                    .selected(self.sidebar_tab == SidebarTab::Thumbnails),
                            ).on_hover_text("Thumbnails")
                            .clicked()
                        {
                            self.sidebar_tab = SidebarTab::Thumbnails;
                        }
                        if cols[1]
                            .add_sized(
                                [cols[1].available_width(), 26.0],
                                egui::Button::new("  ")
                                    .selected(self.sidebar_tab == SidebarTab::Outline),
                            ).on_hover_text("Outline and Bookmarks")
                            .clicked()
                        {
                            self.sidebar_tab = SidebarTab::Outline;
                        }
                    });
                    ui.separator();

                    let mut page_to_go: Option<u32> = None;
                    let mut toggle_bm_page: Option<u32> = None;
                    let mut remove_bm_page: Option<u32> = None;
                    let mut delete_page_action: Option<u32> = None;
                    let mut move_page_action: Option<(u32, u32)> = None;

                    match self.sidebar_tab {
                        SidebarTab::Thumbnails => {
                            let row_h = 116.0; // Compact row height
                            let total = self.total_pages as usize;
                            let mut thumb_rows: Option<(usize, usize)> = None;
                            let avail_h = ui.available_height();

                            let last_thumb_page_id = ui.make_persistent_id("last_synced_thumb_page");
                            let prev_thumb_page: u32 = ui
                                .ctx()
                                .data_mut(|d| *d.get_temp_mut_or(last_thumb_page_id, 0u32));
                            let page_just_changed = prev_thumb_page != self.current_page;
                            if page_just_changed {
                                ui.ctx()
                                    .data_mut(|d| d.insert_temp(last_thumb_page_id, self.current_page));
                            }
                            
                            ui.spacing_mut().item_spacing.y = 0.0;


                            let mut thumb_scroll = egui::ScrollArea::vertical()
                                .id_salt("thumbs_scroll")
                                .auto_shrink([false, false]);

                            if scroll_sidebar_to_current || page_just_changed {
                            let max_offset = (
                                total as f32 * row_h - avail_h
                            ).max(0.0);

                            let ideal_y =
                                (self.current_page.saturating_sub(1) as f32 * row_h)
                                - (avail_h - row_h) * 0.5;

                            thumb_scroll = thumb_scroll.vertical_scroll_offset(
                                ideal_y.clamp(0.0, max_offset)
                            );
                            }


                            // Create IDs on the same `Ui` and reuse them
                            // inside `show_rows` and after the ScrollArea.
                            // IDs created inside row UIs differ from IDs created outside them.
                            let drag_source_id = ui.make_persistent_id("thumb_drag_source_page");
                            let drag_target_id = ui.make_persistent_id("thumb_drag_target_page");

                            let pointer_pos = ui.input(|i| {
                                i.pointer.interact_pos().or_else(|| i.pointer.hover_pos())
                            });

                            thumb_scroll.show_rows(ui, row_h, total, |ui, row_range| {
                                thumb_rows = Some((row_range.start, row_range.end));
                                let avail_w = ui.available_width();

                                for idx in row_range {
                                    let p = (idx + 1) as u32;
                                    let is_current = p == self.current_page;
                                    let is_hidden = self.hidden_pages.contains(&p);
                                    let has_note = self
                                        .notes
                                        .get(&p)
                                        .map(|n| !n.trim().is_empty())
                                        .unwrap_or(false);
                                    let is_bm = self.user_bookmarks.iter().any(|b| b.page == p);

                                    // Allocate exact `row_h` so virtual scrolling never drifts!
                                    let (row_rect, _) = ui.allocate_exact_size(
                                        egui::vec2(avail_w, row_h),
                                        egui::Sense::hover(),
                                    );

                                    let card_w = (avail_w - 14.0).clamp(90.0, 156.0);
                                    let card_h = row_h - 6.0;
                                    let card_rect = egui::Rect::from_center_size(
                                        row_rect.center(),
                                        egui::vec2(card_w, card_h),
                                    );
                                    let card_resp = ui.interact(
                                        card_rect,
                                        ui.make_persistent_id(("thumb_card", p)),
                                        egui::Sense::click_and_drag(),
                                    );

                                    if card_resp.drag_started() {
                                        ui.ctx().data_mut(|d| {
                                            d.insert_temp(drag_source_id, p);
                                            d.insert_temp(drag_target_id, p);
                                        });
                                    }
                                    let drag_source: u32 = ui.ctx().data(|d| d.get_temp(drag_source_id).unwrap_or(0));
                                    if drag_source != 0 {
                                        if let Some(pos) = pointer_pos.filter(|pos| card_rect.contains(*pos)) {
                                            // target = slot d'inserimento (da 1 a total_pages + 1).
                                            let target = if pos.y < card_rect.center().y {
                                                p
                                            } else {
                                                p.saturating_add(1).min(self.total_pages + 1)
                                            };
                                            ui.ctx().data_mut(|d| d.insert_temp(drag_target_id, target));
                                        }
                                    }

                                    let border_col = if is_current {
                                        rgb(self.theme.accent)
                                    } else if card_resp.hovered() {
                                        rgb(self.theme.fg_muted)
                                    } else {
                                        rgb(self.theme.border)
                                    };
                                    let bg_col = if is_current {
                                        rgb(self.theme.bg_active)
                                    } else {
                                        rgb(self.theme.bg_elevated)
                                    };

                                    ui.painter().rect_filled(
                                        card_rect,
                                        egui::CornerRadius::same(5),
                                        bg_col,
                                    );
                                    ui.painter().rect_stroke(
                                        card_rect,
                                        egui::CornerRadius::same(5),
                                        egui::Stroke::new(
                                            if is_current { 2.0 } else { 1.0 },
                                            border_col,
                                        ),
                                        egui::StrokeKind::Inside,
                                    );

                                    // Compact thumbnail image rect inside the card
                                    let img_rect = egui::Rect::from_min_max(
                                        egui::pos2(card_rect.left() + 5.0, card_rect.top() + 4.0),
                                        egui::pos2(
                                            card_rect.right() - 5.0,
                                            card_rect.bottom() - 20.0,
                                        ),
                                    );

                                    let tint = if is_hidden {
                                        egui::Color32::from_white_alpha(75)
                                    } else {
                                        egui::Color32::WHITE
                                    };

                                    if let Some((tex, (tw, th))) = self.thumb_cache.get(&p) {
                                        let ar = (*tw as f32 / (*th as f32).max(1.0)).max(0.1);
                                        let mut w = img_rect.width();
                                        let mut h = w / ar;
                                        if h > img_rect.height() {
                                            h = img_rect.height();
                                            w = h * ar;
                                        }
                                        let fitted = egui::Rect::from_center_size(
                                            img_rect.center(),
                                            egui::vec2(w, h),
                                        );
                                        ui.painter().image(
                                            tex.id(),
                                            fitted,
                                            egui::Rect::from_min_max(
                                                egui::pos2(0.0, 0.0),
                                                egui::pos2(1.0, 1.0),
                                            ),
                                            tint,
                                        );
                                    } else {
                                        ui.painter().rect_filled(
                                            img_rect,
                                            egui::CornerRadius::same(2),
                                            rgb(self.theme.bg_canvas),
                                        );
                                    }

                                    if is_hidden {
                                        ui.painter().text(
                                            img_rect.center(),
                                            egui::Align2::CENTER_CENTER,
                                            "Hidden",
                                            egui::FontId::proportional(11.0),
                                            rgb(self.theme.error),
                                        );
                                    }

                                    let active_drag_source: u32 = ui.ctx().data(|d| d.get_temp(drag_source_id).unwrap_or(0));
                                    let active_drag_target: u32 = ui.ctx().data(|d| d.get_temp(drag_target_id).unwrap_or(0));
                                    if active_drag_source != 0 && active_drag_source != p {
                                        let marker_y = if active_drag_target == p { Some(card_rect.top() + 1.0) }
                                            else if active_drag_target == p + 1 { Some(card_rect.bottom() - 1.0) }
                                            else { None };
                                        if let Some(y) = marker_y {
                                            ui.painter().line_segment(
                                                [egui::pos2(card_rect.left(), y), egui::pos2(card_rect.right(), y)],
                                                egui::Stroke::new(3.0, rgb(self.theme.accent)),
                                            );
                                        }
                                    }

                                    // Footer label inside card
                                    let footer_y = card_rect.bottom() - 10.0;
                                    let lbl = match (has_note, is_bm) {
                                        (true, true) => format!("p.{}  󰃀", p),
                                        (true, false) => format!("p.{} ", p),
                                        (false, true) => format!("p.{} 󰃀", p),
                                        (false, false) => format!("p.{}", p),
                                    };
                                    ui.painter().text(
                                        egui::pos2(card_rect.center().x, footer_y),
                                        egui::Align2::CENTER_CENTER,
                                        lbl,
                                        egui::FontId::proportional(11.0),
                                        if is_current {
                                            egui::Color32::WHITE
                                        } else {
                                            rgb(self.theme.fg_muted)
                                        },
                                    );

                                    // Hover quick-actions: Top-Left = Hide, Top-Right = Bookmark
                                    let pointer_in_card = pointer_pos
                                        .map(|pos| card_rect.contains(pos))
                                        .unwrap_or(false);

                                    let hide_rect = egui::Rect::from_min_size(
                                        egui::pos2(card_rect.left() + 6.0, card_rect.top() + 5.0),
                                        egui::vec2(18.0, 18.0),
                                    );
                                    let bm_rect = egui::Rect::from_min_size(
                                        egui::pos2(
                                            card_rect.right() - 24.0,
                                            card_rect.top() + 5.0,
                                        ),
                                        egui::vec2(18.0, 18.0),
                                    );
                                    let del_rect = egui::Rect::from_min_size(
                                        egui::pos2(card_rect.right() - 22.0, card_rect.bottom() - 20.0),
                                        egui::vec2(17.0, 17.0),
                                    );
                                    let del_hovered = pointer_pos
                                        .map(|pos| del_rect.contains(pos))
                                        .unwrap_or(false);

                                    if pointer_in_card && self.total_pages > 1 {
                                        ui.painter().rect_filled(
                                            del_rect,
                                            egui::CornerRadius::same(3),
                                            egui::Color32::from_black_alpha(175),
                                        );
                                        ui.painter().text(
                                            del_rect.center(),
                                            egui::Align2::CENTER_CENTER,
                                            "󰆴",
                                            egui::FontId::proportional(11.0),
                                            if del_hovered {
                                                rgb(self.theme.error)
                                            } else {
                                                egui::Color32::WHITE
                                            },
                                        );
                                    }

                                    let hide_hovered = pointer_pos
                                        .map(|pos| hide_rect.contains(pos))
                                        .unwrap_or(false);
                                    let bm_hovered = pointer_pos
                                        .map(|pos| bm_rect.contains(pos))
                                        .unwrap_or(false);

                                    // Show reorder arrows on hover.
                                    // Keep these separate from hide, bookmark, and delete actions.
                                    let move_up_rect = egui::Rect::from_min_size(
                                        egui::pos2(card_rect.left() + 6.0, card_rect.bottom() - 21.0),
                                        egui::vec2(18.0, 16.0),
                                    );
                                    let move_down_rect = egui::Rect::from_min_size(
                                        egui::pos2(card_rect.left() + 27.0, card_rect.bottom() - 21.0),
                                        egui::vec2(18.0, 16.0),
                                    );
                                    let up_resp = ui.interact(
                                        move_up_rect,
                                        ui.make_persistent_id(("thumb_move_up", p)),
                                        egui::Sense::click(),
                                    );
                                    let down_resp = ui.interact(
                                        move_down_rect,
                                        ui.make_persistent_id(("thumb_move_down", p)),
                                        egui::Sense::click(),
                                    );
                                    let up_hovered = up_resp.hovered();
                                    let down_hovered = down_resp.hovered();
                                    if pointer_in_card {
                                        for (rect, glyph, hovered) in [
                                            (move_up_rect, "↑", up_hovered),
                                            (move_down_rect, "↓", down_hovered),
                                        ] {
                                            ui.painter().rect_filled(
                                                rect,
                                                egui::CornerRadius::same(3),
                                                egui::Color32::from_black_alpha(175),
                                            );
                                            ui.painter().text(
                                                rect.center(),
                                                egui::Align2::CENTER_CENTER,
                                                glyph,
                                                egui::FontId::proportional(12.0),
                                                if hovered { rgb(self.theme.accent) } else { egui::Color32::WHITE },
                                            );
                                        }
                                    }
                                    if up_resp.clicked() && p > 1 {
                                        move_page_action = Some((p, p - 1));
                                    } else if down_resp.clicked() && p < self.total_pages {
                                        move_page_action = Some((p, p + 1));
                                    }

                                    if pointer_in_card || is_hidden {
                                        ui.painter().rect_filled(
                                            hide_rect,
                                            egui::CornerRadius::same(3),
                                            egui::Color32::from_black_alpha(175),
                                        );
                                        ui.painter().text(
                                            hide_rect.center(),
                                            egui::Align2::CENTER_CENTER,
                                            if is_hidden { "󰈉 " } else { "󰈈 " },
                                            egui::FontId::proportional(11.5),
                                            if is_hidden || hide_hovered {
                                                rgb(self.theme.error)
                                            } else {
                                                egui::Color32::WHITE
                                            },
                                        );
                                    }

                                    if pointer_in_card || is_bm {
                                        ui.painter().rect_filled(
                                            bm_rect,
                                            egui::CornerRadius::same(3),
                                            egui::Color32::from_black_alpha(175),
                                        );
                                        ui.painter().text(
                                            bm_rect.center(),
                                            egui::Align2::CENTER_CENTER,
                                            if is_bm { "󰃀" } else { "󰃃" },
                                            egui::FontId::proportional(11.5),
                                            if is_bm || bm_hovered {
                                                rgb(self.theme.warning)
                                            } else {
                                                egui::Color32::WHITE
                                            },
                                        );
                                    }

                                    // Conferma di cancellazione inline
                                    let confirming = self.pending_delete_page == Some(p);
                                    let confirm_rect = egui::Rect::from_center_size(
                                        egui::pos2(img_rect.center().x - 24.0, img_rect.center().y + 14.0),
                                        egui::vec2(40.0, 22.0),
                                    );
                                    let cancel_rect = egui::Rect::from_center_size(
                                        egui::pos2(img_rect.center().x + 24.0, img_rect.center().y + 14.0),
                                        egui::vec2(40.0, 22.0),
                                    );
                                    let confirm_hovered = pointer_pos.map(|pos| confirm_rect.contains(pos)).unwrap_or(false);
                                    let cancel_hovered = pointer_pos.map(|pos| cancel_rect.contains(pos)).unwrap_or(false);

                                    if confirming {
                                        ui.painter().rect_filled(
                                            card_rect,
                                            egui::CornerRadius::same(4),
                                            egui::Color32::from_black_alpha(215),
                                        );
                                        ui.painter().text(
                                            egui::pos2(img_rect.center().x, img_rect.center().y - 12.0),
                                            egui::Align2::CENTER_CENTER,
                                            "Delete page?",
                                            egui::FontId::proportional(11.5),
                                            egui::Color32::WHITE,
                                        );
                                        let confirm_color = if confirm_hovered {
                                        let error = self.theme.error;

                                        let lighten = |c: u8| -> u8 {
                                            (c as f32 + (255.0 - c as f32) * 0.25) as u8
                                        };

                                        egui::Color32::from_rgb(
                                            lighten(error[0]),
                                            lighten(error[1]),
                                            lighten(error[2]),
                                        )
                                    } else {
                                        rgb(self.theme.error)
                                    };

                                    ui.painter().rect_filled(
                                        confirm_rect,
                                        egui::CornerRadius::same(4),
                                        confirm_color,
                                    );
                                        ui.painter().text(
                                            confirm_rect.center(),
                                            egui::Align2::CENTER_CENTER,
                                            "Yes",
                                            egui::FontId::proportional(11.0),
                                            egui::Color32::WHITE,
                                        );
                                        ui.painter().rect_filled(
                                            cancel_rect,
                                            egui::CornerRadius::same(4),
                                            egui::Color32::from_white_alpha(if cancel_hovered { 60 } else { 30 }),
                                        );
                                        ui.painter().text(
                                            cancel_rect.center(),
                                            egui::Align2::CENTER_CENTER,
                                            "No",
                                            egui::FontId::proportional(11.0),
                                            egui::Color32::WHITE,
                                        );
                                    }

                                    if card_resp.clicked() && !up_resp.clicked() && !down_resp.clicked() {
                                        if confirming {
                                            if confirm_hovered {
                                                delete_page_action = Some(p);
                                            }
                                            self.pending_delete_page = None; // Yes deletes; any other click cancels.
                                        } else if hide_hovered {
                                            self.toggle_hide_page(p, ui);
                                        } else if bm_hovered {
                                            toggle_bm_page = Some(p);
                                        } else if del_hovered && self.total_pages > 1 {
                                            if self.is_disposable_blank_page(p) {
                                                delete_page_action = Some(p);
                                            } else {
                                                self.pending_delete_page = Some(p);
                                            }
                                        } else {
                                            page_to_go = Some(p);
                                        }
                                    }
                                }
                            });

                            let drag_released = ui.input(|i| i.pointer.any_released());
                            if drag_released {
                                let (source, target): (u32, u32) = ui.ctx().data(|d| (
                                    d.get_temp(drag_source_id).unwrap_or(0),
                                    d.get_temp(drag_target_id).unwrap_or(0),
                                ));
                                if source != 0 && target != 0 && source != target {
                                    let final_target = if source < target { target.saturating_sub(1) } else { target };
                                    let final_target = final_target.clamp(1, self.total_pages);
                                    if final_target != source { move_page_action = Some((source, final_target)); }
                                }
                                ui.ctx().data_mut(|d| {
                                    d.insert_temp(drag_source_id, 0u32);
                                    d.insert_temp(drag_target_id, 0u32);
                                });
                            }

                            if let Some((start, end)) = thumb_rows {
                                self.request_thumbs(start, end);
                            }
                        }

                        SidebarTab::Outline => {
                            egui::ScrollArea::vertical()
                                .id_salt("toc_scroll")
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    if self.user_bookmarks.is_empty() && self.outline.is_empty() {
                                        // Remain cleanly empty when document has no outline/bookmarks
                                        return;
                                    }

                                    // 1. User Bookmarks
                                    if !self.user_bookmarks.is_empty() {
                                        ui.label(
                                            egui::RichText::new("󰃀  Bookmarks")
                                                .size(12.0)
                                                .color(rgb(self.theme.fg_muted)),
                                        );
                                        for bm in &self.user_bookmarks {
                                            let is_active = self.current_page == bm.page;
                                            let has_note = self
                                                .notes
                                                .get(&bm.page)
                                                .map(|s| !s.trim().is_empty())
                                                .unwrap_or(false);
                                            ui.horizontal(|ui| {
                                                let prefix = if has_note { "● " } else { "" };
                                                let label = format!(
                                                    "{}{}  (p. {})",
                                                    prefix, bm.title, bm.page
                                                );
                                                let resp = ui
                                                    .selectable_label(is_active, label);
                                                if is_active && scroll_sidebar_to_current {
                                                    resp.scroll_to_me(Some(egui::Align::Center));
                                                }
                                                if resp.clicked() {
                                                    page_to_go = Some(bm.page);
                                                }
                                                ui.with_layout(
                                                    egui::Layout::right_to_left(
                                                        egui::Align::Center,
                                                    ),
                                                    |ui| {
                                                        if ui
                                                            .small_button("󰅖")
                                                            .on_hover_text("Delete bookmark")
                                                            .clicked()
                                                        {
                                                            remove_bm_page = Some(bm.page);
                                                        }
                                                    },
                                                );
                                            });
                                        }
                                        if !self.outline.is_empty() {
                                            ui.separator();
                                        }
                                    }

                                    // 2. Document Native Outline (if present)
                                    if !self.outline.is_empty() {
                                        ui.label(
                                            egui::RichText::new("󰈙 Document Outline")
                                                .size(12.0)
                                                .color(rgb(self.theme.fg_muted)),
                                        );
                                        for item in &self.outline {
                                            let is_active = self.current_page == item.page;
                                            let has_note = self
                                                .notes
                                                .get(&item.page)
                                                .map(|s| !s.trim().is_empty())
                                                .unwrap_or(false);
                                            let text = if has_note {
                                                format!("● {}", item.title)
                                            } else {
                                                item.title.clone()
                                            };
                                            let resp = ui.selectable_label(is_active, text);
                                            if is_active && scroll_sidebar_to_current {
                                                resp.scroll_to_me(Some(egui::Align::Center));
                                                }
                                            if resp.clicked(){
                                                page_to_go = Some(item.page);
                                            }
                                        }
                                    }
                                });
                        }
                    }

                    if let Some(p) = toggle_bm_page {
                        if self.user_bookmarks.iter().any(|b| b.page == p) {
                            self.remove_bookmark(p);
                        } else {
                            self.add_or_update_bookmark(p, format!("Page {}", p));
                        }
                    }
                    if let Some(p) = remove_bm_page {
                        self.remove_bookmark(p);
                    }
                    if let Some(target) = page_to_go {
                        self.go_to_page(target, ui.ctx());
                    }
                    if let Some(del_p) = delete_page_action {
                        self.delete_pdf_page_and_shift_data(del_p, ui.ctx());
                    }
                    if let Some((from, to)) = move_page_action {
                        self.reorder_pdf_page(from, to, ui.ctx());
                    }
                });
        }

        // Right Panel (Notes + Bottom Annotation Tools)
        if self.has_pdf() {
        if self.view_mode == ViewMode::SlideshowWithNotes {let active_slide_note = if self.note_page == self.current_page {
            self.current_note.trim().to_string()
        } else {
            self.notes
                .get(&self.current_page)
                .cloned()
                .unwrap_or_default()
                .trim()
                .to_string()
        };

        // In `ExpandFull` mode, if the current slide has no note (and note page isn't locked),
        // we hide the right panel completely so the slide expands to 100% fullscreen!
        let hide_panel_for_expand = self.view_mode == ViewMode::SlideshowWithNotes
            && self.app_state.settings.slideshow_empty_mode == SlideshowEmptyMode::ExpandFull
            && active_slide_note.is_empty()
            && !self.lock_note_page;

        if self.view_mode == ViewMode::SlideshowWithNotes
            && self.has_pdf()
            && !hide_panel_for_expand
        {
            let show_hud = ui
                        .input(|i| i.pointer.hover_pos())
                        .map(|p| p.y < 60.0)
                        .unwrap_or(false);
                    if show_hud {
                        let theme = &self.theme;

                        let to_color = |rgb: [u8; 3]| {
                            egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2])
                        };

                        egui::Area::new(ui.make_persistent_id("slideshow_hud"))
                            .anchor(egui::Align2::CENTER_TOP, egui::vec2(-20.0, 20.0))
                            .order(egui::Order::Foreground)
                            .show(ui.ctx(), |ui| {
                                egui::Frame::NONE
                                    .fill(to_color(theme.bg_elevated))
                                    .corner_radius(20)
                                    .stroke(egui::Stroke::new(
                                        1.0,
                                        to_color(theme.border),
                                    ))
                                    .inner_margin(egui::Margin::symmetric(14, 6))
                                    .shadow(egui::epaint::Shadow {
                                        offset: [0, 4],
                                        blur: 12,
                                        spread: 0,
                                        color: egui::Color32::from_black_alpha(100),
                                    })
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 10.0;

                                            ui.scope(|ui| {
                                                let visuals = &mut ui.style_mut().visuals.widgets;

                                                visuals.hovered.weak_bg_fill =
                                                    to_color(theme.bg_hover);
                                                visuals.hovered.fg_stroke =
                                                    egui::Stroke::new(
                                                        1.0,
                                                        to_color(theme.accent),
                                                    );
                                                visuals.hovered.corner_radius =
                                                    egui::CornerRadius::same(12);

                                                visuals.active.weak_bg_fill =
                                                    to_color(theme.bg_active);
                                                visuals.active.fg_stroke =
                                                    egui::Stroke::new(
                                                        1.0,
                                                        to_color(theme.accent),
                                                    );
                                                visuals.active.corner_radius =
                                                    egui::CornerRadius::same(12);

                                                let exit_btn = egui::Button::new(
                                                    egui::RichText::new("󰅖 Exit")
                                                        .size(13.0)
                                                        .strong()
                                                        .color(to_color(theme.accent)),
                                                )
                                                .frame(false)
                                                .corner_radius(12);

                                                if ui
                                                    .add(exit_btn)
                                                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                                                    .on_hover_text("Press Esc to exit")
                                                    .clicked()
                                                {
                                                    self.view_mode = self.prev_view_mode;
                                                }
                                            });
                                        });
                                    });
                            });
                    }

                egui::Panel::right("slideshow_notes_panel")
                    .resizable(true)
                    .default_size(320.0)
                    .show(ui, |ui| {
                        self.render_slideshow_notes_contents(
                            ui,
                            active_slide_note,
                        );
                    });
            
            }
        } else if self.show_notes && !in_slideshow {
            if use_bottom_notes {
            let max_panel_h = (win_rect.height() * 0.75).max(220.0);
            let notes_panel = egui::Panel::bottom("notes_panel_bottom")
                .resizable(true)
                .default_size((win_rect.height() * 0.36).clamp(180.0, 420.0))
                .size_range(120.0..=max_panel_h)
                // Keep the same frame in Edit, Preview, and Sketch so switching modes
                // never shifts the panel contents or changes its apparent height. Restore
                // a modest horizontal inset so content does not sit against the panel edges.
                .frame(
                    egui::Frame::side_top_panel(ui.style())
                        .inner_margin(egui::Margin::symmetric(6, 0)),
                );
            notes_panel
                .show(ui, |ui| {
                    self.render_notes_panel_contents(
                        ui,
                        text_edit_id,
                        true,
                    );
                });
        } else {
            let notes_panel = egui::Panel::right("notes_panel")
                .resizable(true)
                .default_size(350.0)
                .size_range(250.0..=520.0)
                // Match the bottom-panel treatment: a stable frame across all note modes,
                // with a modest horizontal inset around the notes content.
                .frame(
                    egui::Frame::side_top_panel(ui.style())
                        .inner_margin(egui::Margin::symmetric(6, 0)),
                );
            notes_panel
                .show(ui, |ui| {
                    self.render_notes_panel_contents(
                        ui,
                        text_edit_id,
                        false,
                    );
                });
        }
        }
        }

        // Central Panel (Welcome Screen when no PDF is loaded, otherwise PDF Canvas)
        // Secondary PDF Pane when Split-View is active (resizable side-by-side PDF view!)
        if self.split_view_enabled && self.has_pdf() && !in_slideshow {
            egui::Panel::right("pdf_split_secondary_pane")
                .resizable(true)
                .default_size(460.0)
                .size_range(240.0..=960.0)
                .frame(
                    egui::Frame::NONE
                        .fill(rgb(self.theme.bg_canvas))
                        .inner_margin(egui::Margin::same(1))
                        .stroke(egui::Stroke::new(1.0, rgb(self.theme.border))),
                )
                .show(ui, |ui| {
                    // Compact Top Navigation Bar for the Secondary PDF View
                    egui::Frame::NONE
                        .fill(rgb(self.theme.bg_panel))
                        .corner_radius(egui::CornerRadius::same(6))
                        .inner_margin(egui::Margin::symmetric(8, 2))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new("󰯌 Split")
                                        .strong()
                                        .size(12.5)
                                        .color(rgb(self.theme.accent)),
                                );
                                ui.separator();

                                if ui
                                    .add_sized([22.0, 22.0], egui::Button::new(""))
                                    .clicked()
                                    && self.split_secondary_page > 1
                                {
                                    self.split_secondary_page -= 1;
                                }

                                let mut sec_p = self.split_secondary_page;
                                if ui
                                    .add_sized(
                                        [46.0, 22.0],
                                        egui::DragValue::new(&mut sec_p)
                                            .prefix("p.")
                                            .range(1..=self.total_pages.max(1))
                                            .speed(0.1),
                                    )
                                    .changed()
                                {
                                    self.split_secondary_page =
                                        sec_p.clamp(1, self.total_pages.max(1));
                                }

                                if ui
                                    .add_sized([22.0, 22.0], egui::Button::new(""))
                                    .clicked()
                                    && self.split_secondary_page < self.total_pages
                                {
                                    self.split_secondary_page += 1;
                                }

                                ui.separator();

                                // Independent zoom controls for the secondary pane
                                if ui.small_button("󰍴").clicked() {
                                    self.split_secondary_zoom =
                                        (self.split_secondary_zoom - 0.15).max(0.4);
                                }
                                if ui
                                    .small_button(format!(
                                        "{:.0}%",
                                        self.split_secondary_zoom * 100.0
                                    ))
                                    .on_hover_text("Reset secondary zoom to 100%")
                                    .clicked()
                                {
                                    self.split_secondary_zoom = 1.0;
                                }
                                if ui.small_button("󰐕").clicked() {
                                    self.split_secondary_zoom =
                                        (self.split_secondary_zoom + 0.15).min(4.0);
                                }

                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui
                                            .small_button("󰅖")
                                            .on_hover_text("Close Split-View")
                                            .clicked()
                                        {
                                            self.split_view_enabled = false;
                                        }
                                        if ui
                                            .small_button("󰁨 =Main")
                                            .on_hover_text("Jump secondary view to current main slide")
                                            .clicked()
                                        {
                                            self.split_secondary_page = self.current_page;
                                        }
                                    },
                                );
                            });
                        });

                    ui.add_space(6.0);

                    let avail = ui.available_size();
                    let sec_page = self.split_secondary_page;

                    // Lookup high-res texture in `page_cache`, or fallback to `thumb_cache`
                    let tex_opt = self
                        .page_cache
                        .get(&sec_page)
                        .cloned()
                        .or_else(|| self.thumb_cache.get(&sec_page).cloned());

                    if let Some((tex, (tw, th))) = tex_opt {
                        let ar = (th as f32 / (tw as f32).max(1.0)).max(0.1);
                        let fit_w = (avail.x - 2.0).max(80.0);
                        let fit_h = fit_w * ar;
                        let (base_w, base_h) = if fit_h > avail.y - 2.0 {
                            let h = (avail.y - 2.0).max(80.0);
                            (h / ar, h)
                        } else {
                            (fit_w, fit_h)
                        };

                        let disp_w = base_w * self.split_secondary_zoom;
                        let disp_h = base_h * self.split_secondary_zoom;

                        egui::ScrollArea::both()
                            .id_salt("split_secondary_scroll")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                let pad_x = ((avail.x - disp_w) * 0.5).max(0.0);
                                let pad_y = ((avail.y - disp_h) * 0.5).max(0.0);
                                ui.add_space(pad_y);
                                ui.horizontal(|ui| {
                                    ui.add_space(pad_x);
                                    let sense = egui::Sense::click_and_drag();
                                    let img_resp = ui.add(
                                        egui::Image::new((
                                            tex.id(),
                                            egui::vec2(disp_w, disp_h),
                                        ))
                                        .sense(sense),
                                    );
                                    let slide_rect = img_resp.rect;
                                    self.handle_slide_annotations(
                                        ui,
                                        sec_page,
                                        slide_rect,
                                        &img_resp,
                                    );
                                });
                            });
                    } else {
                        ui.centered_and_justified(|ui| {
                            ui.spinner();
                        });
                    }
                });
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::central_panel(&ui.style())
                    .fill(rgb(self.theme.bg_canvas))
                    .inner_margin(egui::Margin::ZERO),
            )
            .show(ui, |ui| {
                if !self.has_pdf() {
                    self.show_toc = false;
                    self.show_notes = false;

                    let mut file_to_open: Option<PathBuf> = None;
                    let mut file_to_remove: Option<PathBuf> = None;

                    // Collect completed first-page previews and schedule missing ones off the UI thread.
                    while let Ok((path, image)) = self.home_thumb_rx.try_recv() {
                        let size = image.size;
                        let texture = ui.ctx().load_texture(
                            format!("home-preview:{}", path.display()),
                            image,
                            egui::TextureOptions::LINEAR,
                        );
                        self.home_thumb_cache
                            .insert(path.clone(), (texture, size.into()));
                        self.home_thumb_pending.remove(&path);
                    }

                    for entry in &self.app_state.recent_files {
                        let path = entry.path.clone();
                        if path.is_file()
                            && !self.home_thumb_cache.contains_key(&path)
                            && self.home_thumb_pending.insert(path.clone())
                        {
                            let tx = self.home_thumb_tx.clone();
                            let ctx = ui.ctx().clone();
                            thread::spawn(move || {
                                let rendered = path
                                    .to_str()
                                    .and_then(|p| Document::open(p).ok())
                                    .and_then(|doc| render_to_image(&doc, 0, 0.42, false))
                                    .map(|(_, image, _)| image);
                                if let Some(image) = rendered {
                                    let _ = tx.send((path, image));
                                    ctx.request_repaint();
                                }
                            });
                        }
                    }

                            ui.vertical_centered(|ui| {
                                let content_width =
                                    (ui.available_width() - 48.0).clamp(260.0, 1040.0);
                                ui.set_width(content_width);

                                ui.add_space(60.0);

                                // 1. Title and controls
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 12.0;
                                    ui.label(
                                        egui::RichText::new("")
                                            .size(30.0)
                                            .color(rgb(self.theme.accent)),
                                    );
                                    ui.label(
                                        egui::RichText::new(" Lapis")
                                            .size(26.0)
                                            .strong()
                                            .color(rgb(self.theme.fg_primary)),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            let settings = egui::Button::new(
                                                egui::RichText::new("  Settings")
                                                    .size(12.0)
                                                    .color(rgb(self.theme.fg_primary)),
                                            )
                                            .corner_radius(egui::CornerRadius::same(6))
                                            .min_size(egui::vec2(92.0, 30.0));
                                            if ui
                                                .add(settings)
                                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                                .clicked()
                                            {
                                                self.show_settings = true;
                                            }
                                        },
                                    );
                                });

                                ui.add_space(28.0);

                                // 2. Quick actions
                                ui.columns(2, |cols| {
                                    // Open PDF
                                    let open_frame = egui::Frame::NONE
                                        .fill(rgb(self.theme.bg_panel))
                                        .stroke(egui::Stroke::new(1.0, rgb(self.theme.border)))
                                        .corner_radius(egui::CornerRadius::same(
                                            self.theme.corner_radius,
                                        ))
                                        .inner_margin(egui::Margin::same(16))
                                        .show(&mut cols[0], |ui| {
                                            ui.set_width(ui.available_width());

                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    egui::RichText::new("󰝰  Open PDF")
                                                        .size(15.5)
                                                        .strong()
                                                        .color(rgb(self.theme.fg_primary)),
                                                );
                                            });

                                            ui.add_space(6.0);

                                            ui.label(
                                                egui::RichText::new(
                                                    "Import an existing PDF document from your computer.",
                                                )
                                                .size(11.5)
                                                .color(rgb(self.theme.fg_muted)),
                                            );

                                            ui.add_space(14.0);

                                            ui.label(
                                                egui::RichText::new("Browse files  →")
                                                    .size(12.0)
                                                    .strong()
                                                    .color(rgb(self.theme.accent)),
                                            );
                                        });

                                    let open_rect = open_frame.response.rect;
                                    let open_resp = cols[0]
                                        .interact(
                                            open_rect,
                                            cols[0].id().with("action_open_pdf"),
                                            egui::Sense::click(),
                                        )
                                        .on_hover_cursor(egui::CursorIcon::PointingHand);

                                    if open_resp.hovered() {
                                        cols[0].painter().rect_stroke(
                                            open_rect,
                                            egui::CornerRadius::same(self.theme.corner_radius),
                                            egui::Stroke::new(1.0, rgb(self.theme.accent)),
                                            egui::StrokeKind::Inside,
                                        );
                                    }

                                    if open_resp.clicked() {
                                        self.dialogs.open(
                                            cols[0].ctx(),
                                            DialogKind::OpenPdf,
                                            DialogSpec::open_file()
                                                .filter("PDF files", &["pdf"]),
                                        );
                                    }

                                    // New Notebook
                                    let nb_frame = egui::Frame::NONE
                                        .fill(rgb(self.theme.bg_panel))
                                        .stroke(egui::Stroke::new(1.0, rgb(self.theme.border)))
                                        .corner_radius(egui::CornerRadius::same(
                                            self.theme.corner_radius,
                                        ))
                                        .inner_margin(egui::Margin::same(16))
                                        .show(&mut cols[1], |ui| {
                                            ui.set_width(ui.available_width());

                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    egui::RichText::new("󱞁  Blank Notebook")
                                                        .size(15.5)
                                                        .strong()
                                                        .color(rgb(self.theme.fg_primary)),
                                                );
                                            });

                                            ui.add_space(6.0);

                                            ui.label(
                                                egui::RichText::new(
                                                    "Create a new blank study notebook from scratch.",
                                                )
                                                .size(11.5)
                                                .color(rgb(self.theme.fg_muted)),
                                            );

                                            ui.add_space(14.0);

                                            ui.label(
                                                egui::RichText::new("Create notebook  →")
                                                    .size(12.0)
                                                    .strong()
                                                    .color(rgb(self.theme.accent)),
                                            );
                                        });

                                    let nb_rect = nb_frame.response.rect;
                                    let nb_resp = cols[1]
                                        .interact(
                                            nb_rect,
                                            cols[1].scope_id().with("action_new_notebook"),
                                            egui::Sense::click(),
                                        )
                                        .on_hover_cursor(egui::CursorIcon::PointingHand);

                                    if nb_resp.hovered() {
                                        cols[1].painter().rect_stroke(
                                            nb_rect,
                                            egui::CornerRadius::same(self.theme.corner_radius),
                                            egui::Stroke::new(1.0, rgb(self.theme.accent)),
                                            egui::StrokeKind::Inside,
                                        );
                                    }

                                    if nb_resp.clicked() {
                                        self.dialogs.open(
                                            cols[1].ctx(),
                                            DialogKind::NewNotebook,
                                            DialogSpec::save_file()
                                                .file_name("notebook.pdf")
                                                .filter("PDF Notebook", &["pdf"]),
                                        );
                                    }
                                });

                                ui.add_space(32.0);

                                // 3. Recent Documents
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new("Recent documents")
                                            .size(15.5)
                                            .strong()
                                            .color(rgb(self.theme.fg_primary)),
                                    );

                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui
                                                .selectable_label(
                                                    self.app_state.home_grid_view,
                                                    " 󱇙 ",
                                                )
                                                .on_hover_text("Grid view")
                                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                                .clicked()
                                            {
                                                self.app_state.home_grid_view = true;
                                                self.app_state.save();
                                            }
                                            if ui
                                                .selectable_label(
                                                    !self.app_state.home_grid_view,
                                                    "  ",
                                                )
                                                .on_hover_text("List view")
                                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                                .clicked()
                                            {
                                                self.app_state.home_grid_view = false;
                                                self.app_state.save();
                                            }
                                        },
                                    );
                                });

                                ui.add_space(12.0);

                                if self.app_state.recent_files.is_empty() {
                                    egui::Frame::NONE
                                        .fill(rgb(self.theme.bg_panel))
                                        .stroke(egui::Stroke::new(1.0, rgb(self.theme.border)))
                                        .corner_radius(egui::CornerRadius::same(
                                            self.theme.corner_radius,
                                        ))
                                        .inner_margin(egui::Margin::same(28))
                                        .show(ui, |ui| {
                                            ui.set_width((content_width - 58.0).max(100.0));
                                            ui.vertical_centered(|ui| {
                                                ui.label(
                                                    egui::RichText::new("󰈙")
                                                        .size(24.0)
                                                        .color(rgb(self.theme.fg_muted)),
                                                );
                                                ui.label(
                                                    egui::RichText::new("No documents yet")
                                                        .size(14.0)
                                                        .strong()
                                                        .color(rgb(self.theme.fg_primary)),
                                                );
                                                ui.add_space(4.0);
                                                ui.label(
                                                    egui::RichText::new(
                                                        "Your recently opened PDFs will appear here.",
                                                    )
                                                    .size(11.5)
                                                    .color(rgb(self.theme.fg_muted)),
                                                );
                                            });
                                        });
                                } else {
                                    egui::ScrollArea::vertical()
                                        .id_salt("recent_files_scroll")
                                        .auto_shrink([false, true])
                                        .max_height((ui.available_height() - 32.0).max(120.0))                                        
                                        .show(ui, |ui| {
                                            if self.app_state.home_grid_view {
                                                let gap = 12.0_f32;
                                                let available = ui.available_width().max(1.0);
                                                let cols = (((available + gap) / (165.0 + gap))
                                                    .floor()
                                                    as usize)
                                                    .clamp(1, 6);
                                                let card_w = (available
                                                    - gap * (cols - 1) as f32)
                                                    / cols as f32;
                                                let card_h = 152.0_f32;

                                                egui::Grid::new("recent_documents_grid")
                                                    .num_columns(cols)
                                                    .spacing(egui::vec2(gap, gap))
                                                    .show(ui, |ui| {
                                                        for (idx, entry) in self
                                                            .app_state
                                                            .recent_files
                                                            .iter()
                                                            .enumerate()
                                                        {
                                                            let cell = ui.allocate_ui_with_layout(
                                                                egui::vec2(card_w, card_h),
                                                                egui::Layout::top_down(
                                                                    egui::Align::Min,
                                                                ),
                                                                |cell_ui| {
                                                                    let frame = egui::Frame::NONE
                                                                        .fill(rgb(
                                                                            self.theme.bg_panel,
                                                                        ))
                                                                        .stroke(egui::Stroke::new(
                                                                            1.0,
                                                                            rgb(self.theme.border),
                                                                        ))
                                                                        .corner_radius(
                                                                            egui::CornerRadius::same(8),
                                                                        )
                                                                        .inner_margin(
                                                                            egui::Margin::same(8),
                                                                        )
                                                                        .show(cell_ui, |card_ui| {
                                                                            // card_w - margin(16) - stroke(2) = card_w - 18
                                                                            let inner_w = (card_w
                                                                                - 18.0)
                                                                                .max(60.0);
                                                                            card_ui
                                                                                .set_width(inner_w);

                                                                            let preview_h =
                                                                                card_h - 60.0;
                                                                            card_ui.allocate_ui_with_layout(
                                                                                egui::vec2(inner_w, preview_h),
                                                                                egui::Layout::centered_and_justified(egui::Direction::TopDown),
                                                                                |preview_ui| {
                                                                                    if let Some((texture, (w, h))) = self.home_thumb_cache.get(&entry.path) {
                                                                                        let max_w = inner_w - 4.0;
                                                                                        let max_h = preview_h - 4.0;
                                                                                        let scale = (max_w / (*w as f32).max(1.0)).min(max_h / (*h as f32).max(1.0));
                                                                                        let size = egui::vec2(*w as f32 * scale, *h as f32 * scale);
                                                                                        preview_ui.add(egui::Image::new((texture.id(), size)));
                                                                                    } else if self.home_thumb_pending.contains(&entry.path) {
                                                                                        preview_ui.spinner();
                                                                                    } else {
                                                                                        preview_ui.label(
                                                                                            egui::RichText::new("PDF")
                                                                                                .size(10.0)
                                                                                                .color(rgb(self.theme.fg_muted)),
                                                                                        );
                                                                                    }
                                                                                },
                                                                            );

                                                                            card_ui.add_space(6.0);

                                                                            card_ui.add_sized(
                                                                                [inner_w, 17.0],
                                                                                egui::Label::new(
                                                                                    egui::RichText::new(
                                                                                        truncate_filename(&entry.name, 22),
                                                                                    )
                                                                                    .size(13.0)
                                                                                    .strong()
                                                                                    .color(rgb(self.theme.fg_primary)),
                                                                                )
                                                                                .truncate(),
                                                                            );

                                                                            card_ui.label(
                                                                                egui::RichText::new(format!(
                                                                                    "Page {} / {}",
                                                                                    entry.last_page,
                                                                                    entry.total_pages,
                                                                                ))
                                                                                .size(10.0)
                                                                                .color(rgb(self.theme.fg_muted)),
                                                                            );

                                                                            card_ui.add_space(4.0);

                                                                            let progress = (entry.last_page as f32
                                                                                / (entry.total_pages.max(1) as f32))
                                                                                .clamp(0.0, 1.0);
                                                                            let (bar_rect, _) = card_ui.allocate_exact_size(
                                                                                egui::vec2(inner_w, 3.0),
                                                                                egui::Sense::hover(),
                                                                            );
                                                                            card_ui.painter().rect_filled(
                                                                                bar_rect,
                                                                                2.0,
                                                                                rgb(self.theme.border),
                                                                            );
                                                                            let mut fill_rect = bar_rect;
                                                                            fill_rect.set_width(
                                                                                bar_rect.width() * progress,
                                                                            );
                                                                            card_ui.painter().rect_filled(
                                                                                fill_rect,
                                                                                2.0,
                                                                                rgb(self.theme.accent),
                                                                            );
                                                                        });

                                                                    let rect = frame.response.rect;
                                                                    let close_rect =
                                                                        egui::Rect::from_min_size(
                                                                            egui::pos2(
                                                                                rect.right()
                                                                                    - 24.0,
                                                                                rect.top() + 6.0,
                                                                            ),
                                                                            egui::vec2(18.0, 18.0),
                                                                        );

                                                                    let hovered = cell_ui
                                                                        .rect_contains_pointer(
                                                                            rect,
                                                                        );
                                                                    let on_close = hovered
                                                                        && cell_ui
                                                                            .rect_contains_pointer(
                                                                                close_rect,
                                                                            );

                                                                    if hovered {
                                                                        cell_ui
                                                                            .painter()
                                                                            .rect_stroke(
                                                                                rect,
                                                                                egui::CornerRadius::same(8),
                                                                                egui::Stroke::new(
                                                                                    1.0,
                                                                                    rgb(self.theme.accent),
                                                                                ),
                                                                                egui::StrokeKind::Inside,
                                                                            );
                                                                    }

                                                                    let mut remove_clicked = false;
                                                                    let mut card_clicked = false;

                                                                    if on_close {
                                                                        let close_resp = cell_ui
                                                                            .interact(
                                                                                close_rect,
                                                                                cell_ui.id().with((
                                                                                    "remove_btn",
                                                                                    idx,
                                                                                )),
                                                                                egui::Sense::click(),
                                                                            )
                                                                            .on_hover_text(
                                                                                "Remove from history",
                                                                            )
                                                                            .on_hover_cursor(
                                                                                egui::CursorIcon::PointingHand,
                                                                            );
                                                                        remove_clicked =
                                                                            close_resp.clicked();
                                                                    } else {
                                                                        let card_resp = cell_ui
                                                                            .interact(
                                                                                rect,
                                                                                cell_ui.id().with((
                                                                                    "recent_card",
                                                                                    idx,
                                                                                )),
                                                                                egui::Sense::click(),
                                                                            )
                                                                            .on_hover_text(
                                                                                entry
                                                                                    .path
                                                                                    .to_string_lossy(),
                                                                            )
                                                                            .on_hover_cursor(
                                                                                egui::CursorIcon::PointingHand,
                                                                            );
                                                                        card_clicked =
                                                                            card_resp.clicked();
                                                                    }

                                                                    if hovered {
                                                                        cell_ui
                                                                            .painter()
                                                                            .rect_filled(
                                                                                close_rect,
                                                                                4.0,
                                                                                if on_close {
                                                                                    rgb(self.theme.accent)
                                                                                } else {
                                                                                    rgb(self.theme.bg_canvas)
                                                                                },
                                                                            );
                                                                        cell_ui.painter().text(
                                                                            close_rect.center(),
                                                                            egui::Align2::CENTER_CENTER,
                                                                            "×",
                                                                            egui::FontId::proportional(13.0),
                                                                            rgb(self.theme.fg_primary),
                                                                        );
                                                                    }

                                                                    (remove_clicked, card_clicked)
                                                                },
                                                            );

                                                            let (remove_clicked, card_clicked) =
                                                                cell.inner;
                                                            if remove_clicked {
                                                                file_to_remove =
                                                                    Some(entry.path.clone());
                                                            } else if card_clicked {
                                                                file_to_open =
                                                                    Some(entry.path.clone());
                                                            }

                                                            if (idx + 1) % cols == 0 {
                                                                ui.end_row();
                                                            }
                                                        }
                                                    });
                                            } else {
                                                // List
                                                for (idx, entry) in
                                                    self.app_state.recent_files.iter().enumerate()
                                                {
                                                    let row = egui::Frame::NONE
                                                        .fill(rgb(self.theme.bg_panel))
                                                        .stroke(egui::Stroke::new(
                                                            1.0,
                                                            rgb(self.theme.border),
                                                        ))
                                                        .corner_radius(egui::CornerRadius::same(6))
                                                        .inner_margin(egui::Margin::symmetric(
                                                            10, 6,
                                                        ))
                                                        .show(ui, |ui| {
                                                            let mut remove_clicked = false;
                                                            let mut close_rect = egui::Rect::NOTHING;

                                                            ui.horizontal(|ui| {
                                                                let row_h = 32.0;
                                                                ui.set_min_height(row_h);

                                                                ui.allocate_ui_with_layout(
                                                                    egui::vec2(36.0, row_h),
                                                                    egui::Layout::centered_and_justified(
                                                                        egui::Direction::TopDown,
                                                                    ),
                                                                    |thumb_ui| {
                                                                        if let Some((
                                                                            texture,
                                                                            (w, h),
                                                                        )) = self
                                                                            .home_thumb_cache
                                                                            .get(&entry.path)
                                                                        {
                                                                            let max_w = 34.0_f32;
                                                                            let max_h = 30.0_f32;
                                                                            let scale = (max_w
                                                                                / (*w as f32)
                                                                                    .max(1.0))
                                                                            .min(
                                                                                max_h
                                                                                    / (*h as f32)
                                                                                        .max(1.0),
                                                                            );
                                                                            let size = egui::vec2(
                                                                                *w as f32 * scale,
                                                                                *h as f32 * scale,
                                                                            );
                                                                            thumb_ui.add(
                                                                                egui::Image::new((
                                                                                    texture.id(),
                                                                                    size,
                                                                                )),
                                                                            );
                                                                        } else if self
                                                                            .home_thumb_pending
                                                                            .contains(&entry.path)
                                                                        {
                                                                            thumb_ui.spinner();
                                                                        }
                                                                    },
                                                                );

                                                                ui.add_space(6.0);

                                                                ui.add(
                                                                    egui::Label::new(
                                                                        egui::RichText::new(
                                                                            truncate_filename(
                                                                                &entry.name,
                                                                                48,
                                                                            ),
                                                                        )
                                                                        .size(12.5)
                                                                        .strong()
                                                                        .color(rgb(
                                                                            self.theme.fg_primary,
                                                                        )),
                                                                    )
                                                                    .truncate(),
                                                                );

                                                                ui.add_space(8.0);

                                                                ui.label(
                                                                    egui::RichText::new(format!(
                                                                        "Page {} / {}",
                                                                        entry.last_page,
                                                                        entry.total_pages
                                                                    ))
                                                                    .size(10.0)
                                                                    .color(rgb(
                                                                        self.theme.fg_muted,
                                                                    )),
                                                                );

                                                                ui.with_layout(
                                                                    egui::Layout::right_to_left(
                                                                        egui::Align::Center,
                                                                    ),
                                                                    |ui| {
                                                                        let btn = ui
                                                                            .small_button("×")
                                                                            .on_hover_text(
                                                                                "Remove from history",
                                                                            )
                                                                            .on_hover_cursor(
                                                                                egui::CursorIcon::PointingHand,
                                                                            );
                                                                        close_rect = btn.rect;
                                                                        if btn.clicked() {
                                                                            remove_clicked = true;
                                                                        }
                                                                    },
                                                                );
                                                            });

                                                            (remove_clicked, close_rect)
                                                        });

                                                    let row_rect = row.response.rect;
                                                    let (remove_clicked, close_rect) = row.inner;
                                                    let on_close =
                                                        ui.rect_contains_pointer(close_rect);

                                                    if remove_clicked {
                                                        file_to_remove = Some(entry.path.clone());
                                                    } else if !on_close {
                                                        let row_resp = ui
                                                            .interact(
                                                                row_rect,
                                                                ui.id()
                                                                    .with(("recent_row", idx)),
                                                                egui::Sense::click(),
                                                            )
                                                            .on_hover_text(
                                                                entry.path.to_string_lossy(),
                                                            )
                                                            .on_hover_cursor(
                                                                egui::CursorIcon::PointingHand,
                                                            );

                                                        if row_resp.clicked() {
                                                            file_to_open = Some(entry.path.clone());
                                                        }
                                                    }

                                                    if ui.rect_contains_pointer(row_rect) {
                                                        ui.painter().rect_stroke(
                                                            row_rect,
                                                            egui::CornerRadius::same(6),
                                                            egui::Stroke::new(
                                                                1.0,
                                                                rgb(self.theme.accent),
                                                            ),
                                                            egui::StrokeKind::Inside,
                                                        );
                                                    }
                                                    ui.add_space(4.0);
                                                }
                                            }
                                        });
                                }

                                ui.add_space(32.0);
                            });
                    
                    
                    // 4. Actions
                    if let Some(p) = file_to_remove {
                        self.app_state.remove_recent(&p);
                    }

                    if let Some(p) = file_to_open {
                        self.open_pdf_user(p, ui.ctx());
                    }

                    return;
                }
                let visible_w = ui.available_width();
                let visible_h = ui.available_height();

                let page_w = self.page_dims.0 as f32;
                let page_h = self.page_dims.1 as f32;
                let (base_w, base_h) = if page_w > 0.0 && page_h > 0.0 {
                    (page_w, page_h)
                } else {
                    (800.0, 600.0)
                };
                let aspect_ratio = base_h / base_w;

                let effective_zoom = if in_slideshow {
                    ZoomMode::FitPage
                } else {
                    self.zoom_mode
                };

                const EDGE_PAD: f32 = 2.0;
                let (display_w, display_h) = match effective_zoom {
                    ZoomMode::Custom => {
                        let w = (visible_w - EDGE_PAD).max(100.0) * self.zoom_scale;
                        (w, w * aspect_ratio)
                    }
                    ZoomMode::FitWidth => {
                        let w = (visible_w - EDGE_PAD).max(100.0);
                        (w, w * aspect_ratio)
                    }
                    ZoomMode::FitHeight => {
                        let h = (visible_h - EDGE_PAD).max(100.0);
                        (h / aspect_ratio, h)
                    }
                    ZoomMode::FitPage => {
                        let max_w = (visible_w - EDGE_PAD).max(100.0);
                        let max_h = (visible_h - EDGE_PAD).max(100.0);
                        let scale = (max_w / base_w).min(max_h / base_h);
                        (base_w * scale, base_h * scale)
                    }
                };

                self.display_w_pt = display_w;
                self.last_effective_zoom = display_w / (visible_w - EDGE_PAD).max(1.0);

                // Keep the cursor or page center anchored during zoom.
            let anchor_screen = self.zoom_anchor.take(); // Consume the anchor even when zoom is unchanged.
            let zoom_now = (self.zoom_mode, self.zoom_scale);
            let zoom_changed = !in_slideshow
                && (zoom_now.0 != self.zoom_track.0 || (zoom_now.1 - self.zoom_track.1).abs() > 1e-4);
            let old_layout = self.last_layout;
            self.zoom_track = zoom_now;
            self.last_layout = (display_w, display_h, visible_w, visible_h);

            // None means no zoom change.
            // Some((Some(cursor_rel), old_layout)) means cursor-anchored zoom.
            // Some((None, old_layout)) means a fit-mode or UI zoom change.
            let zoom_fix: Option<(Option<egui::Vec2>, (f32, f32, f32, f32))> =
                if zoom_changed && old_layout.0 > 1.0 && old_layout.1 > 1.0 {   
                    let cursor_anchor = anchor_screen
                        .filter(|p| self.canvas_rect.contains(*p))
                        .map(|p| p - self.canvas_rect.min);

                    Some((cursor_anchor, old_layout))
                } else {
                    None
                };

            // Middle-button panning follows the pointer.
            let (mid_pressed, mid_down, ptr_delta, ptr_pos) = ui.input(|i| {
                (
                    i.pointer.button_pressed(egui::PointerButton::Middle),
                    i.pointer.button_down(egui::PointerButton::Middle),
                    i.pointer.delta(),
                    i.pointer.hover_pos().or(i.pointer.latest_pos()),
                )
            });
            if mid_pressed && !in_slideshow && ptr_pos.map_or(false, |p| self.canvas_rect.contains(p)) {
                self.pdf_panning = true;
            }
            if !mid_down {
                self.pdf_panning = false;
            }
            let pan_speed = if self.view_mode == ViewMode::Continuous { 4.8 } else { 3.5 };
            let pan_delta = if self.pdf_panning {
                ptr_delta * pan_speed
            } else {
                egui::Vec2::ZERO
            };

                if self.pdf_panning {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                    ui.ctx().request_repaint();
                }

                // Floating Exit button in pure Slideshow mode when mouse is near top
                if self.view_mode == ViewMode::Slideshow {
                    let show_hud = ui
                        .input(|i| i.pointer.hover_pos())
                        .map(|p| p.y < 60.0)
                        .unwrap_or(false);
                    if show_hud {
                        let theme = &self.theme;

                        let to_color = |rgb: [u8; 3]| {
                            egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2])
                        };

                        egui::Area::new(ui.make_persistent_id("slideshow_hud"))
                            .anchor(egui::Align2::CENTER_TOP, egui::vec2(-20.0, 20.0))
                            .order(egui::Order::Foreground)
                            .show(ui.ctx(), |ui| {
                                egui::Frame::NONE
                                    .fill(to_color(theme.bg_elevated))
                                    .corner_radius(20)
                                    .stroke(egui::Stroke::new(
                                        1.0,
                                        to_color(theme.border),
                                    ))
                                    .inner_margin(egui::Margin::symmetric(14, 6))
                                    .shadow(egui::epaint::Shadow {
                                        offset: [0, 4],
                                        blur: 12,
                                        spread: 0,
                                        color: egui::Color32::from_black_alpha(100),
                                    })
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 10.0;

                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "Slide {} / {}",
                                                    self.current_page,
                                                    self.total_pages
                                                ))
                                                .color(to_color(theme.fg_primary))
                                                .strong(),
                                            );

                                            ui.separator();

                                            ui.scope(|ui| {
                                                let visuals = &mut ui.style_mut().visuals.widgets;

                                                visuals.hovered.weak_bg_fill =
                                                    to_color(theme.bg_hover);
                                                visuals.hovered.fg_stroke =
                                                    egui::Stroke::new(
                                                        1.0,
                                                        to_color(theme.accent),
                                                    );
                                                visuals.hovered.corner_radius =
                                                    egui::CornerRadius::same(12);

                                                visuals.active.weak_bg_fill =
                                                    to_color(theme.bg_active);
                                                visuals.active.fg_stroke =
                                                    egui::Stroke::new(
                                                        1.0,
                                                        to_color(theme.accent),
                                                    );
                                                visuals.active.corner_radius =
                                                    egui::CornerRadius::same(12);

                                                let exit_btn = egui::Button::new(
                                                    egui::RichText::new("󰅖 Exit")
                                                        .size(13.0)
                                                        .strong()
                                                        .color(to_color(theme.accent)),
                                                )
                                                .frame(false)
                                                .corner_radius(12);

                                                if ui
                                                    .add(exit_btn)
                                                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                                                    .on_hover_text("Press Esc to exit")
                                                    .clicked()
                                                {
                                                    self.view_mode = self.prev_view_mode;
                                                }
                                            });
                                        });
                                    });
                            });
                    }
                    
                }

                else if self.view_mode == ViewMode::Continuous {
    let hover_in_canvas = ui
        .input(|i| i.pointer.hover_pos())
        .map_or(false, |p| self.canvas_rect.contains(p));
    if hover_in_canvas && !self.pdf_panning {
        ui.input_mut(|i| {
            if !i.modifiers.ctrl && !i.modifiers.command {
                const CONTINUOUS_WHEEL_MULT: f32 = 2.2;
                i.smooth_scroll_delta *= CONTINUOUS_WHEEL_MULT;
            }
        });
    }
    ui.spacing_mut().item_spacing.y = 0.0;
    let gap = 14.0;
    let row_h = display_h + gap;
    let visible_doc_pages: Vec<u32> = (1..=self.total_pages)
        .filter(|p| !self.hidden_pages.contains(p))
        .collect();

    if visible_doc_pages.is_empty() {
        return;
    }
    let total_rows = visible_doc_pages.len();
    let row_pitch = row_h;
    let visible_w = ui.available_width();
    let visible_h = ui.available_height();

    let approx_viewport_center_y = ui.max_rect().top() + visible_h * 0.5;

    let mut best_page_by_dist: Option<(u32, f32)> = None;
    let mut vis_range: Option<(u32, u32)> = None;

    let scroll_src = if self.pdf_panning {
        ScrollSource {
            drag: DragScroll::Never,
            scroll_bar: true,
            mouse_wheel: true,
        }
    } else {
        ScrollSource::ALL
    };

    let mut scroll_area = egui::ScrollArea::both()
        .id_salt("pdf_continuous_scroll")
        .auto_shrink([false, false])
        .scroll_source(scroll_src);

    let max_scroll_y = (total_rows as f32 * row_pitch - visible_h).max(0.0);

    // The previous bottom-of-document special case was removed.
    

    let just_jumped = self.continuous_scroll_to_page.is_some();

    if let Some(target_p) = self.continuous_scroll_to_page.take() {
        let row_idx = visible_doc_pages
            .iter()
            .position(|&p| p >= target_p)
            .unwrap_or(0);

        let offset_y = ((row_idx as f32) * row_pitch).clamp(0.0, max_scroll_y);
        scroll_area = scroll_area.vertical_scroll_offset(offset_y);

        self.jump_lock_y = Some(offset_y);
        self.jump_lock_frame = ui.ctx().cumulative_frame_nr();
        self.reanchor_left = 0; // Explicit navigation cancels pending reanchoring.
    }

    // Reanchor after zoom, resize, or fit-mode changes.
    let row_layout_changed = (row_h - self.continuous_prev_layout.2).abs() > 0.5
        || (display_w - self.continuous_prev_layout.0).abs() > 0.5
        || (display_h - self.continuous_prev_layout.1).abs() > 0.5;


    let needs_reanchor = !just_jumped && (zoom_fix.is_some() || row_layout_changed);

    if needs_reanchor {

    let from_zoom = zoom_fix.is_some();
    let cursor_y = zoom_fix.as_ref().and_then(|(a, _)| a.map(|a| a.y));

    if let (Some(cy), Some((_, old))) = (cursor_y, zoom_fix.as_ref()) {
        // Point under the cursor in the previous layout.
        let old_display_h = old.1.max(1.0);
        let old_pitch = old_display_h + gap;
        let doc_y = self.pdf_scroll_offset.y + cy;
        let idx = ((doc_y / old_pitch).floor() as isize)
            .clamp(0, total_rows as isize - 1) as usize;
        self.reanchor_page = visible_doc_pages[idx];
        self.reanchor_frac =
            ((doc_y - idx as f32 * old_pitch) / old_display_h).clamp(0.0, 1.0);
        self.reanchor_ref_y = cy;
    } else if from_zoom {
        // Fit, 100%, and keyboard zoom place the current page at the top.
        self.reanchor_page = self.current_page;
        self.reanchor_frac = 0.0;
        self.reanchor_ref_y = 0.0;
    } else {
        // Keep the current page at the top after resize.
        self.reanchor_page = self.continuous_anchor_page;
        self.reanchor_frac = self.continuous_anchor_frac;
        self.reanchor_ref_y = 0.0;
    }
    self.reanchor_left = 6;
    self.scroll_epoch = self.scroll_epoch.wrapping_add(1);
    }
        let user_scrolling = ui.input(|i| {
        let zooming = i.modifiers.ctrl || i.modifiers.command;
        (!zooming && i.smooth_scroll_delta.y.abs() > 0.5)
            || i.key_pressed(egui::Key::ArrowUp)
            || i.key_pressed(egui::Key::ArrowDown)
    }) || pan_delta != egui::Vec2::ZERO;

    if user_scrolling && !needs_reanchor {
        self.reanchor_left = 0;
        self.jump_lock_y = None;
    }
    // Keep the anchor locked throughout reanchoring.
    let reanchoring = self.reanchor_left > 0 && !just_jumped;
    let mut reanchor_desired: Option<f32> = None;
    if reanchoring {
        let new_row_idx = visible_doc_pages
            .iter()
            .position(|&p| p >= self.reanchor_page)
            .unwrap_or(visible_doc_pages.len() - 1);
        let new_offset_y = ((new_row_idx as f32) * row_pitch
            + self.reanchor_frac * display_h
            - self.reanchor_ref_y)
            .clamp(0.0, max_scroll_y);

        reanchor_desired = Some(new_offset_y);   // FUORI da qualsiasi if
        scroll_area = scroll_area.vertical_scroll_offset(new_offset_y);


        if self.reanchor_left == 6 {
            // Apply horizontal positioning only on the first frame.
            let anchor_x = zoom_fix
                .as_ref()
                .and_then(|(a, _)| a.map(|a| a.x))
                .unwrap_or(visible_w * 0.5);
            let (old_vis_w, old_disp_w) = zoom_fix
                .as_ref()
                .map(|(_, old)| (old.2, old.0))
                .unwrap_or((visible_w, display_w));
            let new_offset_x = zoom_scroll_offset(
                self.pdf_scroll_offset.x,
                anchor_x,
                old_vis_w,
                visible_w,
                old_disp_w,
                display_w,
            );
            reanchor_desired = Some(new_offset_y);
            scroll_area = scroll_area.horizontal_scroll_offset(new_offset_x);
        }

        self.pdf_scroll_offset.y = new_offset_y;
        self.jump_lock_y = Some(new_offset_y);
        self.jump_lock_frame = ui.ctx().cumulative_frame_nr();
        ui.ctx().request_repaint();
    }

    // Save the layout snapshot for the next frame.
    self.continuous_prev_layout = (display_w, display_h, row_h);
    
    scroll_area = scroll_area.id_salt(("pdf_continuous_scroll", self.scroll_epoch));
    let out = scroll_area.show_rows(ui, row_h, total_rows, |ui, row_range| {
        if pan_delta != egui::Vec2::ZERO {
            ui.scroll_with_delta(pan_delta);
            self.jump_lock_y = None;
            self.reanchor_left = 0;
        }

        if self.continuous_scroll_delta.abs() > 0.1 {
            ui.scroll_with_delta(egui::vec2(0.0, self.continuous_scroll_delta));
            self.continuous_scroll_delta = 0.0;
            self.jump_lock_y = None;
            self.reanchor_left = 0;
        }

        if !row_range.is_empty() {
            let first_p = visible_doc_pages[row_range.start];
            let last_p = visible_doc_pages[row_range.end.saturating_sub(1)];
            vis_range = Some((first_p, last_p));
        }

        for idx in row_range {
            let p = visible_doc_pages[idx];
            let margin = ((visible_w - display_w) * 0.5).max(0.0);

            ui.horizontal(|ui| {
                if margin > 0.0 {
                    ui.add_space(margin);
                }

                let sense = egui::Sense::click_and_drag();
                let (slide_rect, slide_resp) =
                    ui.allocate_exact_size(egui::vec2(display_w, display_h), sense);

                let uv = egui::Rect::from_min_max(
                    egui::pos2(0.0, 0.0),
                    egui::pos2(1.0, 1.0),
                );
                if let Some((handle, _)) = self.page_cache.get(&p) {
                    ui.painter()
                        .image(handle.id(), slide_rect, uv, egui::Color32::WHITE);
                } else if let Some((thumb_h, _)) = self.thumb_cache.get(&p) {
                    ui.painter()
                        .image(thumb_h.id(), slide_rect, uv, egui::Color32::WHITE);
                } else {
                    ui.painter().rect_filled(
                        slide_rect,
                        egui::CornerRadius::same(2),
                        egui::Color32::WHITE,
                    );
                }

                self.handle_slide_annotations(ui, p, slide_rect, &slide_resp);

                let dist_to_center =
                    (slide_rect.center().y - approx_viewport_center_y).abs();
                if best_page_by_dist
                    .map(|(_, best_d)| dist_to_center < best_d)
                    .unwrap_or(true)
                {
                    best_page_by_dist = Some((p, dist_to_center));
                }
            });

            ui.add_space(gap);
        }
    });

    self.canvas_rect = out.inner_rect;


    self.pdf_scroll_offset = clamp_scroll_offset(
        out.state.clamped_offset(),
        out.content_size,
        out.inner_rect.size(),
    );

    if let Some(r) = vis_range {
        self.visible_pages = r;
    }

    let actual_y = out.state.clamped_offset().y;

    if reanchoring {
        let real_max = (out.content_size.y - out.inner_rect.height()).max(0.0);
        let expected_h = total_rows as f32 * row_pitch;
        let content_ok = (out.content_size.y - expected_h).abs() < 2.0;
        let at_target = reanchor_desired
            .map_or(true, |d| (actual_y - d.min(real_max)).abs() < 40.0);
        if content_ok && at_target {
            self.reanchor_left = 0;
        } else {
            self.reanchor_left -= 1;
            ui.ctx().request_repaint();
        }
    }
    let gesture_active = ui.input(|i| {
        i.pointer.any_down() || i.smooth_scroll_delta.y.abs() > 0.5
    });
    let frames_since = ui
        .ctx()
        .cumulative_frame_nr()
        .saturating_sub(self.jump_lock_frame);

    if !reanchoring {
        // 1. Keep the scroll lock until the user scrolls.
        if let Some(lock_y) = self.jump_lock_y {
            if frames_since <= 1 {
                self.jump_lock_y = Some(actual_y);
            } else if gesture_active || (actual_y - lock_y).abs() > 1.5 {
                self.jump_lock_y = None;
            }
        }

        // 2. Keep the top-page anchor and fraction updated.
        let row_idx = ((actual_y / row_pitch.max(1.0)).floor() as isize)
            .clamp(0, total_rows as isize - 1) as usize;
        self.continuous_anchor_page = visible_doc_pages[row_idx];
        self.continuous_anchor_frac =
            ((actual_y - row_idx as f32 * row_pitch) / display_h.max(1.0)).clamp(0.0, 1.0);
    }

    // 3. At the bottom, select the last page;
    // otherwise select the page nearest the center.
    if !reanchoring
        && !just_jumped
        && self.jump_lock_y.is_none()
        && self.continuous_scroll_to_page.is_none()
    {
        let max_y = (out.content_size.y - out.inner_rect.height()).max(0.0);
        let at_bottom = max_y > 0.0 && actual_y >= max_y - 2.0;
        let candidate = if at_bottom {
            visible_doc_pages.last().copied()
        } else {
            best_page_by_dist.map(|(p, _)| p)
        };

        if let Some(vis_p) = candidate {
            if vis_p != self.current_page && (1..=self.total_pages).contains(&vis_p) {
                self.sync_and_save();
                self.current_page = vis_p;
                self.target_page = vis_p;
                if !self.lock_note_page {
                    self.note_page = vis_p;
                    self.current_note = self.notes.get(&vis_p).cloned().unwrap_or_default();
                    self.note_is_dirty = false;
                }
                self.selected_textbox = None;
            }
        }
    }
}
                else if self.view_mode == ViewMode::TwoPage {
                    let (left_opt, right_opt) = self.current_book_spread();
                    let spine_gap = 2.0;

                    let (book_page_w, book_page_h) = match self.zoom_mode {
                        ZoomMode::Custom => {
                            let w = ((visible_w - 4.0 - spine_gap) * 0.5).max(80.0) * self.zoom_scale;
                            (w, w * aspect_ratio)
                        }
                        ZoomMode::FitWidth => {
                            let w = ((visible_w - 4.0 - spine_gap) * 0.5).max(80.0);
                            (w, w * aspect_ratio)
                        }
                        ZoomMode::FitHeight => {
                            let h = (visible_h - 4.0).max(80.0);
                            (h / aspect_ratio, h)
                        }
                        ZoomMode::FitPage => {
                            let max_single_w = ((visible_w - 4.0 - spine_gap) * 0.5).max(80.0);
                            let max_h = (visible_h - 4.0).max(80.0);
                            let scale = (max_single_w / base_w).min(max_h / base_h);
                            (base_w * scale, base_h * scale)
                        }
                    };

                    self.display_w_pt = book_page_w;

                    match (left_opt, right_opt) {
                        (Some(l), Some(r)) => self.visible_pages = (l, r),
                        (None, Some(r)) => self.visible_pages = (r, r),
                        (Some(l), None) => self.visible_pages = (l, l),
                        (None, None) => {}
                    }

                    let num_pages_shown = left_opt.iter().count() + right_opt.iter().count();
                    let total_spread_w = if num_pages_shown == 2 {
                        book_page_w * 2.0 + spine_gap
                    } else {
                        book_page_w
                    };

                    egui::ScrollArea::both()
                        .id_salt("pdf_book_twopage_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let pad_x = ((visible_w - total_spread_w) * 0.5).max(0.0);
                            let pad_y = ((visible_h - book_page_h) * 0.5).max(0.0);

                            if pad_y > 0.0 {
                                ui.add_space(pad_y);
                            }

                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                if pad_x > 0.0 {
                                    ui.add_space(pad_x);
                                }

                                let pages_in_spread = [left_opt, right_opt];
                                for (slot_idx, p_opt) in pages_in_spread.iter().enumerate() {
                                    let Some(p) = *p_opt else {
                                        continue;
                                    };

                                    // Spine
                                    if slot_idx == 1 && left_opt.is_some() {
                                        let (spine_rect, _) = ui.allocate_exact_size(
                                            egui::vec2(spine_gap, book_page_h),
                                            egui::Sense::hover(),
                                        );
                                        let center_x = spine_rect.center().x;
                                        ui.painter().line_segment(
                                            [
                                                egui::pos2(center_x, spine_rect.top() + 4.0),
                                                egui::pos2(center_x, spine_rect.bottom() - 4.0),
                                            ],
                                            egui::Stroke::new(
                                                2.0,
                                                egui::Color32::from_black_alpha(110),
                                            ),
                                        );
                                    }

                                    let sense = egui::Sense::click_and_drag();
                                    let (slide_rect, slide_resp) = ui.allocate_exact_size(
                                        egui::vec2(book_page_w, book_page_h),
                                        sense,
                                    );

                                    if slide_resp.clicked() && self.current_page != p {
                                        self.sync_and_save();
                                        self.current_page = p;
                                        self.target_page = p;
                                        if !self.lock_note_page {
                                            self.note_page = p;
                                            self.current_note =
                                                self.notes.get(&p).cloned().unwrap_or_default();
                                            self.note_is_dirty = false;
                                        }
                                    }

                                    if let Some((handle, _)) = self.page_cache.get(&p) {
                                        ui.painter().image(
                                            handle.id(),
                                            slide_rect,
                                            egui::Rect::from_min_max(
                                                egui::pos2(0.0, 0.0),
                                                egui::pos2(1.0, 1.0),
                                            ),
                                            egui::Color32::WHITE,
                                        );
                                    } else if let Some((thumb_h, _)) = self.thumb_cache.get(&p) {
                                        ui.painter().image(
                                            thumb_h.id(),
                                            slide_rect,
                                            egui::Rect::from_min_max(
                                                egui::pos2(0.0, 0.0),
                                                egui::pos2(1.0, 1.0),
                                            ),
                                            egui::Color32::WHITE,
                                        );
                                    } else {
                                        ui.painter().rect_filled(
                                            slide_rect,
                                            egui::CornerRadius::same(3),
                                            egui::Color32::WHITE,
                                        );
                                    }

                                    // Border for active
                                    let is_active_side = p == self.current_page;
                                    ui.painter().rect_stroke(
                                        slide_rect,
                                        egui::CornerRadius::same(2),
                                        egui::Stroke::new(
                                            if is_active_side { 1.5 } else { 1.0 },
                                            if is_active_side {
                                                rgb(self.theme.accent)
                                            } else {
                                                rgb(self.theme.border)
                                            },
                                        ),
                                        egui::StrokeKind::Inside,
                                    );

                                    self.handle_slide_annotations(ui, p, slide_rect, &slide_resp);
                                }
                            });
                        });
                    return;
                }
                if self.view_mode == ViewMode::SinglePage || self.view_mode == ViewMode::Slideshow || self.view_mode == ViewMode::SlideshowWithNotes  {
                    if let Some((handle, dims)) = self.page_cache.get(&self.current_page) {
                        self.texture_handle = Some(handle.clone());
                        self.page_dims = *dims;
                    }
                    
                    let mut canvas_scroll = egui::ScrollArea::both()
                        .id_salt("pdf_canvas_scroll")
                        .auto_shrink([false, false]);

                    if let Some((opt_anchor, old)) = zoom_fix {
                        // `old` = (display_w, display_h, visible_w, visible_h) of the previous frame.
                        // Cursor-anchored zoom (Ctrl + wheel or touchpad pinch) keeps the page
                        // point under the cursor fixed; fit modes and UI zoom keep the view center.
                        let anchor_y = opt_anchor.map(|a| a.y).unwrap_or(visible_h * 0.5);
                        let y = zoom_scroll_offset(
                            self.pdf_scroll_offset.y,
                            anchor_y,
                            old.3,
                            visible_h,
                            old.1,
                            display_h,
                        );

                        let anchor_x = opt_anchor.map(|a| a.x).unwrap_or(visible_w * 0.5);
                        let x = zoom_scroll_offset(
                            self.pdf_scroll_offset.x,
                            anchor_x,
                            old.2,
                            visible_w,
                            old.0,
                            display_w,
                        );

                        canvas_scroll = canvas_scroll.scroll_offset(egui::vec2(x, y));
                    }

                    let out = canvas_scroll.show_viewport(ui, |ui, viewport| {
                        if pan_delta != egui::Vec2::ZERO {
                            ui.scroll_with_delta(pan_delta);
                        }
                        self.pdf_scroll_offset = viewport.min.to_vec2(); 

                        if let Some(handle) = &self.texture_handle {
                            let sized = egui::load::SizedTexture::new(
                                handle.id(),
                                egui::vec2(display_w, display_h),
                            );

                            let sense = egui::Sense::click_and_drag();

                            // Compute horizontal and vertical padding.
                            let pad_x = ((visible_w - display_w) * 0.5).max(0.0);
                            let pad_y = ((visible_h - display_h) * 0.5).max(0.0);

                            if pad_y > 0.0 {
                                ui.add_space(pad_y);
                            }

                            let img_resp = ui
                                .horizontal(|ui| {
                                    if pad_x > 0.0 {
                                        ui.add_space(pad_x);
                                    }
                                    ui.add(egui::Image::from_texture(sized).sense(sense))
                                })
                                .inner;

                            let slide_rect = img_resp.rect;
                            self.handle_slide_annotations(
                                ui,
                                self.current_page,
                                slide_rect,
                                &img_resp,
                            );
                        }
                    });

                    // Needed to turn the pointer position into a canvas-relative zoom anchor.
                    self.canvas_rect = out.inner_rect;

                    self.pdf_scroll_offset = clamp_scroll_offset(
                        self.pdf_scroll_offset, 
                        out.content_size, 
                        out.inner_rect.size()
                    );
                }
            });

        
        // Search
        if self.show_search && self.has_pdf() {
            self.render_search_popup(ui);
        }

        //Command Palette
        if self.show_command_palette {
            self.render_command_palette(ui);
        }

        //Flashcards
        if self.show_flashcards_modal && self.has_pdf() {
            self.render_flashcards_modal(ui);
        }

        // Advanced Settings        
        if self.show_settings { self.render_settings_modal(ui); }

// Toast Notification
        let mut clear_toast = false;
        if let Some((msg, time)) = &self.toast_message {
            const TOTAL_DURATION: f32 = 3.0;
            const FADE_IN: f32 = 0.18;
            const FADE_START: f32 = 2.6;
            let elapsed = time.elapsed().as_secs_f32();

            if elapsed < TOTAL_DURATION {
                // Compute alpha with a quick fade-in and smooth fade-out.
                let alpha = if elapsed < FADE_IN {
                    egui::emath::remap_clamp(elapsed, 0.0..=FADE_IN, 0.0..=1.0)
                } else if elapsed >= FADE_START {
                    egui::emath::remap_clamp(elapsed, FADE_START..=TOTAL_DURATION, 1.0..=0.0)
                } else {
                    1.0
                };
                // Use cubic easing for smooth motion.
                let smooth_alpha = 1.0 - (1.0 - alpha).powi(3);

                // Repaint while progress and animations are active.
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(16));

                // Separate the Nerd Font icon from the text and choose its color.
                let (icon, clean_msg, accent_rgb) = if let Some(rest) = msg.strip_prefix("󰄬") {
                    ("󰄬", rest.trim_start(), rgb(self.theme.success))
                } else if let Some(rest) = msg.strip_prefix("󰅚") {
                    ("󰅚", rest.trim_start(), rgb(self.theme.error))
                } else if let Some(rest) = msg.strip_prefix("󰀪") {
                    ("󰀪", rest.trim_start(), rgb(self.theme.warning))
                } else {
                    ("󰋼", msg.as_str(), rgb(self.theme.warning))
                };

                let bg_color = rgb(self.theme.bg_panel).gamma_multiply(smooth_alpha * 0.98);
                let accent_color = accent_rgb.gamma_multiply(smooth_alpha);
                let badge_bg = accent_rgb.gamma_multiply(smooth_alpha * 0.16);
                let border_color = rgb(self.theme.border).gamma_multiply(smooth_alpha);
                let text_color = rgb(self.theme.fg_primary).gamma_multiply(smooth_alpha);
                let shadow_color = egui::Color32::from_black_alpha(88).gamma_multiply(smooth_alpha);

                // Slide-up in entrata e leggero slide-down in uscita
                let y_offset = -24.0 + (1.0 - smooth_alpha) * 12.0;

                egui::Area::new(ui.make_persistent_id("toast_notification"))
                    .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(24.0, y_offset))
                    .order(egui::Order::Foreground)
                    .interactable(false)
                    .show(ui.ctx(), |ui| {
                        let frame_resp = egui::Frame::NONE
                            .fill(bg_color)
                            .stroke(egui::Stroke::new(1.0, border_color))
                            .corner_radius(egui::CornerRadius::same(self.theme.corner_radius.saturating_add(3)))
                            .inner_margin(egui::Margin::symmetric(14, 9))
                            .shadow(egui::epaint::Shadow {
                                spread: 0,
                                blur: 16,
                                color: shadow_color,
                                offset: [0, 6],
                            })
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 12.0;

                                    // Circular icon badge.
                                    let (rect, _) = ui.allocate_exact_size(
                                        egui::vec2(26.0, 26.0),
                                        egui::Sense::hover(),
                                    );
                                    ui.painter().circle_filled(rect.center(), 13.0, badge_bg);
                                    ui.painter().text(
                                        rect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        icon,
                                        egui::FontId::proportional(15.0),
                                        accent_color,
                                    );

                                    // Message text.
                                    ui.label(
                                        egui::RichText::new(clean_msg)
                                            .color(text_color)
                                            .size(13.5)
                                            .strong(),
                                    );

                                    ui.add_space(4.0);
                                });
                            });

                        // Thin progress bar at the bottom of the toast.
                        let progress = (1.0 - (elapsed / TOTAL_DURATION)).clamp(0.0, 1.0);
                        let rect = frame_resp.response.rect;
                        let bar_margin = 10.0;
                        let max_bar_w = (rect.width() - bar_margin * 2.0).max(0.0);
                        let bar_rect = egui::Rect::from_min_size(
                            egui::pos2(rect.left() + bar_margin, rect.bottom() - 2.5),
                            egui::vec2(max_bar_w * progress, 2.0),
                        );
                        ui.painter().rect_filled(
                            bar_rect,
                            egui::CornerRadius::same(1),
                            accent_rgb.gamma_multiply(smooth_alpha * 0.65),
                        );
                    });
            } else {
                clear_toast = true;
            }
        }
        if clear_toast {
            self.toast_message = None;
        }

        let ctx = ui.ctx().clone();
        self.pump_render_requests(&ctx);
        
     if self.radial_menu_pos.is_some() && self.has_pdf() {
         self.render_radial_tools_menu(ui);
     }
    }
}

fn load_icon() -> Option<egui::IconData> {
    let img = image::load_from_memory(include_bytes!("../assets/icon.png"))
        .ok()?
        .to_rgba8();
    let (width, height) = img.dimensions();
    Some(egui::IconData { rgba: img.into_raw(), width, height })
}

fn main() -> eframe::Result<()> {
    // With windows_subsystem = "windows" and panic = "abort" a crash would be silent.
    std::panic::set_hook(Box::new(|info| {
        let _ = std::fs::write(paths::data_dir().join("crash.log"), info.to_string());
    }));

    let initial_pdf = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .filter(|p| p.exists());

    let mut viewport = egui::ViewportBuilder::default()
        .with_drag_and_drop(true)
        .with_inner_size([1200.0, 720.0])
        .with_min_inner_size([560.0, 400.0])
        .with_title("Lapis");
    if let Some(icon) = load_icon() {
        viewport = viewport.with_icon(icon);
    }
    let opts = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "Lapis",
        opts,
        Box::new(|cc| Ok(Box::new(Lapis::new(cc, initial_pdf)))),
    )
}

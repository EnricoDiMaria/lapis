use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use std::path::{Path, PathBuf};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum AnnotationKind {
    Highlight,
    Underline,
    Freehand,
    TextBox,
    Rect,
    Circle,
    Line,
    Arrow,
    Tape,
    Image,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageAnnotation {
    pub kind: AnnotationKind,
    pub color: [u8; 3],
    pub points: Vec<(f32, f32)>,
    #[serde(default)]
    pub text: String,
    #[serde(default = "default_textbox_size")]
    pub font_size: f32,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default = "default_annotation_stroke_width")]
    pub stroke_width: f32,
}

fn default_textbox_size() -> f32 {
    18.0
}

pub const DEFAULT_ANNOTATION_STROKE_WIDTH: f32 = 2.5;

/// Spacing (sketch units) of the ruled / grid / dotted sketch backgrounds.
pub const SKETCH_GRID_STEP: f32 = 28.0;

/// Version 1 sketches stored coordinates normalized to a 400 x (400 * aspect) sheet.
const LEGACY_SKETCH_W: f32 = 400.0;

fn default_sketch_paper() -> BlankPageStyle {
    BlankPageStyle::Blank
}

/// Free-hand scratchpad attached to one PDF page, shown in the notes panel ("Sketch" tab).
///
/// It is a plain background plus strokes on an unbounded canvas: coordinates are in sketch
/// units (1 unit = 1 px at 100 % zoom), measured from the top-left corner, so the drawing does
/// not depend on the size or position of the panel it was drawn in.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SketchPage {
    #[serde(default = "default_sketch_paper")]
    pub paper: BlankPageStyle,
    #[serde(default)]
    pub strokes: Vec<PageAnnotation>,
    /// Only present in version 1 files, whose strokes were normalized to a fixed-ratio sheet.
    /// Read once by [`SketchPage::migrate_legacy`] and never written back.
    #[serde(default, rename = "aspect", skip_serializing)]
    legacy_aspect: Option<f32>,
}

impl SketchPage {
    pub fn new(paper: BlankPageStyle) -> Self {
        Self { paper, strokes: Vec::new(), legacy_aspect: None }
    }

    /// Nothing worth saving: no strokes and the default background.
    pub fn is_default(&self) -> bool {
        self.strokes.is_empty() && self.paper == BlankPageStyle::Blank
    }

    /// Converts a version 1 sketch (normalized coordinates) to sketch units. Idempotent.
    pub fn migrate_legacy(&mut self) {
        let Some(aspect) = self.legacy_aspect.take() else {
            return;
        };
        let (sx, sy) = (LEGACY_SKETCH_W, LEGACY_SKETCH_W * aspect.clamp(0.3, 4.0));
        for ann in &mut self.strokes {
            for p in &mut ann.points {
                *p = (p.0 * sx, p.1 * sy);
            }
        }
    }
}

impl Default for SketchPage {
    fn default() -> Self {
        Self::new(BlankPageStyle::Blank)
    }
}

fn default_annotation_stroke_width() -> f32 {
    DEFAULT_ANNOTATION_STROKE_WIDTH
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DocData {
    #[serde(default)]
    pub notes: HashMap<u32, String>,
    #[serde(default)]
    pub annotations: HashMap<u32, Vec<PageAnnotation>>,
    #[serde(default)] 
    pub blank_pages: std::collections::HashSet<u32>,
    /// Per-page sketch sheets (notes panel "Sketch" tab).
    #[serde(default)]
    pub sketches: HashMap<u32, SketchPage>,
    #[serde(default)]
    pub source_path: Option<PathBuf>,
    /// Stable document identity (see docid.rs). Travels with the PDF.
    #[serde(default)]
    pub doc_id: Option<String>,
    /// Revision: ms since the epoch, strictly increasing along a chain of saves.
    /// 0 = data written by versions that did not track revisions.
    #[serde(default)]
    pub rev: u64,
}

const NOTES_START_TAG: &[u8] = b"\n---Lapis_NOTES_V1---\n";
const NOTES_END_TAG: &[u8] = b"\n---Lapis_NOTES_END---\n";

/// Read the PDF and separate the note data stored after %%EOF.
fn read_pdf_and_notes(pdf_path: &Path) -> io::Result<(Vec<u8>, Vec<u8>)> {
    let mut bytes = fs::read(pdf_path)?;
    let clean_len = bytes
        .windows(NOTES_START_TAG.len())
        .rposition(|w| w == NOTES_START_TAG)
        .unwrap_or(bytes.len());
    let tail = bytes.split_off(clean_len); // Note data, including its tag.
    Ok((bytes, tail))
}

/// Loads the PDF without the note block appended after %%EOF (used by the export).
pub(crate) fn load_clean_pdf(pdf_path: &Path) -> Result<lopdf::Document, String> {
    let (clean, _notes) = read_pdf_and_notes(pdf_path).map_err(|e| e.to_string())?;
    load_lopdf(&clean, pdf_path, "export")
}

/// Try lopdf in memory, then normalize once with MuPDF if needed.
fn load_lopdf(clean: &[u8], pdf_path: &Path, tag: &str) -> Result<lopdf::Document, String> {
    match lopdf::Document::load_mem(clean) {
        Ok(d) => Ok(d),
        Err(first_err) => {
            let norm_in = pdf_path.with_extension(format!("{tag}_norm_in.pdf"));
            let norm_out = pdf_path.with_extension(format!("{tag}_norm_out.pdf"));
            let normalized = (|| -> Option<Vec<u8>> {
                use mupdf::pdf::{PdfDocument, PdfWriteOptions};
                fs::write(&norm_in, clean).ok()?;
                let mu_doc = PdfDocument::open(norm_in.to_str()?).ok()?;
                let mut opts = PdfWriteOptions::default();
                opts.set_garbage(true);
                mu_doc.save_with_options(norm_out.to_str()?, opts).ok()?;
                fs::read(&norm_out).ok()
            })();
            let _ = fs::remove_file(&norm_in);
            let _ = fs::remove_file(&norm_out);
            normalized
                .and_then(|data| lopdf::Document::load_mem(&data).ok())
                .ok_or_else(|| format!("lopdf load error: {first_err}"))
        }
    }
}

/// Write the PDF and note data to a temporary file, then replace the original atomically.
fn save_pdf_with_notes(
    doc: &mut lopdf::Document,
    pdf_path: &Path,
    tag: &str,
    notes: &[u8],
) -> io::Result<()> {
    let tmp_path = pdf_path.with_extension(format!("{tag}_tmp.pdf"));
    let res = (|| -> io::Result<()> {
        let file = fs::File::create(&tmp_path)?;
        let mut w = BufWriter::with_capacity(1 << 20, file);
        doc.save_to(&mut w)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("lopdf save error: {e}")))?;
        w.write_all(notes)?;
        w.flush()
    })();
    res.and_then(|_| fs::rename(&tmp_path, pdf_path)).map_err(|e| {
        let _ = fs::remove_file(&tmp_path);
        e
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlankPageStyle { Ruled, Dotted, Grid, Blank }
impl Default for BlankPageStyle { fn default() -> Self { Self::Ruled } }

pub fn insert_blank_page_with_style_into_pdf(pdf_path: &Path, after_page: u32, style: BlankPageStyle) -> io::Result<u32> {
    use lopdf::{dictionary, Object, ObjectId, Stream};
    use std::fmt::Write as _;

    ensure_original_pdf_backup(pdf_path)?;

    fn obj_to_f32(o: &Object) -> Option<f32> {
        match *o {
            Object::Integer(i) => Some(i as f32),
            Object::Real(f) => Some(f),
            _ => None,
        }
    }

    fn find_inherited_array(
        doc: &lopdf::Document,
        mut id: ObjectId,
        key: &[u8],
    ) -> Option<Vec<Object>> {
        for _ in 0..16 {
            let dict = doc.get_object(id).ok()?.as_dict().ok()?;
            if let Ok(arr) = dict.get(key).and_then(|o| o.as_array()) {
                return Some(arr.clone());
            }
            id = dict.get(b"Parent").and_then(|o| o.as_reference()).ok()?;
        }
        None
    }

    fn find_rotation(doc: &lopdf::Document, mut id: ObjectId) -> i64 {
        for _ in 0..16 {
            let Ok(dict) = doc.get_object(id).and_then(|o| o.as_dict()) else {
                break;
            };
            if let Ok(r) = dict.get(b"Rotate").and_then(|o| o.as_i64()) {
                return r.rem_euclid(360);
            }
            let Ok(next) = dict.get(b"Parent").and_then(|o| o.as_reference()) else {
                break;
            };
            id = next;
        }
        0
    }

    fn find_parent_and_index(
        doc: &lopdf::Document,
        node_id: ObjectId,
        target_page_id: ObjectId,
        depth: u32,
    ) -> Option<(ObjectId, usize)> {
        if depth > 32 {
            return None;
        }
        let dict = doc.get_object(node_id).ok()?.as_dict().ok()?;
        let kids = dict.get(b"Kids").and_then(|o| o.as_array()).ok()?;
        for (idx, item) in kids.iter().enumerate() {
            if let Ok(kid_id) = item.as_reference() {
                if kid_id == target_page_id {
                    return Some((node_id, idx));
                }
                if let Some(found) = find_parent_and_index(doc, kid_id, target_page_id, depth + 1) {
                    return Some(found);
                }
            }
        }
        None
    }

    let (clean, notes) = read_pdf_and_notes(pdf_path)?;
    let mut doc = load_lopdf(&clean, pdf_path, "ins")
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    drop(clean);

    let pages = doc.get_pages();
    let total = pages.len() as u32;
    let ref_page_num = after_page.clamp(1, total.max(1));
    let ref_id = pages.get(&ref_page_num).copied();
    drop(pages);

    let catalog_pages_root = doc
        .catalog()
        .and_then(|cat| cat.get(b"Pages"))
        .and_then(|o| o.as_reference())
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

    let mut x0 = 0.0_f32;
    let mut y0 = 0.0_f32;
    let mut width = 960.0_f32;
    let mut height = 540.0_f32;

    if let Some(id) = ref_id {
        let mb = find_inherited_array(&doc, id, b"CropBox")
            .or_else(|| find_inherited_array(&doc, id, b"MediaBox"));
        if let Some(arr) = mb {
            if arr.len() == 4 {
                if let (Some(bx0), Some(by0), Some(bx1), Some(by1)) = (
                    obj_to_f32(&arr[0]),
                    obj_to_f32(&arr[1]),
                    obj_to_f32(&arr[2]),
                    obj_to_f32(&arr[3]),
                ) {
                    x0 = bx0.min(bx1);
                    y0 = by0.min(by1);
                    width = (bx1 - bx0).abs().max(72.0);
                    height = (by1 - by0).abs().max(72.0);
                }
            }
        }
        let rot = find_rotation(&doc, id);
        if rot == 90 || rot == 270 {
            std::mem::swap(&mut width, &mut height);
            x0 = 0.0;
            y0 = 0.0;
        }
    }

    let (target_parent_id, insert_pos) = ref_id
        .and_then(|rid| {
            find_parent_and_index(&doc, catalog_pages_root, rid, 0)
                .map(|(p, idx)| (p, Some(idx + 1)))
        })
        .unwrap_or((catalog_pages_root, None));

    let media_box = vec![
        Object::Real(x0),
        Object::Real(y0),
        Object::Real(x0 + width),
        Object::Real(y0 + height),
    ];

    let margin_x = width * 0.04;
    let top_margin = height * 0.08;
    let bottom_margin = height * 0.07;
    let usable_h = (height - top_margin - bottom_margin).max(20.0);
    let x_start = x0 + margin_x;
    let x_end = x0 + width - margin_x;
    let line_w = (height / 650.0).clamp(0.25, 0.75);
    let mut grid_ops = String::with_capacity(4096);
    match style {
        BlankPageStyle::Blank => {},
        BlankPageStyle::Ruled => {
            let step_y = usable_h / 18.0;
            let _ = write!(grid_ops, "q 0.86 0.89 0.94 RG {line_w:.2} w ");
            for i in 0..=18 {
                let y = (y0 + height - top_margin) - (i as f32) * step_y;
                let _ = write!(grid_ops, "{x_start:.2} {y:.2} m {x_end:.2} {y:.2} l S ");
            }
            grid_ops.push_str("Q");
        }
        BlankPageStyle::Grid => {
            // Keep the grid inside the page margins. The clip rectangle is inset by
            // half the stroke width, preventing the outer strokes from protruding
            // past the page boundary on both landscape and 4:3 documents.
            let step = (width.min(height) / 28.0).clamp(10.0, 24.0);
            let y_bottom = y0 + bottom_margin;
            let y_top = y0 + height - top_margin;
            let clip_inset = line_w * 0.75;
            let clip_x = x_start + clip_inset;
            let clip_y = y_bottom + clip_inset;
            let clip_w = (x_end - x_start - 2.0 * clip_inset).max(1.0);
            let clip_h = (y_top - y_bottom - 2.0 * clip_inset).max(1.0);
            let _ = write!(grid_ops,
                "q 0.88 0.90 0.93 RG {line_w:.2} w {clip_x:.2} {clip_y:.2} {clip_w:.2} {clip_h:.2} re W n "
            );
            let mut x = x_start + step * 0.5;
            while x < x_end {
                let _ = write!(grid_ops, "{x:.2} {y_bottom:.2} m {x:.2} {y_top:.2} l S ");
                x += step;
            }
            let mut y = y_bottom + step * 0.5;
            while y < y_top {
                let _ = write!(grid_ops, "{x_start:.2} {y:.2} m {x_end:.2} {y:.2} l S ");
                y += step;
            }
            grid_ops.push_str("Q");
        }
        BlankPageStyle::Dotted => {
            let step = (width / 32.0).clamp(10.0, 24.0);
            let dot = (height / 700.0).clamp(0.7, 1.3);
            let _ = write!(grid_ops, "q 0.68 0.72 0.78 rg ");
            let mut y = y0 + bottom_margin;
            while y <= y0 + height - top_margin {
                let mut x = x_start;
                while x <= x_end { let _ = write!(grid_ops, "{:.2} {:.2} {dot:.2} {dot:.2} re f ", x - dot * 0.5, y - dot * 0.5); x += step; }
                y += step;
            }
            grid_ops.push_str("Q");
        }
    }

    let content_id = doc.add_object(Stream::new(dictionary! {}, grid_ops.into_bytes()));

    let new_page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => target_parent_id,
        "MediaBox" => media_box.clone(),
        "CropBox" => media_box,
        "Rotate" => 0,
        "Contents" => content_id,
        "Resources" => dictionary! {},
    });

    if let Ok(parent_dict) = doc.get_object_mut(target_parent_id).and_then(|o| o.as_dict_mut()) {
        if let Ok(kids) = parent_dict.get_mut(b"Kids").and_then(|o| o.as_array_mut()) {
            let pos = insert_pos.unwrap_or(kids.len()).min(kids.len());
            kids.insert(pos, Object::Reference(new_page_id));
        }
    }

    let mut node = Some(target_parent_id);
    for _ in 0..32 {
        let Some(id) = node else { break };
        node = match doc.get_object_mut(id).and_then(|o| o.as_dict_mut()) {
            Ok(d) => {
                if let Ok(c) = d.get(b"Count").and_then(|o| o.as_i64()) {
                    d.set("Count", Object::Integer(c + 1));
                }
                d.get(b"Parent").and_then(|o| o.as_reference()).ok()
            }
            Err(_) => None,
        };
    }

    let actual_new_page_num = doc
        .get_pages()
        .into_iter()
        .find_map(|(p_num, oid)| if oid == new_page_id { Some(p_num) } else { None })
        .unwrap_or(ref_page_num + 1);

    save_pdf_with_notes(&mut doc, pdf_path, "ins", &notes)?;
    Ok(actual_new_page_num)
}

pub fn delete_page_from_pdf(pdf_path: &Path, page_to_delete: u32) -> io::Result<()> {
    ensure_original_pdf_backup(pdf_path)?;

    let (clean, notes) = read_pdf_and_notes(pdf_path)?;
    let mut doc = load_lopdf(&clean, pdf_path, "del")
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    drop(clean);

    let pages = doc.get_pages();
    if pages.len() <= 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Cannot delete the only remaining page of the document",
        ));
    }
    if !pages.contains_key(&page_to_delete) {
        return Ok(()); // Nothing to write for a missing page.
    }
    drop(pages);

    doc.delete_pages(&[page_to_delete]);
    let _ = doc.prune_objects(); // Remove orphaned objects and resources.

    save_pdf_with_notes(&mut doc, pdf_path, "del", &notes)
}

/// Move a page to a new 1-based position while preserving
/// page objects and their PDF links and destinations.
/// Flatten the /Pages tree and copy inherited attributes
/// to each page before reordering.
pub fn reorder_pages_in_pdf(pdf_path: &Path, from_page: u32, to_page: u32) -> io::Result<()> {
    use lopdf::{Object, ObjectId};

    if from_page == to_page {
        return Ok(());
    }
    ensure_original_pdf_backup(pdf_path)?;
    let (clean, notes) = read_pdf_and_notes(pdf_path)?;
    let mut doc = load_lopdf(&clean, pdf_path, "reorder")
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    drop(clean);

    let pages_map = doc.get_pages();
    let total = pages_map.len() as u32;
    if from_page == 0 || to_page == 0 || from_page > total || to_page > total {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "Page index out of range"));
    }
    let mut ordered: Vec<ObjectId> = (1..=total)
        .filter_map(|n| pages_map.get(&n).copied())
        .collect();
    drop(pages_map);
    if ordered.len() != total as usize {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid PDF page tree"));
    }
    let moved = ordered.remove((from_page - 1) as usize);
    ordered.insert((to_page - 1) as usize, moved);

    let root_id = doc.catalog()
        .and_then(|cat| cat.get(b"Pages"))
        .and_then(|o| o.as_reference())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

    // Collect the nearest inherited attributes for each page.
    const INHERITED: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];
    for &page_id in &ordered {
        let mut inherited: Vec<(&'static [u8], Object)> = Vec::new();
        let parent = doc.get_object(page_id)
            .and_then(|o| o.as_dict())
            .ok()
            .and_then(|d| d.get(b"Parent").and_then(|o| o.as_reference()).ok());
        for key in INHERITED {
            let present = doc.get_object(page_id)
                .and_then(|o| o.as_dict())
                .map(|d| d.has(key))
                .unwrap_or(false);
            if present { continue; }
            let mut cursor = parent;
            while let Some(id) = cursor {
                let dict = match doc.get_object(id).and_then(|o| o.as_dict()) {
                    Ok(d) => d,
                    Err(_) => break,
                };
                if let Ok(value) = dict.get(key) {
                    inherited.push((key, value.clone()));
                    break;
                }
                cursor = dict.get(b"Parent").and_then(|o| o.as_reference()).ok();
            }
        }
        let page_dict = doc.get_object_mut(page_id)
            .and_then(|o| o.as_dict_mut())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        for (key, value) in inherited { page_dict.set(key, value); }
        page_dict.set("Parent", Object::Reference(root_id));
    }

    let root = doc.get_object_mut(root_id)
        .and_then(|o| o.as_dict_mut())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    root.set("Kids", Object::Array(ordered.into_iter().map(Object::Reference).collect()));
    root.set("Count", Object::Integer(total as i64));

    save_pdf_with_notes(&mut doc, pdf_path, "reorder", &notes)
}

/// Notes backups: `<data_dir>/notes` (migrates the legacy `lapispdf/notes` folder on first use).
pub fn get_backup_dir() -> PathBuf {
    crate::paths::notes_backup_dir()
}

pub fn get_backup_path(pdf_path: &Path) -> PathBuf {
    get_backup_dir().join(format!("{}.json", crate::docid::resolve_key(pdf_path)))
}

pub fn parse_doc_data_bytes(bytes: &[u8]) -> Option<DocData> {
    let mut data: DocData = serde_json::from_slice(bytes).ok()?;
    
    data.notes.retain(|_, v| !v.trim().is_empty());
    data.annotations.retain(|_, v| !v.is_empty());
    for sketch in data.sketches.values_mut() {
        sketch.migrate_legacy();
    }
    data.sketches.retain(|_, s| !s.is_default());
    
    Some(data)
}

fn save_local_backup(pdf_path: &Path, data: &DocData) {
    if pdf_path.as_os_str().is_empty() {
      return;
    }
    let backup_path = get_backup_path(pdf_path);
    if data.notes.is_empty() && data.annotations.is_empty() {
        let _ = fs::remove_file(backup_path);
        return;
    }
    let mut data = data.clone();
    data.source_path = Some(
        dunce::canonicalize(pdf_path).unwrap_or_else(|_| pdf_path.to_path_buf()),
    );
    if let Ok(json) = serde_json::to_string_pretty(&data) {
        let _ = fs::write(backup_path, json);
    }
}


fn load_local_backup(pdf_path: &Path) -> Option<DocData> {
    if pdf_path.as_os_str().is_empty() {
        return None;
    }
    let backup_path = get_backup_path(pdf_path);
    let bytes = fs::read(backup_path).ok()?;
    parse_doc_data_bytes(&bytes)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn known_revs() -> &'static Mutex<HashMap<PathBuf, u64>> {
    static REVS: OnceLock<Mutex<HashMap<PathBuf, u64>>> = OnceLock::new();
    REVS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn remember_rev(pdf_path: &Path, rev: u64) {
    if let Ok(mut m) = known_revs().lock() {
        m.insert(pdf_path.to_path_buf(), rev);
    }
}

/// Next revision: never lower than the last one loaded/saved for this document, even if this
/// machine's clock is behind (keeps the order correct along a chain of edits across machines).
fn next_rev(pdf_path: &Path) -> u64 {
    let last = known_revs()
        .lock()
        .ok()
        .and_then(|m| m.get(pdf_path).copied())
        .unwrap_or_else(|| load_local_backup(pdf_path).map(|d| d.rev).unwrap_or(0));
    now_ms().max(last + 1)
}

fn same_content(a: &DocData, b: &DocData) -> bool {
    a.blank_pages == b.blank_pages
        && serde_json::to_value(&a.notes).ok() == serde_json::to_value(&b.notes).ok()
        && serde_json::to_value(&a.annotations).ok() == serde_json::to_value(&b.annotations).ok()
        && serde_json::to_value(&a.sketches).ok() == serde_json::to_value(&b.sketches).ok()
}

/// When one version wins over the other, the loser is kept (if different and not empty) as
/// `notes/<id>.conflict-<ms>.bak` (newest 5 kept) so a wrong choice never loses data.
/// The `.bak` extension keeps these files out of the storage manager list.
fn stash_conflict(key: &str, lost: &DocData, kept: &DocData) {
    if (lost.notes.is_empty() && lost.annotations.is_empty() && lost.sketches.is_empty())
        || same_content(lost, kept)
    {
        return;
    }
    let dir = get_backup_dir();
    if let Ok(json) = serde_json::to_string_pretty(lost) {
        let _ = fs::write(dir.join(format!("{key}.conflict-{}.bak", now_ms())), json);
    }
    let prefix = format!("{key}.conflict-");
    let mut all: Vec<PathBuf> = fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map_or(false, |n| n.starts_with(&prefix))
        })
        .collect();
    all.sort(); // timestamps with the same number of digits: lexicographic == chronological
    while all.len() > 5 {
        let _ = fs::remove_file(all.remove(0));
    }
}

/// Reads only the document id from the data appended after %%EOF (last 8 MB of the file).
pub(crate) fn read_trailing_doc_id(pdf_path: &Path) -> Option<String> {
    #[derive(Deserialize)]
    struct IdOnly {
        #[serde(default)]
        doc_id: Option<String>,
    }
    let mut f = File::open(pdf_path).ok()?;
    let len = f.metadata().ok()?.len();
    let take = len.min(8 * 1024 * 1024);
    f.seek(SeekFrom::Start(len - take)).ok()?;
    let mut buf = Vec::with_capacity(take as usize);
    f.read_to_end(&mut buf).ok()?;
    let start = buf
        .windows(NOTES_START_TAG.len())
        .rposition(|w| w == NOTES_START_TAG)?;
    let content_start = start + NOTES_START_TAG.len();
    let end = buf[content_start..]
        .windows(NOTES_END_TAG.len())
        .position(|w| w == NOTES_END_TAG)?;
    serde_json::from_slice::<IdOnly>(&buf[content_start..content_start + end])
        .ok()?
        .doc_id
}

/// Reads the whole note block appended after %%EOF.
fn read_trailing_doc_data(pdf_path: &Path) -> Option<DocData> {
    let mut file = File::open(pdf_path).ok()?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).ok()?;
    let start_pos = buffer
        .windows(NOTES_START_TAG.len())
        .rposition(|w| w == NOTES_START_TAG)?;
    let content_start = start_pos + NOTES_START_TAG.len();
    let end_offset = buffer[content_start..]
        .windows(NOTES_END_TAG.len())
        .position(|w| w == NOTES_END_TAG)?;
    parse_doc_data_bytes(&buffer[content_start..content_start + end_offset])
}

/// Copy of the data without machine-specific fields, for embedding in the PDF.
fn portable(data: &DocData) -> DocData {
    let mut d = data.clone();
    d.source_path = None;
    d
}

pub fn load_doc_data(pdf_path: &Path) -> DocData {
    if pdf_path.as_os_str().is_empty() {
        return DocData::default();
    }
    let key = crate::docid::resolve_key(pdf_path);

    // 1. Local backup (immune to PDF locks) and 2. note block appended to the PDF.
    let backup_opt = load_local_backup(pdf_path);
    let pdf_opt = read_trailing_doc_data(pdf_path);

    // Newest revision wins as a whole (so deletions propagate too).
    // Unversioned data (rev == 0, written by older versions) keeps the old rule:
    // backup wins and only the pages missing from it are taken from the PDF.
    // `heal` = the PDF is behind the local backup and must be rewritten.
    let (mut data, heal) = match (backup_opt, pdf_opt) {
        (Some(b), Some(p)) if b.rev > 0 && p.rev > 0 => {
            if p.rev > b.rev {
                stash_conflict(&key, &b, &p);
                (p, false)
            } else if b.rev > p.rev {
                stash_conflict(&key, &p, &b);
                (b, true)
            } else {
                (b, false)
            }
        }
        (Some(b), Some(p)) if b.rev == 0 && p.rev > 0 => {
            stash_conflict(&key, &b, &p);
            (p, false)
        }
        (Some(mut b), Some(p)) => {
            for (k, v) in p.notes {
                b.notes.entry(k).or_insert(v);
            }
            for (k, v) in p.annotations {
                b.annotations.entry(k).or_insert(v);
            }
            let upgrade = b.rev > 0; // PDF written by an older version: bring it up to date
            (b, upgrade)
        }
        (Some(b), None) => (b, true),
        (None, Some(p)) => (p, false),
        (None, None) => return DocData::default(),
    };

    data.doc_id = Some(key);
    remember_rev(pdf_path, data.rev);
    save_local_backup(pdf_path, &data);
    if heal {
        let _ = save_data_to_pdf_trailing(pdf_path, &portable(&data));
    }
    data
}

pub fn save_doc_data(
    pdf_path: &Path,
    notes: &HashMap<u32, String>,
    annotations: &HashMap<u32, Vec<PageAnnotation>>,
    blank_pages: std::collections::HashSet<u32>,
    sketches: &HashMap<u32, SketchPage>,
) -> io::Result<()> {
    if pdf_path.as_os_str().is_empty() {
        return Ok(());
    }

    let mut clean_notes = notes.clone();
    clean_notes.retain(|_, v| !v.trim().is_empty());

    let mut clean_anns = annotations.clone();
    clean_anns.retain(|_, v| !v.is_empty());

    let mut clean_pages: HashSet<u32> = blank_pages.clone();
    clean_pages.retain(|&v| v != 0);

    let mut clean_sketches = sketches.clone();
    clean_sketches.retain(|&k, s| k != 0 && !s.is_default());

    let rev = next_rev(pdf_path);
    let data = DocData {
        notes: clean_notes,
        annotations: clean_anns,
        blank_pages: clean_pages,
        sketches: clean_sketches,
        doc_id: Some(crate::docid::resolve_key(pdf_path)),
        rev,
        ..Default::default()
    };
    remember_rev(pdf_path, rev);

    // Always write the local JSON backup first!
    save_local_backup(pdf_path, &data);
    save_data_to_pdf_trailing(pdf_path, &data)
}

fn save_data_to_pdf_trailing(path: &Path, data: &DocData) -> io::Result<()> {
    let bytes = fs::read(path)?;

    let clean_len = if let Some(start_pos) = bytes
        .windows(NOTES_START_TAG.len())
        .rposition(|w| w == NOTES_START_TAG)
    {
        start_pos
    } else {
        bytes.len()
    };
    //If there are contents after the EOF they get truncated first and the rewritten
    let mut file = OpenOptions::new().write(true).read(true).open(path)?;
    file.set_len(clean_len as u64)?;
    file.seek(SeekFrom::End(0))?;

    if !data.notes.is_empty() || !data.annotations.is_empty() {
        let json_string = serde_json::to_string(data)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        file.write_all(NOTES_START_TAG)?;
        file.write_all(json_string.as_bytes())?;
        file.write_all(NOTES_END_TAG)?;
        file.flush()?;
    }

    Ok(())
}

/// Tries to parse a line as a slide/page header.
/// Recognizes patterns like:
/// Accept headers such as "SLIDE 1", "PAGE 1", "PAGINA 1", and "P. 1".
/// Examples: "# Slide 1", "## Page 2 - Title", and "[Slide 4]".
/// Returns `Some((page_number, optional_inline_text))` if matched.
fn parse_slide_header(line: &str) -> Option<(u32, Option<&str>)> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Avoid misinterpreting standard Markdown bullet lists or code blocks as headers
    if trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("```") {
        return None;
    }

    // Strip surrounding markdown/divider symbols (#, -, =, *, [, ]) without heap allocations
    let core = trimmed
        .trim_matches(|c: char| matches!(c, '#' | '-' | '=' | '*' | '[' | ']'))
        .trim();

    let prefixes = ["slide", "pagina", "page", "pag.", "pag", "sld", "p.", "p"];

    for kw in prefixes {
        let Some(head) = core.get(..kw.len()) else { continue }; // None se non è char boundary
        if core.len() <= kw.len() || !head.eq_ignore_ascii_case(kw) { continue; }
        let after_kw = &core[kw.len()..];

        // Must be followed by whitespace, '#', '.', ':' or '-' (prevents matching words like "point")
        let first_ch = after_kw.chars().next()?;
        if !first_ch.is_whitespace() && !matches!(first_ch, '#' | '.' | ':' | '-') {
            continue;
        }

        let rest = after_kw
            .trim_start_matches(|c: char| c.is_whitespace() || matches!(c, '#' | '.'))
            .trim_start();

        // Extract consecutive ASCII digits for the page number
        let digit_len = rest.bytes().take_while(|b| b.is_ascii_digit()).count();
        if digit_len == 0 {
            continue;
        }

        let page_num: u32 = rest[..digit_len].parse().ok()?;
        if page_num == 0 {
            return None;
        }

        let remainder = rest[digit_len..].trim();
        if remainder.is_empty() {
            return Some((page_num, None));
        }

        // If remainder only contains closing divider chars (e.g. "---"), ignore it
        let clean_rem = remainder
            .trim_start_matches(|c: char| matches!(c, ':' | '-' | '—' | '–' | ')' | ']'))
            .trim();

        if clean_rem.is_empty() {
            return Some((page_num, None));
        } else if remainder.starts_with(':')
            || remainder.starts_with('-')
            || remainder.starts_with('—')
            || remainder.starts_with('–')
        {
            // Inline note after a header (e.g. "Slide 3: Key concept").
            return Some((page_num, Some(clean_rem)));
        } else {
            // E.g. "P 1 qualcosa" without separator might be normal text unless line started with #
            if trimmed.starts_with('#') || kw.len() >= 4 {
                return Some((page_num, None));
            }
        }
    }

    None
}

fn parse_lapis_header(line: &str) -> Option<u32> {
    let rest = line.trim().strip_prefix('#')?.trim_start_matches('#').trim_start();
    rest.strip_prefix("Slide ")?.trim().parse().ok().filter(|&n| n > 0)
}

/// Imports notes from a `.txt` or `.md` file, mapping each section to its slide number.
/// `max_pages` bounds the page numbers so out-of-range slides are ignored safely.
pub fn import_notes_from_file(
    path: &Path,
    max_pages: u32,
) -> io::Result<HashMap<u32, String>> {
    let text = fs::read_to_string(path)?;
    let strict = text.lines().next().map_or(false, |l| l.starts_with("<!-- Notes exported from Lapis"));

    let mut imported: HashMap<u32, String> = HashMap::new();
    let mut current_page: Option<u32> = None;
    let mut current_buf = String::new();

    let flush_note = |page: Option<u32>, buf: &mut String, map: &mut HashMap<u32, String>| {
        if let Some(p) = page {
            let trimmed = buf.trim();
            if !trimmed.is_empty() && (max_pages == 0 || p <= max_pages) {
                map.entry(p)
                    .and_modify(|existing| {
                        existing.push_str("\n\n");
                        existing.push_str(trimmed);
                    })
                    .or_insert_with(|| trimmed.to_string());
            }
        }
        buf.clear();
    };

    for line in text.lines() {
        let hdr = if strict { parse_lapis_header(line).map(|n| (n, None)) } else { parse_slide_header(line) };
        if let Some((page_num, inline_text)) = hdr {
            flush_note(current_page, &mut current_buf, &mut imported);
            current_page = Some(page_num);
            if let Some(inline) = inline_text {
                current_buf.push_str(inline);
                current_buf.push('\n');
            }
        } else if current_page.is_some() {
            current_buf.push_str(line);
            current_buf.push('\n');
        }
    }

    flush_note(current_page, &mut current_buf, &mut imported);
    Ok(imported)
}

/// Exports all document notes into a single unified and sorted Markdown (`.md`) file.
pub fn export_raw_notes_md(
    output_path: &Path,
    pdf_name: &str,
    notes: &HashMap<u32, String>,
) -> io::Result<()> {
    let file = File::create(output_path)?;
    let mut writer = io::BufWriter::new(file);

    // Sort pages in ascending order
    let mut sorted_notes: Vec<(&u32, &String)> = notes
        .iter()
        .filter(|(_, text)| !text.trim().is_empty())
        .collect();
    sorted_notes.sort_unstable_by_key(|&(page, _)| page);

    writeln!(writer, "<!-- Notes exported from Lapis: {} -->\n", pdf_name)?;

    for (i, (page, content)) in sorted_notes.iter().enumerate() {
        if i > 0 {
            writeln!(writer)?;
        }
        writeln!(writer, "# Slide {}\n", page)?;
        writeln!(writer, "{}", content.trim())?;
    }

    writer.flush()
}

// Convert LaTeX expressions to Unicode math symbols, superscripts, and subscripts.
pub fn format_latex_expr(expr: &str) -> String {
    let mut s = expr.trim().to_string();

    // Convert \frac{a}{b} to (a/b).
    while let Some(pos) = s.find("\\frac{") {
        let after_num = &s[pos + 6..];
        if let Some(close_num) = after_num.find('}') {
            let num = &after_num[..close_num];
            let rest = &after_num[close_num + 1..];
            if let Some(after_den) = rest.strip_prefix('{') {
                if let Some(close_den) = after_den.find('}') {
                    let den = &after_den[..close_den];
                    let replacement = format!("({}/{})", num, den);
                    let total_len = 6 + close_num + 1 + 1 + close_den + 1;
                    s.replace_range(pos..pos + total_len, &replacement);
                    continue;
                }
            }
        }
        break;
    }

    // Convert \sqrt{x} to √(x).
    while let Some(pos) = s.find("\\sqrt{") {
        let after = &s[pos + 6..];
        if let Some(close) = after.find('}') {
            let inner = &after[..close];
            let replacement = format!("√({})", inner);
            s.replace_range(pos..pos + 6 + close + 1, &replacement);
        } else {
            break;
        }
    }

    // Convert common LaTeX text and font commands.
    for prefix in ["\\text{", "\\mathrm{", "\\mathbf{", "\\mathcal{"] {
        while let Some(pos) = s.find(prefix) {
            let plen = prefix.len();
            let after = &s[pos + plen..];
            if let Some(close) = after.find('}') {
                let inner = after[..close].to_string();
                s.replace_range(pos..pos + plen + close + 1, &inner);
            } else {
                break;
            }
        }
    }

    // Convert common LaTeX accents and vector commands.
    for (cmd, comb) in [
        ("\\overline{", '\u{0305}'),
        ("\\bar{", '\u{0304}'),
        ("\\vec{", '\u{20D7}'),
        ("\\hat{", '\u{0302}'),
        ("\\tilde{", '\u{0303}'),
        ("\\dot{", '\u{0307}'),
        ("\\ddot{", '\u{0308}'),
    ] {
        while let Some(pos) = s.find(cmd) {
            let clen = cmd.len();
            let after = &s[pos + clen..];
            if let Some(close) = after.find('}') {
                let inner = &after[..close];
                let mut rep = String::new();
                for ch in inner.chars() {
                    rep.push(ch);
                    rep.push(comb);
                }
                s.replace_range(pos..pos + clen + close + 1, &rep);
            } else {
                break;
            }
        }
    }

    const LATEX_SYMBOLS: &[(&str, &str)] = &[
        // Insiemi numerici (Blackboard bold)
        ("\\mathbb{R}", "ℝ"), ("\\mathbb{N}", "ℕ"), ("\\mathbb{Z}", "ℤ"),
        ("\\mathbb{Q}", "ℚ"), ("\\mathbb{C}", "ℂ"), ("\\mathbb{P}", "ℙ"),
        ("\\mathbb{E}", "𝔼"), ("\\mathbb{F}", "𝔽"), ("\\mathbb{1}", "𝟙"),
        ("\\Re", "ℜ"), ("\\Im", "ℑ"), ("\\ell", "ℓ"), ("\\hbar", "ℏ"),
        ("\\wp", "℘"), ("\\aleph", "ℵ"),
        // Frecce e logica
        ("\\Longleftrightarrow", "⟺"), ("\\Leftrightarrow", "⇔"),
        ("\\longrightarrow", "⟶"), ("\\Longrightarrow", "⟹"),
        ("\\longleftarrow", "⟵"), ("\\Longleftarrow", "⟸"),
        ("\\longmapsto", "⟼"), ("\\leftrightarrow", "↔"),
        ("\\rightleftharpoons", "⇌"), ("\\hookrightarrow", "↪"),
        ("\\hookleftarrow", "↩"), ("\\twoheadrightarrow", "↠"),
        ("\\rightarrow", "→"), ("\\Rightarrow", "⇒"),
        ("\\leftarrow", "←"), ("\\Leftarrow", "⇐"),
        ("\\uparrow", "↑"), ("\\Uparrow", "⇑"),
        ("\\downarrow", "↓"), ("\\Downarrow", "⇓"),
        ("\\updownarrow", "↕"), ("\\nearrow", "↗"),
        ("\\searrow", "↘"), ("\\swarrow", "↙"), ("\\nwarrow", "↖"),
        ("\\mapsto", "↦"), ("\\implies", "⇒"), ("\\impliedby", "⇐"),
        ("\\iff", "⇔"), ("\\to", "→"), ("\\gets", "←"),
        // Quantificatori e operatori logici
        ("\\forall", "∀"), ("\\exists", "∃"), ("\\nexists", "∄"),
        ("\\therefore", "∴"), ("\\because", "∵"),
        ("\\land", "∧"), ("\\wedge", "∧"), ("\\bigwedge", "⋀"),
        ("\\lor", "∨"), ("\\vee", "∨"), ("\\bigvee", "⋁"),
        ("\\neg", "¬"), ("\\lnot", "¬"), ("\\top", "⊤"), ("\\bot", "⊥"),
        ("\\vdash", "⊢"), ("\\models", "⊨"),
        // Insiemi e relazioni
        ("\\subseteq", "⊆"), ("\\supseteq", "⊇"),
        ("\\subsetneq", "⊊"), ("\\supsetneq", "⊋"),
        ("\\nsubseteq", "⊈"), ("\\nsupseteq", "⊉"),
        ("\\subset", "⊂"), ("\\supset", "⊃"),
        ("\\emptyset", "∅"), ("\\varnothing", "∅"),
        ("\\notin", "∉"), ("\\in", "∈"), ("\\ni", "∋"),
        ("\\bigcup", "⋃"), ("\\bigcap", "⋂"),
        ("\\bigsqcup", "⨆"), ("\\bigoplus", "⨁"), ("\\bigotimes", "⨂"),
        ("\\cup", "∪"), ("\\cap", "∩"), ("\\setminus", "∖"),
        // Analysis and calculation.
        ("\\iiint", "∭"), ("\\iint", "∬"), ("\\oint", "∮"), ("\\int", "∫"),
        ("\\sum", "∑"), ("\\prod", "∏"), ("\\coprod", "∐"),
        ("\\partial", "∂"), ("\\nabla", "∇"), ("\\infty", "∞"),
        ("\\infinity", "∞"), ("\\sqrt[3]", "∛"), ("\\sqrt[4]", "∜"), ("\\sqrt", "√"),
        ("\\lim", "lim"), ("\\sup", "sup"), ("\\inf", "inf"),
        ("\\max", "max"), ("\\min", "min"), ("\\det", "det"),
        ("\\dim", "dim"), ("\\ker", "ker"), ("\\deg", "deg"),
        ("\\sin", "sin"), ("\\cos", "cos"), ("\\tan", "tan"),
        ("\\arcsin", "arcsin"), ("\\arccos", "arccos"), ("\\arctan", "arctan"),
        ("\\sinh", "sinh"), ("\\cosh", "cosh"), ("\\tanh", "tanh"),
        ("\\log", "log"), ("\\ln", "ln"), ("\\exp", "exp"),
        // Confronti ed equivalenze
        ("\\approxeq", "≊"), ("\\approx", "≈"), ("\\simeq", "≃"),
        ("\\cong", "≅"), ("\\equiv", "≡"), ("\\not\\equiv", "≢"),
        ("\\propto", "∝"), ("\\sim", "∼"), ("\\asymp", "≍"),
        ("\\leqslant", "⩽"), ("\\geqslant", "⩾"),
        ("\\leq", "≤"), ("\\geq", "≥"), ("\\le", "≤"), ("\\ge", "≥"),
        ("\\neq", "≠"), ("\\ne", "≠"), ("\\ll", "≪"), ("\\gg", "≫"),
        ("\\prec", "≺"), ("\\succ", "≻"), ("\\preceq", "≼"), ("\\succeq", "≽"),
        ("\\perp", "⊥"), ("\\parallel", "∥"), ("\\mid", "∣"), ("\\nmid", "∤"),
        // Operatori binari e delimitatori
        ("\\otimes", "⊗"), ("\\oplus", "⊕"), ("\\ominus", "⊖"),
        ("\\odot", "⊙"), ("\\circ", "∘"), ("\\bullet", "∙"),
        ("\\times", "×"), ("\\div", "÷"), ("\\cdot", "·"),
        ("\\pm", "±"), ("\\mp", "∓"), ("\\star", "⋆"), ("\\ast", "∗"),
        ("\\langle", "⟨"), ("\\rangle", "⟩"),
        ("\\lceil", "⌈"), ("\\rceil", "⌉"),
        ("\\lfloor", "⌊"), ("\\rfloor", "⌋"),
        ("\\ldots", "…"), ("\\cdots", "⋯"), ("\\vdots", "⋮"), ("\\ddots", "⋱"),
        ("\\prime", "′"), ("\\degree", "°"), ("\\circ", "°"),
        ("\\triangle", "△"), ("\\square", "□"), ("\\diamond", "⋄"),
        ("\\angle", "∠"),
        // Alfabeto greco
        ("\\varepsilon", "ε"), ("\\epsilon", "ϵ"),
        ("\\vartheta", "ϑ"), ("\\theta", "θ"), ("\\Theta", "Θ"),
        ("\\varpi", "ϖ"), ("\\pi", "π"), ("\\Pi", "Π"),
        ("\\varrho", "ϱ"), ("\\rho", "ρ"),
        ("\\varsigma", "ς"), ("\\sigma", "σ"), ("\\Sigma", "Σ"),
        ("\\varphi", "φ"), ("\\phi", "ϕ"), ("\\Phi", "Φ"),
        ("\\alpha", "α"), ("\\beta", "β"), ("\\gamma", "γ"), ("\\Gamma", "Γ"),
        ("\\delta", "δ"), ("\\Delta", "Δ"), ("\\zeta", "ζ"), ("\\eta", "η"),
        ("\\iota", "ι"), ("\\kappa", "κ"), ("\\lambda", "λ"), ("\\Lambda", "Λ"),
        ("\\mu", "μ"), ("\\nu", "ν"), ("\\xi", "ξ"), ("\\Xi", "Ξ"),
        ("\\tau", "τ"), ("\\upsilon", "υ"), ("\\Upsilon", "Υ"),
        ("\\chi", "χ"), ("\\psi", "ψ"), ("\\Psi", "Ψ"),
        ("\\omega", "ω"), ("\\Omega", "Ω"),
        ("\\left(", "("), ("\\right)", ")"),
        ("\\left[", "["), ("\\right]", "]"),
        ("\\left\\{", "{"), ("\\right\\}", "}"),
        ("\\left|", "|"), ("\\right|", "|"),
        ("\\,", " "), ("\\;", " "), ("\\quad", "  "), ("\\qquad", "    "),
    ];

    for &(cmd, sym) in LATEX_SYMBOLS {
        s = s.replace(cmd, sym);
    }

    // Converte ^ e _ in apici e pedici Unicode
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;

    let to_super = |c: char| -> Option<char> {
        match c {
            '0' => Some('⁰'),
            '1' => Some('¹'),
            '2' => Some('²'),
            '3' => Some('³'),
            '4' => Some('⁴'),
            '5' => Some('⁵'),
            '6' => Some('⁶'),
            '7' => Some('⁷'),
            '8' => Some('⁸'),
            '9' => Some('⁹'),
            '+' => Some('⁺'),
            '-' => Some('⁻'),
            '=' => Some('⁼'),
            '(' => Some('⁽'),
            ')' => Some('⁾'),
            'n' => Some('ⁿ'),
            'i' => Some('ⁱ'),
            'T' => Some('ᵀ'),
            _ => None,
        }
    };

    let to_sub = |c: char| -> Option<char> {
        match c {
            '0' => Some('₀'),
            '1' => Some('₁'),
            '2' => Some('₂'),
            '3' => Some('₃'),
            '4' => Some('₄'),
            '5' => Some('₅'),
            '6' => Some('₆'),
            '7' => Some('₇'),
            '8' => Some('₈'),
            '9' => Some('₉'),
            '+' => Some('₊'),
            '-' => Some('₋'),
            '=' => Some('₌'),
            '(' => Some('₍'),
            ')' => Some('₎'),
            'a' => Some('ₐ'),
            'e' => Some('ₑ'),
            'i' => Some('ᵢ'),
            'j' => Some('ⱼ'),
            'k' => Some('ₖ'),
            'm' => Some('ₘ'),
            'n' => Some('ₙ'),
            'o' => Some('ₒ'),
            'p' => Some('ₚ'),
            'r' => Some('ᵣ'),
            's' => Some('ₛ'),
            't' => Some('ₜ'),
            'u' => Some('ᵤ'),
            'v' => Some('ᵥ'),
            'x' => Some('ₓ'),
            _ => None,
        }
    };

    while i < chars.len() {
        if (chars[i] == '^' || chars[i] == '_') && i + 1 < chars.len() {
            let is_sup = chars[i] == '^';
            if chars[i + 1] == '{' {
                if let Some(close_offset) = chars[i + 2..].iter().position(|&c| c == '}') {
                    let slice = &chars[i + 2..i + 2 + close_offset];
                    let mapped: Option<String> = slice
                        .iter()
                        .copied()
                        .map(|c| if is_sup { to_super(c) } else { to_sub(c) })
                        .collect();
                    if let Some(conv) = mapped {
                        out.push_str(&conv);
                    } else {
                        out.push(chars[i]);
                        out.push('(');
                        for &c in slice {
                            out.push(c);
                        }
                        out.push(')');
                    }
                    i += 2 + close_offset + 1;
                    continue;
                }
            } else {
                let next_c = chars[i + 1];
                if let Some(conv) = if is_sup {
                    to_super(next_c)
                } else {
                    to_sub(next_c)
                } {
                    out.push(conv);
                    i += 2;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }

    out.replace('{', "").replace('}', "").replace(':', "∶")
}

// Convert `Term :: Definition` lines into Markdown callouts.
// Ignore code blocks and LaTeX math blocks.
pub fn preprocess_definitions(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len() + 128);
    let mut lines = markdown.lines().peekable();
    let mut in_code_block = false;
    let mut in_math_block = false;
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") { in_code_block = !in_code_block; out.push_str(line); out.push('\n'); continue; }
        if trimmed.starts_with("$$") { if !trimmed[2..].contains("$$") { in_math_block = !in_math_block; } out.push_str(line); out.push('\n'); continue; }
        if !in_code_block && !in_math_block {
            let clean = trimmed.trim_start_matches(['-', '*', '+']).trim();
            let q_start = clean.strip_prefix("Q:").or_else(|| clean.strip_prefix("**Q:**")).or_else(|| clean.strip_prefix("D:"));
            if let Some(q) = q_start {
                let mut question = q.trim().to_string(); let mut answer = String::new(); let mut in_answer = false;
                while let Some(next) = lines.peek() {
                    let t = next.trim();
                    if t.is_empty() && in_answer { break; }
                    if t.starts_with("Q:") || t.starts_with("**Q:**") || t.starts_with("D:") || t.contains("::") { break; }
                    let consumed = lines.next().unwrap().trim();
                    if let Some(a) = consumed.strip_prefix("A:").or_else(|| consumed.strip_prefix("**A:**")).or_else(|| consumed.strip_prefix("R:")) { in_answer = true; answer.push_str(a.trim()); }
                    else if in_answer { if !answer.is_empty() { answer.push('\n'); } answer.push_str(consumed); }
                    else if !consumed.is_empty() { if !question.is_empty() { question.push('\n'); } question.push_str(consumed); }
                }
                if !question.is_empty() && !answer.is_empty() {
                    out.push_str(&format!("\n> **Domanda**  \n> {}  \n> **Risposta**  \n> {}\n\n", question.replace('\n', "  \n> "), answer.replace('\n', "  \n> ")));
                    continue;
                }
                out.push_str(line); out.push('\n'); continue;
            }
            if let Some((term, def)) = clean.split_once("::") {
                let t = term.trim(); let d = def.trim();
                let valid = !t.is_empty() && !d.is_empty() && !t.contains('$') && !t.contains('`') && !t.contains('\\') && !t.starts_with('>');
                if valid { out.push_str(&format!("\n> **{}**  \n> {}\n\n", t, d)); continue; }
            }
        }
        out.push_str(line); out.push('\n');
    }
    out
}

/// Fence language names unknown to the bundled syntax set, mapped to the closest syntax it has
/// (e.g. TypeScript -> JavaScript). Only used for on-screen rendering, never for exports.
fn code_fence_alias(lang: &str) -> Option<&'static str> {
    Some(match lang.to_ascii_lowercase().as_str() {
        "ts" | "typescript" | "tsx" | "jsx" | "mjs" | "cjs" | "node" | "vue" | "svelte" => "js",
        "json5" | "jsonc" => "json",
        "kt" | "kts" | "kotlin" => "java",
        "csharp" | "c#" => "cs",
        "golang" => "go",
        "python3" | "py3" => "py",
        "c++" | "cc" | "cxx" | "hpp" | "hxx" | "cuda" | "cu" => "cpp",
        "shell" | "console" | "zsh" | "fish" | "ksh" | "dockerfile" => "sh",
        "scss" | "sass" | "less" => "css",
        "svg" | "xhtml" | "xaml" => "xml",
        "postgres" | "postgresql" | "mysql" | "sqlite" | "plsql" | "tsql" => "sql",
        "octave" => "matlab",
        _ => return None,
    })
}

/// Rewrites the language of fenced code blocks through [`code_fence_alias`].
fn alias_code_fences(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut in_code_block = false;
    for line in markdown.split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']);
        let eol = &line[body.len()..];
        let rest = body.trim_start();
        if rest.starts_with("```") {
            in_code_block = !in_code_block;
            if in_code_block {
                let indent = &body[..body.len() - rest.len()];
                let ticks = rest.chars().take_while(|&c| c == '`').count();
                let lang = rest[ticks..]
                    .trim()
                    .split(|c: char| c.is_whitespace() || c == ',' || c == '{')
                    .next()
                    .unwrap_or("");
                if let Some(alias) = code_fence_alias(lang) {
                    out.push_str(indent);
                    out.push_str(&"`".repeat(ticks));
                    out.push_str(alias);
                    out.push_str(eol);
                    continue;
                }
            }
        }
        out.push_str(line);
    }
    out
}

/// Markdown preparation for on-screen rendering: math preprocessing plus code-fence language
/// aliases, so more programming languages get syntax highlighting.
pub fn preprocess_markdown_view(markdown: &str) -> String {
    alias_code_fences(&preprocess_markdown_math(markdown))
}

// Convert display and inline math into styled Markdown for egui_commonmark. 

pub fn preprocess_markdown_math(markdown: &str) -> String { let mut result = String::with_capacity(markdown.len() + 64); let mut rem = markdown;      // 1. Process display math blocks. 
    while let Some(start) = rem.find("$$") {
        result.push_str(&rem[..start]);
        let after_start = &rem[start + 2..];
        if let Some(end) = after_start.find("$$") { let expr = &after_start[..end]; let formatted = format_latex_expr(expr); result.push_str(&format!("\n\n**`{}`**\n\n", formatted)); rem = &after_start[end + 2..]; } else { result.push_str("$$");
            rem = after_start;
            break;
        }
    }
    result.push_str(rem);

    // 2. Process inline math line by line, skipping code blocks.
    let mut final_out = String::with_capacity(result.len());
    let mut in_code_block = false;
    let lines: Vec<&str> = result.lines().collect();

    for (line_idx, &line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            final_out.push_str(line);
            final_out.push('\n');
            continue;
        }

        if in_code_block {
            final_out.push_str(line);
            final_out.push('\n');
            continue;
        }

        let mut l_rem = line;
        while let Some(s_idx) = l_rem.find('$') {
            final_out.push_str(&l_rem[..s_idx]);
            let after_s = &l_rem[s_idx + 1..];
            if let Some(e_idx) = after_s.find('$') {
                let expr = &after_s[..e_idx];
                if !expr.is_empty() && !expr.starts_with(' ') && !expr.ends_with(' ') {
                    let formatted = format_latex_expr(expr);
                    final_out.push_str(&format!("`{}`", formatted));
                    l_rem = &after_s[e_idx + 1..];
                    continue;
                }
            }
            final_out.push('$');
            l_rem = after_s;
        }
        final_out.push_str(l_rem);

        let is_blockquote = trimmed.starts_with('>');
        let next_is_blockquote = lines
            .get(line_idx + 1)
            .map_or(false, |next| next.trim().starts_with('>'));

        if is_blockquote && !next_is_blockquote {
            final_out.push_str("\n\n");
        } else if !trimmed.is_empty() && !is_blockquote {
            final_out.push_str("  \n");
        } else {
            final_out.push('\n');
        }
    }

    final_out
}

/// Shifts all page-indexed maps (`notes`, `annotations`, `blank_pages`) when a blank page
/// is inserted at `at_page` (1-based) or removed.
pub fn shift_doc_data_on_insert(
    notes: &mut HashMap<u32, String>,
    annotations: &mut HashMap<u32, Vec<PageAnnotation>>,
    blank_pages: &mut std::collections::HashSet<u32>,
    sketches: &mut HashMap<u32, SketchPage>,
    inserted_page: u32,
) {
    let mut new_notes = HashMap::with_capacity(notes.len());
    for (k, v) in notes.drain() {
        new_notes.insert(if k >= inserted_page { k + 1 } else { k }, v);
    }
    *notes = new_notes;

    let mut new_anns = HashMap::with_capacity(annotations.len());
    for (k, v) in annotations.drain() {
        new_anns.insert(if k >= inserted_page { k + 1 } else { k }, v);
    }
    *annotations = new_anns;

    let mut new_sketches = HashMap::with_capacity(sketches.len());
    for (k, v) in sketches.drain() {
        new_sketches.insert(if k >= inserted_page { k + 1 } else { k }, v);
    }
    *sketches = new_sketches;

    let mut new_blanks = std::collections::HashSet::with_capacity(blank_pages.len() + 1);
    for k in blank_pages.drain() {
        new_blanks.insert(if k >= inserted_page { k + 1 } else { k });
    }
    new_blanks.insert(inserted_page);
    *blank_pages = new_blanks;
}

/// Decimates jittery micro-points and applies one iteration of Chaikin's corner-cutting
/// B-spline smoothing so trackpad/mouse freehand strokes look crisp and calligraphic.
pub fn smooth_freehand_points(raw: &[(f32, f32)]) -> Vec<(f32, f32)> {
    if raw.len() <= 2 {
        return raw.to_vec();
    }

    // 1. Radial distance decimation (removes stationary trackpad jitter)
    let mut filtered = Vec::with_capacity(raw.len());
    filtered.push(raw[0]);
    for &pt in &raw[1..] {
        let &(lx, ly) = filtered.last().unwrap();
        if (pt.0 - lx).hypot(pt.1 - ly) >= 0.0022 {
            filtered.push(pt);
        }
    }
    if let Some(&last) = raw.last() {
        if filtered.last() != Some(&last) {
            filtered.push(last);
        }
    }
    if filtered.len() <= 2 {
        return filtered;
    }

    // 2. Chaikin 1/4 - 3/4 corner cutting
    let mut smoothed = Vec::with_capacity(filtered.len() * 2);
    smoothed.push(filtered[0]);
    for pair in filtered.windows(2) {
        let (x0, y0) = pair[0];
        let (x1, y1) = pair[1];
        let q = (0.75 * x0 + 0.25 * x1, 0.75 * y0 + 0.25 * y1);
        let r = (0.25 * x0 + 0.75 * x1, 0.25 * y0 + 0.75 * y1);
        smoothed.push(q);
        smoothed.push(r);
    }
    smoothed.push(*filtered.last().unwrap());
    smoothed
}
#[derive(Debug, Clone)]
pub struct Flashcard {
    pub page: u32,
    pub front: String,
    pub back: String,
}

/// Parses all slide notes to extract flashcards using either:
/// 1. Inline syntax: `Concept :: Definition`
/// 2. Multi-line block syntax: `Q: Question...` followed by `A: Answer...`
pub fn extract_flashcards_from_notes(notes: &HashMap<u32, String>) -> Vec<Flashcard> {
    let mut cards = Vec::new();
    let mut sorted_pages: Vec<u32> = notes.keys().copied().collect();
    sorted_pages.sort_unstable();

    for page in sorted_pages {
        if let Some(content) = notes.get(&page) {
            cards.extend(extract_flashcards_from_page(page, content));
        }
    }

    cards
}

/// `true` when the note of one page contains at least one flashcard (what the trainer would find).
pub fn page_has_flashcards(content: &str) -> bool {
    // Cheap pre-filter: this runs every frame for the page on screen.
    if !content.contains("::") && !content.contains("Q:") && !content.contains("D:") {
        return false;
    }
    !extract_flashcards_from_page(0, content).is_empty()
}

/// Removes a Markdown list marker (`- `, `* `, `+ `) at the start of a line. A `*` that is part of
/// the text (`*args`, `**Term**`, `A*`) is never touched.
fn strip_list_marker(line: &str) -> &str {
    let mut t = line.trim();
    while let Some(rest) = ["- ", "* ", "+ "].iter().find_map(|m| t.strip_prefix(m)) {
        t = rest.trim_start();
    }
    t
}

/// `**Term**` / `*Term*` -> `Term`, only when the emphasis wraps the whole title and nothing inside
/// it uses `*`; any other asterisk belongs to the title and is kept.
fn strip_title_emphasis(title: &str) -> &str {
    let t = title.trim();
    for m in ["***", "**", "*"] {
        if t.len() > 2 * m.len() && t.starts_with(m) && t.ends_with(m) {
            let inner = t[m.len()..t.len() - m.len()].trim();
            if !inner.is_empty() && !inner.contains('*') {
                return inner;
            }
        }
    }
    t
}

fn extract_flashcards_from_page(page: u32, content: &str) -> Vec<Flashcard> {
    let mut cards = Vec::new();
    let mut lines = content.lines().peekable();
    while let Some(raw_line) = lines.next() {
        let trimmed = strip_list_marker(raw_line);

        // 1. Check inline `Front :: Back` syntax
        if let Some((left, right)) = trimmed.split_once("::") {
            let f = strip_title_emphasis(left);
            let b = right.trim();
            if !f.is_empty() && !b.is_empty() {
                cards.push(Flashcard {
                    page,
                    front: f.to_string(),
                    back: b.to_string(),
                });
                continue;
            }
        }

        // 2. Check `Q: ...` / `A: ...` block syntax
        if let Some(q_start) = trimmed
            .strip_prefix("Q:")
            .or_else(|| trimmed.strip_prefix("**Q:**"))
            .or_else(|| trimmed.strip_prefix("D:"))
        {
            let mut question = q_start.trim().to_string();
            let mut answer = String::new();
            let mut in_answer = false;

            while let Some(&next_raw) = lines.peek() {
                let next_trim = next_raw.trim();
                if next_trim.is_empty() && in_answer {
                    break;
                }
                if next_trim.starts_with("Q:")
                    || next_trim.starts_with("**Q:**")
                    || next_trim.contains("::")
                {
                    break;
                }

                let consumed = lines.next().unwrap().trim();
                if let Some(a_start) = consumed
                    .strip_prefix("A:")
                    .or_else(|| consumed.strip_prefix("**A:**"))
                    .or_else(|| consumed.strip_prefix("R:"))
                {
                    in_answer = true;
                    answer.push_str(a_start.trim());
                } else if in_answer {
                    if !answer.is_empty() {
                        answer.push('\n');
                    }
                    answer.push_str(consumed);
                } else if !consumed.is_empty() {
                    if !question.is_empty() {
                        question.push('\n');
                    }
                    question.push_str(consumed);
                }
            }

            if !question.is_empty() && !answer.is_empty() {
                cards.push(Flashcard {
                    page,
                    front: question,
                    back: answer,
                });
            }
        }
    }

    cards
}

/// Escapes the `*` of a flashcard question that Markdown would swallow because they do not form a
/// valid emphasis (`*args`, `A*`, `a * b`, `**kwargs`), so they are shown as typed. Asterisks that
/// do open and close an emphasis (`**bold**`, `*italic*`), list markers and code spans are kept.
pub fn escape_stray_asterisks(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len() + 8);
    for (i, line) in markdown.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let indent = line.len() - line.trim_start().len();
        let body = &line[indent..];
        out.push_str(&line[..indent]);
        let body = match body.strip_prefix("* ") {
            Some(rest) => {
                out.push_str("* ");
                rest
            }
            None => body,
        };
        // Even segments are text, odd ones are inside `code spans`.
        let segs: Vec<&str> = body.split('`').collect();
        let closed = segs.len() % 2 == 1;
        for (n, seg) in segs.iter().enumerate() {
            if n > 0 {
                out.push('`');
            }
            if n % 2 == 0 || !closed {
                out.push_str(&escape_stray_in_text(seg));
            } else {
                out.push_str(seg);
            }
        }
    }
    out
}

fn escape_stray_in_text(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    // Runs of `*`: (start, len, can_open, can_close)
    let mut runs: Vec<(usize, usize, bool, bool)> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '*' && (i == 0 || chars[i - 1] != '\\') {
            let start = i;
            while i < chars.len() && chars[i] == '*' {
                i += 1;
            }
            let before = start.checked_sub(1).map(|j| chars[j]);
            let after = chars.get(i).copied();
            let can_open = after.is_some_and(|c| !c.is_whitespace());
            let can_close = before.is_some_and(|c| !c.is_whitespace());
            runs.push((start, i - start, can_open, can_close));
        } else {
            i += 1;
        }
    }
    // Pair closers with the nearest opener of the same length.
    let mut matched = vec![false; runs.len()];
    let mut stack: Vec<usize> = Vec::new();
    for (ri, &(_, len, can_open, can_close)) in runs.iter().enumerate() {
        if can_close {
            if let Some(pos) = stack.iter().rposition(|&o| runs[o].1 == len) {
                let o = stack.remove(pos);
                matched[o] = true;
                matched[ri] = true;
                continue;
            }
        }
        if can_open {
            stack.push(ri);
        }
    }
    let mut out = String::with_capacity(text.len() + 4);
    let mut next_run = 0;
    let mut i = 0;
    while i < chars.len() {
        if next_run < runs.len() && runs[next_run].0 == i {
            let (_, len, _, _) = runs[next_run];
            for _ in 0..len {
                if !matched[next_run] {
                    out.push('\\');
                }
                out.push('*');
            }
            i += len;
            next_run += 1;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// Exports extracted flashcards into an Anki-ready `.tsv` file with automatic slide tags.
pub fn export_flashcards_anki_tsv(
    out_path: &Path,
    pdf_name: &str,
    cards: &[Flashcard],
) -> io::Result<()> {
    let mut out = String::from("#separator:tab\n#html:true\n#tags column:3\n");
    let clean_deck_tag: String = pdf_name
        .trim_end_matches(".pdf")
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();

    for c in cards {
        let front_fmt = preprocess_markdown_math(&c.front)
            .replace('\t', " ")
            .replace("  \n", "<br>")
            .replace('\n', "<br>");
        let back_fmt = preprocess_markdown_math(&c.back)
            .replace('\t', " ")
            .replace("  \n", "<br>")
            .replace('\n', "<br>");
        let tag = format!("{} slide_{}", clean_deck_tag, c.page);
        out.push_str(&format!("{}\t{}\t{}\n", front_fmt, back_fmt, tag));
    }

    fs::write(out_path, out)
}

pub fn create_blank_study_notebook(dest_path: &Path) -> io::Result<()> {
    use lopdf::{dictionary, Document as LoDoc, Object, Stream};

    let mut doc = LoDoc::with_version("1.5");
    let pages_id = doc.new_object_id();

    // Light ruled grid for 16:9 lecture slides (960 × 540).
    let mut grid_ops = String::from("q 0.88 0.90 0.94 RG 0.5 w ");
    for y in (24..520).step_by(24) {
        grid_ops.push_str(&format!("20 {} m 940 {} l S ", y, y));
    }
    grid_ops.push_str("Q");
    let content_id = doc.add_object(Stream::new(dictionary! {}, grid_ops.into_bytes()));

    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => vec![0.into(), 0.into(), 960.into(), 540.into()],
        "Contents" => content_id,
        "Resources" => dictionary! {},
    });

    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page_id.into()],
            "Count" => 1,
        }),
    );

    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);

    doc.save(dest_path)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
    Ok(())
}

/// Shared originals folder: ~/.local/share/lapis/originals or XDG_DATA_HOME/lapis/originals.
fn original_storage_dir() -> PathBuf {
    let dir = crate::paths::data_dir().join("originals");
    let _ = fs::create_dir_all(&dir);
    dir
}

/// `<doc id>_<file name>.orig` (the id is stable: see docid.rs).
pub fn original_backup_path(pdf_path: &Path) -> PathBuf {
    original_backup_path_for(pdf_path, &crate::docid::resolve_key(pdf_path))
}

/// Read-only variant (does not create ids nor migrate): for listings such as the storage manager.
pub fn original_backup_lookup(pdf_path: &Path) -> PathBuf {
    original_backup_path_for(pdf_path, &crate::docid::lookup_key(pdf_path))
}

fn original_backup_path_for(pdf_path: &Path, key: &str) -> PathBuf {
    let fname = pdf_path.file_name().and_then(|n| n.to_str()).unwrap_or("document.pdf");
    original_storage_dir().join(format!("{key}_{fname}.orig"))
}

/// Legacy path used only to migrate older backups.
fn legacy_original_backup_path(pdf_path: &Path) -> PathBuf {
    let parent = pdf_path.parent().unwrap_or_else(|| Path::new("."));
    let fname = pdf_path.file_name().and_then(|n| n.to_str()).unwrap_or("document.pdf");
    parent.join(format!(".{}.orig", fname))
}

pub fn ensure_original_pdf_backup(pdf_path: &Path) -> io::Result<()> {
    if !pdf_path.exists() {
        return Ok(());
    }
    let backup = original_backup_path(pdf_path);
    let legacy = legacy_original_backup_path(pdf_path);
    if backup.exists() {
        // Remove any remaining duplicate beside the PDF.
        if legacy.exists() { let _ = fs::remove_file(legacy); }
        return Ok(());
    }
    if let Some(parent) = backup.parent() {
        fs::create_dir_all(parent)?;
    }
    if legacy.exists() {
        // Move the existing backup to shared storage and remove the adjacent copy.
        fs::copy(&legacy, &backup)?;
        let _ = fs::remove_file(legacy);
    } else {
        fs::copy(pdf_path, &backup)?;
    }
    Ok(())
}

pub fn export_original_pdf(pdf_path: &Path, dest_path: &Path) -> io::Result<()> {
    // Migrate a legacy .orig file without creating a new backup.
    let backup = original_backup_path(pdf_path);
    let legacy = legacy_original_backup_path(pdf_path);
    if !backup.exists() && legacy.exists() {
        if let Some(parent) = backup.parent() { fs::create_dir_all(parent)?; }
        fs::copy(&legacy, &backup)?;
        let _ = fs::remove_file(&legacy);
    }
    let source = if backup.exists() { &backup } else { pdf_path };
    std::fs::copy(source, dest_path)?;
    Ok(())
}

// ════════════════════════════════════════════════════════════════════════════


// ════════════════════════════════════════════════════════════════════════════

/// Valore ereditato dai nodi Pages (MediaBox, Rotate, Resources…), risalendo i Parent.
pub(crate) fn page_inherited(doc: &lopdf::Document, mut id: lopdf::ObjectId, key: &[u8]) -> Option<lopdf::Object> {
    for _ in 0..16 {
        let dict = doc.get_object(id).ok()?.as_dict().ok()?;
        if let Ok(v) = dict.get(key) {
            return Some(v.clone());
        }
        id = dict.get(b"Parent").and_then(|o| o.as_reference()).ok()?;
    }
    None
}

/// Resolve indirect references; return direct objects unchanged.
pub(crate) fn resolve_obj(doc: &lopdf::Document, o: lopdf::Object) -> lopdf::Object {
    match o {
        lopdf::Object::Reference(r) => doc.get_object(r).map(|x| x.clone()).unwrap_or(lopdf::Object::Null),
        other => other,
    }
}

/// Create a Flate-compressed image XObject and return its object ID.
pub(crate) fn add_image_xobject(doc: &mut lopdf::Document, shot: &crate::screenshots::Shot) -> lopdf::ObjectId {
    use lopdf::{dictionary, Object, Stream};

    let (iw, ih) = (shot.width as i64, shot.height as i64);
    let n = (shot.width as usize) * (shot.height as usize);
    let mut rgb = Vec::with_capacity(n * 3);
    let mut alpha = Vec::with_capacity(n);
    let mut has_alpha = false;
    for px in shot.rgba.chunks_exact(4).take(n) {
        rgb.extend_from_slice(&px[..3]);
        alpha.push(px[3]);
        if px[3] != 255 {
            has_alpha = true;
        }
    }

    let smask_id = if has_alpha {
        let mut s = Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => iw,
                "Height" => ih,
                "ColorSpace" => "DeviceGray",
                "BitsPerComponent" => 8,
            },
            alpha,
        );
        let _ = s.compress();
        Some(doc.add_object(s))
    } else {
        None
    };

    let mut img_dict = dictionary! {
        "Type" => "XObject",
        "Subtype" => "Image",
        "Width" => iw,
        "Height" => ih,
        "ColorSpace" => "DeviceRGB",
        "BitsPerComponent" => 8,
    };
    if let Some(id) = smask_id {
        img_dict.set("SMask", Object::Reference(id));
    }
    let mut img_stream = Stream::new(img_dict, rgb);
    let _ = img_stream.compress();
    doc.add_object(img_stream)
}

/// Add an XObject to one page without changing other pages.
pub(crate) fn register_xobject_on_page(
    doc: &mut lopdf::Document,
    page_id: lopdf::ObjectId,
    name: &str,
    xobject_id: lopdf::ObjectId,
) -> Result<(), String> {
    use lopdf::{Dictionary, Object};

    let mut res: Dictionary = match page_inherited(doc, page_id, b"Resources").map(|o| resolve_obj(doc, o)) {
        Some(Object::Dictionary(d)) => d,
        _ => Dictionary::new(),
    };
    let mut xobjects: Dictionary = match res.get(b"XObject").ok().cloned().map(|o| resolve_obj(doc, o)) {
        Some(Object::Dictionary(d)) => d,
        _ => Dictionary::new(),
    };
    xobjects.set(name.as_bytes().to_vec(), Object::Reference(xobject_id));
    res.set("XObject", Object::Dictionary(xobjects));
    doc.get_object_mut(page_id)
        .and_then(|o| o.as_dict_mut())
        .map_err(|e| e.to_string())?
        .set("Resources", Object::Dictionary(res));
    Ok(())
}

/// Image annotations prepared as XObjects but not yet added to a page.
pub struct PreparedImages {
    /// Map filenames to XObject names for `draw_pdf_annotations`.
    pub names: std::collections::HashMap<String, String>,
    xobjects: Vec<(String, lopdf::ObjectId)>,
}

impl PreparedImages {
    /// `(resource name, XObject id)` of every prepared image.
    pub(crate) fn xobject_entries(&self) -> &[(String, lopdf::ObjectId)] {
        &self.xobjects
    }

    /// Add XObjects to `page_id` after creating the page.
    /// Call once for each page that uses the prepared resources.
    pub fn register_on(&self, doc: &mut lopdf::Document, page_id: lopdf::ObjectId) {
        for (name, id) in &self.xobjects {
            let _ = register_xobject_on_page(doc, page_id, name, *id);
        }
    }
}

/// Load image annotation PNGs as XObjects before `draw_pdf_annotations`.
/// `cache` prevents duplicate XObjects across pages.
pub fn prepare_image_xobjects(
    doc: &mut lopdf::Document,
    anns: &[PageAnnotation],
    media_dir: &Path,
    cache: &mut std::collections::HashMap<String, lopdf::ObjectId>,
) -> PreparedImages {
    let mut out = PreparedImages {
        names: std::collections::HashMap::new(),
        xobjects: Vec::new(),
    };
    for ann in anns {
        if ann.kind != AnnotationKind::Image
            || ann.points.len() < 2
            || ann.text.is_empty()
            || out.names.contains_key(&ann.text)
        {
            continue;
        }
        let id = match cache.get(&ann.text) {
            Some(id) => *id,
            None => {
                let Some(shot) = crate::screenshots::Shot::from_file(&media_dir.join(&ann.text)) else {
                    continue; // Skip missing image files.
                };
                if shot.is_empty() {
                    continue;
                }
                let id = add_image_xobject(doc, &shot);
                cache.insert(ann.text.clone(), id);
                id
            }
        };
        let name = format!("LapisImg{}", id.0);
        out.names.insert(ann.text.clone(), name.clone());
        out.xobjects.push((name, id));
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn page_has_flashcards_detects_both_syntaxes() {
        assert!(page_has_flashcards("Mitosis :: cell division"));
        assert!(page_has_flashcards("Q: What is 2+2?\nA: 4"));
        assert!(page_has_flashcards("- **Q:** capital of Italy?\n**A:** Rome"));
        assert!(!page_has_flashcards(""));
        assert!(!page_has_flashcards("Just a plain note\nwith two lines"));
        assert!(!page_has_flashcards("Q: a question without an answer"));
        assert!(!page_has_flashcards("empty side ::   "));
    }

    fn card_fronts(note: &str) -> Vec<String> {
        let mut m = HashMap::new();
        m.insert(1u32, note.to_string());
        extract_flashcards_from_notes(&m).into_iter().map(|c| c.front).collect()
    }

    #[test]
    fn asterisks_in_flashcard_titles_are_kept() {
        assert_eq!(card_fronts("*args :: positional arguments"), ["*args"]);
        assert_eq!(card_fronts("**kwargs :: keyword arguments"), ["**kwargs"]);
        assert_eq!(card_fronts("A* :: a search algorithm"), ["A*"]);
        assert_eq!(card_fronts("a * b :: product"), ["a * b"]);
        assert_eq!(card_fronts("- A* :: search"), ["A*"]);
        // A wrapping emphasis is still removed, a list marker too.
        assert_eq!(card_fronts("**Mitosis** :: division"), ["Mitosis"]);
        assert_eq!(card_fronts("* *Mitosis* :: division"), ["Mitosis"]);
        // Q/A blocks written in bold keep their text clean.
        let mut m = HashMap::new();
        m.insert(1u32, "**Q:** What is *p?\n**A:** a pointer".to_string());
        let c = extract_flashcards_from_notes(&m);
        assert_eq!((c[0].front.as_str(), c[0].back.as_str()), ("What is *p?", "a pointer"));
    }

    #[test]
    fn stray_asterisks_are_escaped_for_display() {
        assert_eq!(escape_stray_asterisks("*args"), "\\*args");
        assert_eq!(escape_stray_asterisks("A*"), "A\\*");
        assert_eq!(escape_stray_asterisks("a * b"), "a \\* b");
        assert_eq!(escape_stray_asterisks("**kwargs and *args"), "\\*\\*kwargs and \\*args");
        // Real emphasis, list markers and code spans are left alone.
        assert_eq!(escape_stray_asterisks("a **bold** and *it* word"), "a **bold** and *it* word");
        assert_eq!(escape_stray_asterisks("* item *x"), "* item \\*x");
        assert_eq!(escape_stray_asterisks("`a*b` and c*"), "`a*b` and c\\*");
    }

    use super::*;

    #[test]
    fn sketches_follow_pages_when_a_page_is_inserted() {
        let mut notes = HashMap::new();
        let mut anns = HashMap::new();
        let mut blanks = HashSet::new();
        let mut sketches = HashMap::new();
        for p in [1u32, 2, 3] {
            let mut sk = SketchPage::default();
            // Marker to recognise the sheet after the shift.
            sk.strokes.push(PageAnnotation {
                kind: AnnotationKind::Line,
                color: [0, 0, 0],
                points: vec![(p as f32, 0.0), (p as f32, 1.0)],
                text: String::new(),
                font_size: 16.0,
                bold: false,
                italic: false,
                stroke_width: 2.0,
            });
            sketches.insert(p, sk);
        }
        shift_doc_data_on_insert(&mut notes, &mut anns, &mut blanks, &mut sketches, 2);
        let mut keys: Vec<u32> = sketches.keys().copied().collect();
        keys.sort();
        assert_eq!(keys, vec![1, 3, 4]);
        assert_eq!(sketches[&1].strokes[0].points[0].0, 1.0);
        assert_eq!(sketches[&3].strokes[0].points[0].0, 2.0);
        assert_eq!(sketches[&4].strokes[0].points[0].0, 3.0);
    }

    #[test]
    fn default_sketch_is_not_saved_but_a_styled_one_is() {
        let mut sk = SketchPage::default();
        assert!(sk.is_default());
        sk.paper = BlankPageStyle::Grid;
        assert!(!sk.is_default());
        let json = serde_json::to_string(&sk).unwrap();
        let back: SketchPage = serde_json::from_str(&json).unwrap();
        assert_eq!(back.paper, BlankPageStyle::Grid);
        // Old files without the field still load.
        let d: DocData = serde_json::from_str("{\"notes\":{}}").unwrap();
        assert!(d.sketches.is_empty());
    }

    #[test]
    fn version_1_sketches_are_converted_to_units_once() {
        let json = r#"{"paper":"Grid","aspect":2.0,"strokes":[
            {"kind":"Line","color":[0,0,0],"points":[[0.5,0.25],[1.0,0.5]],"stroke_width":3.0}]}"#;
        let mut sk: SketchPage = serde_json::from_str(json).unwrap();
        sk.migrate_legacy();
        assert_eq!(sk.strokes[0].points, vec![(200.0, 200.0), (400.0, 400.0)]);
        // Idempotent, and the legacy field is never written back.
        sk.migrate_legacy();
        assert_eq!(sk.strokes[0].points[0], (200.0, 200.0));
        assert!(!serde_json::to_string(&sk).unwrap().contains("aspect"));
    }


    #[test]
    fn header_multibyte_does_not_panic() {
        assert_eq!(parse_slide_header("### 󰗊 Lecture Transcript"), None);
        assert_eq!(parse_slide_header("è un esempio"), None);
    }

    #[test]
    fn identity_follows_the_file_and_newest_revision_wins() {
        let _l = crate::docid::test_lock();
        let root = std::env::temp_dir().join(format!("lapis_notes_flow_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        // SAFETY: test serializzati da test_lock(), nessun altro thread legge l'ambiente.
        unsafe {
            std::env::set_var("XDG_DATA_HOME", root.join("xdg"));
            std::env::set_var("XDG_CONFIG_HOME", root.join("cfg"));
        }
        let pdf_a = root.join("a.pdf");
        fs::write(&pdf_a, b"%PDF-1.4\n%%EOF\n").unwrap();

        // salvataggio iniziale: l'id finisce nel blocco note dentro il PDF
        let mut notes = HashMap::new();
        notes.insert(1u32, "first".to_string());
        let _ = load_doc_data(&pdf_a);
        save_doc_data(&pdf_a, &notes, &HashMap::new(), HashSet::new(), &HashMap::new()).unwrap();
        let id_a = crate::docid::resolve_key(&pdf_a);
        assert_eq!(read_trailing_doc_id(&pdf_a).as_deref(), Some(id_a.as_str()));
        assert!(load_local_backup(&pdf_a).unwrap().rev > 0);

        // 1) spostato/rinominato: stesso id, note intatte
        let sub = root.join("sub");
        fs::create_dir_all(&sub).unwrap();
        let pdf_b = sub.join("renamed.pdf");
        fs::rename(&pdf_a, &pdf_b).unwrap();
        assert_eq!(crate::docid::resolve_key(&pdf_b), id_a);
        assert_eq!(load_doc_data(&pdf_b).notes.get(&1).map(String::as_str), Some("first"));

        // 2) copia con l'originale ancora presente: id nuovo, note comunque caricate dal PDF
        let pdf_c = root.join("copy.pdf");
        fs::copy(&pdf_b, &pdf_c).unwrap();
        let id_c = crate::docid::resolve_key(&pdf_c);
        assert_ne!(id_c, id_a);
        assert_eq!(load_doc_data(&pdf_c).notes.len(), 1);

        // 3) PDF modificato altrove (revisione più recente) batte il backup locale vecchio
        let mut remote = DocData::default();
        remote.notes.insert(1, "edited elsewhere".into());
        remote.doc_id = Some(id_a.clone());
        remote.rev = now_ms() + 10_000;
        save_data_to_pdf_trailing(&pdf_b, &remote).unwrap();
        let loaded = load_doc_data(&pdf_b);
        assert_eq!(loaded.notes[&1], "edited elsewhere");
        assert_eq!(load_local_backup(&pdf_b).unwrap().notes[&1], "edited elsewhere");
        let stashed = fs::read_dir(get_backup_dir())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(&format!("{id_a}.conflict-")))
            .count();
        assert_eq!(stashed, 1, "la versione scartata deve essere conservata");

        // 4) un salvataggio successivo ha sempre revisione maggiore di quella caricata
        let mut n2 = HashMap::new();
        n2.insert(1u32, "local edit".to_string());
        save_doc_data(&pdf_b, &n2, &HashMap::new(), HashSet::new(), &HashMap::new()).unwrap();
        assert!(load_local_backup(&pdf_b).unwrap().rev > remote.rev);

        // 5) backup più recente del PDF (scrittura nel PDF fallita): il PDF viene riallineato
        let mut newer = DocData::default();
        newer.notes.insert(1, "local newer".into());
        newer.rev = now_ms() + 60_000;
        save_local_backup(&pdf_b, &newer);
        assert_eq!(load_doc_data(&pdf_b).notes[&1], "local newer");
        assert_eq!(read_trailing_doc_data(&pdf_b).unwrap().notes[&1], "local newer");

        // 6) dati senza revisione (versioni precedenti): regola storica, il backup vince
        //    e le pagine mancanti arrivano dal PDF
        let mut old_pdf = DocData::default();
        old_pdf.notes.insert(2, "only in pdf".into());
        save_data_to_pdf_trailing(&pdf_c, &old_pdf).unwrap(); // rev 0
        let mut old_bak = DocData::default();
        old_bak.notes.insert(1, "in backup".into());
        save_local_backup(&pdf_c, &old_bak); // rev 0
        let merged = load_doc_data(&pdf_c);
        assert_eq!(merged.notes.len(), 2);
        assert_eq!(merged.notes[&1], "in backup");

        let _ = fs::remove_dir_all(&root);
    }
}
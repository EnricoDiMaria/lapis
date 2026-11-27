use crate::paths::{replace_file, write_atomic, write_synced};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
        Self {
            paper,
            strokes: Vec::new(),
            legacy_aspect: None,
        }
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
    /// True after PDF-native annotations have been imported into the local annotation model.
    /// This keeps autosaved edits/deletions stable across an application restart.
    #[serde(default)]
    pub pdf_annots_imported: bool,
    /// File length at the last import/commit; a changed length can signal a newer external revision.
    #[serde(default)]
    pub pdf_annots_file_len: u64,
}

const NOTES_START_TAG: &[u8] = b"\n%LAPIS_CUSTOM_START\n";
const NOTES_END_TAG: &[u8] = b"\n%LAPIS_CUSTOM_END\n";
const LEGACY_NOTES_START_TAG: &[u8] = b"\n---Lapis_NOTES_V1---\n";
const LEGACY_NOTES_END_TAG: &[u8] = b"\n---Lapis_NOTES_END---\n";
const LAPIS_INCREMENTAL_MARKER: &[u8] = b"%LAPIS_INC_EOF ";

#[derive(Clone, Copy)]
struct EmbeddedBlock {
    start: usize,
    body_start: usize,
    body_end: usize,
    end: usize,
}

fn find_embedded_block(bytes: &[u8]) -> Option<EmbeddedBlock> {
    let candidates = [
        (NOTES_START_TAG, NOTES_END_TAG),
        (LEGACY_NOTES_START_TAG, LEGACY_NOTES_END_TAG),
    ];
    let mut found: Option<EmbeddedBlock> = None;
    for (start_tag, end_tag) in candidates {
        let mut from = 0;
        while from < bytes.len() {
            let Some(rel) = memchr::memmem::find(&bytes[from..], start_tag) else { break };
            let start = from + rel;
            let body_start = start + start_tag.len();
            let Some(end_rel) = memchr::memmem::find(&bytes[body_start..], end_tag) else {
                from = start + start_tag.len();
                continue;
            };
            let body_end = body_start + end_rel;
            let end = body_end + end_tag.len();
            let candidate = EmbeddedBlock { start, body_start, body_end, end };
            if found.is_none_or(|old| candidate.start > old.start) { found = Some(candidate); }
            from = end;
        }
    }
    found
}

fn terminal_embedded_block(bytes: &[u8]) -> Option<EmbeddedBlock> {
    let block = find_embedded_block(bytes)?;
    // Legacy trailers are physically after %%EOF. New metadata lives inside an uncompressed
    // stream object and is therefore not mistaken for a terminal trailer.
    bytes[block.end..].iter().all(u8::is_ascii_whitespace).then_some(block)
}

fn embedded_json_bytes(body: &[u8]) -> Vec<u8> {
    if !body.starts_with(b"%") { return body.to_vec(); }
    let mut json = Vec::with_capacity(body.len());
    for line in body.split(|byte| *byte == b'\n') {
        if let Some(content) = line.strip_prefix(b"%") { json.extend_from_slice(content); }
    }
    json
}

fn parse_embedded_data(body: &[u8]) -> Option<DocData> {
    let json = embedded_json_bytes(body);
    parse_doc_data_bytes(json.trim_ascii())
}

/// Build an uncompressed stream containing the custom Lapis metadata block.
/// The stream is intentionally unreferenced: it is a durable, searchable payload
/// without placing large data between startxref and %%EOF.
fn custom_data_stream(data: &DocData) -> io::Result<lopdf::Stream> {
    let mut payload = Vec::new();
    payload.extend_from_slice(NOTES_START_TAG);
    serde_json::to_writer(&mut payload, &portable(data))
        .map_err(|e| io::Error::other(format!("Lapis metadata serialization error: {e}")))?;
    payload.extend_from_slice(NOTES_END_TAG);
    Ok(lopdf::Stream::new(lopdf::Dictionary::new(), payload))
}

// JSON whitespace keeps the length field fixed-width without changing its type.
const PDF_LEN_FIELD: &[u8] = b"\"pdf_annots_file_len\":18446744073709551615";

fn pending_length_stream(data: &DocData) -> io::Result<lopdf::Stream> {
    let mut data = data.clone();
    data.pdf_annots_file_len = u64::MAX;
    custom_data_stream(&data)
}

fn finalize_pdf_length(output: &mut [u8], revision_start: usize) -> io::Result<u64> {
    let len = output.len() as u64;
    let revision = output.get_mut(revision_start..)
        .ok_or_else(|| io::Error::other("invalid metadata revision offset"))?;
    let block = find_embedded_block(revision)
        .ok_or_else(|| io::Error::other("missing Lapis metadata stream"))?;
    let body = &mut revision[block.body_start..block.body_end];
    let pos = memchr::memmem::rfind(body, PDF_LEN_FIELD)
        .ok_or_else(|| io::Error::other("missing reserved PDF length field"))?;
    let start = pos + PDF_LEN_FIELD.len() - 20;
    let field = &mut body[start..start + 20];
    let digits = len.to_string();
    field.fill(b' ');
    field[..digits.len()].copy_from_slice(digits.as_bytes());
    Ok(len)
}

/// Strip old *unreferenced* custom-stream payloads when we are already doing a
/// full PDF rewrite (page structure changes or an explicit history compaction).
/// Never splice bytes from an existing incremental revision: xref offsets depend on them.
fn remove_custom_data_streams(doc: &mut lopdf::Document) {
    doc.objects.retain(|_, obj| {
        if let lopdf::Object::Stream(stream) = obj {
            if let Some(block) = find_embedded_block(&stream.content) {
                if block.start == 0
                    && block.end == stream.content.len()
                    && parse_embedded_data(&stream.content[block.body_start..block.body_end]).is_some()
                {
                    return false;
                }
            }
        }
        true
    });
}

/// Put the rollback comment just before the final `startxref` keyword, not
/// between the xref offset and `%%EOF`. lopdf's reader (and other strict PDF
/// readers) expects the latter pair to form the unbroken final PDF trailer.
/// The xref table/stream is earlier in the file, so inserting here does not
/// change a single object or xref offset.
fn insert_revision_marker_before_startxref(pdf: &mut Vec<u8>, base_len: usize) -> io::Result<()> {
    let start = memchr::memmem::rfind(pdf, b"startxref")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "PDF has no startxref"))?;
    let tail = &pdf[start + b"startxref".len()..];
    let mut lines = tail.split(|b| *b == b'\n');
    let offset = lines.next().unwrap_or_default();
    // The first line after startxref can be blank; the numeric offset must be
    // followed by the terminal %%EOF, without a Lapis comment in between.
    let offset = if offset.iter().all(u8::is_ascii_whitespace) {
        lines.next().unwrap_or_default()
    } else { offset };
    if offset.iter().all(u8::is_ascii_whitespace)
        || !offset.iter().filter(|b| !b.is_ascii_whitespace()).all(u8::is_ascii_digit)
        || !tail.windows(5).any(|w| w == b"%%EOF")
    {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid final startxref trailer"));
    }
    let mut marker = Vec::with_capacity(48);
    if start > 0 && pdf[start - 1] != b'\n' && pdf[start - 1] != b'\r' {
        marker.push(b'\n');
    }
    marker.extend_from_slice(LAPIS_INCREMENTAL_MARKER);
    marker.extend_from_slice(base_len.to_string().as_bytes());
    marker.push(b'\n');
    pdf.splice(start..start, marker);
    Ok(())
}

/// Strip only Lapis metadata that is physically after the last PDF revision.
/// Metadata between revisions is retained: removing it would shift external xref offsets.
fn strip_terminal_metadata(bytes: &mut Vec<u8>) {
    if let Some(block) = terminal_embedded_block(bytes) {
        bytes.truncate(block.start);
    }
}

/// Read the complete PDF revision chain. Only a legacy terminal Lapis trailer is detached;
/// metadata stored in PDF stream objects and older revisions remains in place.
fn read_pdf_and_notes(pdf_path: &Path) -> io::Result<(Vec<u8>, Vec<u8>)> {
    let mut bytes = fs::read(pdf_path)?;
    let tail = find_embedded_block(&bytes).map(|block| {
        let mut out = Vec::new();
        out.extend_from_slice(NOTES_START_TAG);
        out.extend_from_slice(&bytes[block.body_start..block.body_end]);
        out.extend_from_slice(NOTES_END_TAG);
        out
    }).unwrap_or_default();
    strip_terminal_metadata(&mut bytes);
    Ok((bytes, tail))
}

/// Loads the PDF while preserving its incremental revisions and embedded Lapis metadata.
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

/// Re-keys every per-page map of `data`; `None` drops the page.
fn remap_doc_data_pages(data: &mut DocData, map: &dyn Fn(u32) -> Option<u32>) {
    fn remap<V>(items: &mut HashMap<u32, V>, map: &dyn Fn(u32) -> Option<u32>) {
        *items = std::mem::take(items)
            .into_iter()
            .filter_map(|(page, value)| map(page).map(|new_page| (new_page, value)))
            .collect();
    }
    remap(&mut data.notes, map);
    remap(&mut data.annotations, map);
    remap(&mut data.sketches, map);
    data.blank_pages = data.blank_pages.iter().filter_map(|&page| map(page)).collect();
}

/// Applies a page change to the Lapis payload of a structurally rewritten PDF. It starts from
/// the newest of the local backup and the payload read by [`read_pdf_and_notes`], so the PDF
/// carries the model on screen with its new page numbering, at a revision above the (not yet
/// remapped) backup. A stop before the backup is rewritten then loses nothing.
fn rebase_embedded_notes(pdf_path: &Path, notes: &[u8], change: impl FnOnce(&mut DocData)) -> Vec<u8> {
    let embedded = find_embedded_block(notes)
        .and_then(|block| parse_embedded_data(&notes[block.body_start..block.body_end]));
    let key = crate::docid::resolve_key(pdf_path);
    let backup = load_local_backup(pdf_path)
        .filter(|d| d.doc_id.as_deref().is_none_or(|id| id == key));
    let mut data = match (backup, embedded) {
        (Some(b), Some(e)) => if b.rev >= e.rev { b } else { e },
        (Some(d), None) | (None, Some(d)) => d,
        (None, None) => return notes.to_vec(),
    };
    change(&mut data);
    data.rev = next_rev(pdf_path);
    data.doc_id.get_or_insert(key);
    let mut out = Vec::new();
    out.extend_from_slice(NOTES_START_TAG);
    if serde_json::to_writer(&mut out, &portable(&data)).is_err() {
        return notes.to_vec();
    }
    out.extend_from_slice(NOTES_END_TAG);
    out
}

/// Write the PDF and note data to a temporary file, then replace the original atomically.
fn save_pdf_with_notes(
    doc: &mut lopdf::Document,
    pdf_path: &Path,
    tag: &str,
    notes: &[u8],
) -> io::Result<()> {
    let tmp_path = pdf_path.with_extension(format!("{tag}_tmp.pdf"));
    let mut adopted: Option<DocData> = None;
    let res = (|| -> io::Result<()> {
        // A structural rewrite is already non-incremental: do not carry forward
        // obsolete custom streams from previous revisions.
        remove_custom_data_streams(doc);
        let mut metadata = find_embedded_block(notes)
            .and_then(|block| parse_embedded_data(&notes[block.body_start..block.body_end]));
        // The reserved fixed-width length field is patched in place: one serialization.
        if let Some(data) = metadata.as_ref() {
            doc.add_object(lopdf::Object::Stream(pending_length_stream(data)?));
        }
        let mut output = Vec::new();
        doc.save_to(&mut output)
            .map_err(|e| io::Error::other(format!("lopdf save error: {e}")))?;
        if let Some(data) = metadata.as_mut() {
            data.pdf_annots_file_len = finalize_pdf_length(&mut output, 0)?;
        }
        adopted = metadata;
        write_synced(&tmp_path, &output)
    })();
    res.and_then(|_| replace_file(&tmp_path, pdf_path))
        .inspect_err(|_| { let _ = fs::remove_file(&tmp_path); })?;
    // The embedded payload already wins on its revision; this brings the backup in line.
    if let Some(mut data) = adopted {
        let _guard = checkpoint_lock();
        data.pdf_annots_imported = true;
        remember_rev(pdf_path, data.rev);
        save_local_backup(pdf_path, &data);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum BlankPageStyle {
    #[default]
    Ruled,
    Dotted,
    Grid,
    Blank,
}

pub fn insert_blank_page_with_style_into_pdf(
    pdf_path: &Path,
    after_page: u32,
    style: BlankPageStyle,
    allow_signature_loss: bool,
) -> io::Result<u32> {
    use lopdf::{Object, ObjectId, Stream, dictionary};
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
    let mut doc = load_lopdf(&clean, pdf_path, "ins").map_err(|e| io::Error::other(e))?;
    drop(clean);
    ensure_not_signed(&doc, allow_signature_loss)?;

    let pages = doc.get_pages();
    let total = pages.len() as u32;
    let ref_page_num = after_page.clamp(1, total.max(1));
    let ref_id = pages.get(&ref_page_num).copied();
    drop(pages);

    let catalog_pages_root = doc
        .catalog()
        .and_then(|cat| cat.get(b"Pages"))
        .and_then(|o| o.as_reference())
        .map_err(|e| io::Error::other(e.to_string()))?;

    let mut x0 = 0.0_f32;
    let mut y0 = 0.0_f32;
    let mut width = 960.0_f32;
    let mut height = 540.0_f32;

    if let Some(id) = ref_id {
        let mb = find_inherited_array(&doc, id, b"CropBox")
            .or_else(|| find_inherited_array(&doc, id, b"MediaBox"));
        if let Some(arr) = mb
            && arr.len() == 4
            && let (Some(bx0), Some(by0), Some(bx1), Some(by1)) = (
                obj_to_f32(&arr[0]),
                obj_to_f32(&arr[1]),
                obj_to_f32(&arr[2]),
                obj_to_f32(&arr[3]),
            )
        {
            x0 = bx0.min(bx1);
            y0 = by0.min(by1);
            width = (bx1 - bx0).abs().max(72.0);
            height = (by1 - by0).abs().max(72.0);
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
        BlankPageStyle::Blank => {}
        BlankPageStyle::Ruled => {
            let step_y = usable_h / 18.0;
            let _ = write!(grid_ops, "q 0.86 0.89 0.94 RG {line_w:.2} w ");
            for i in 0..=18 {
                let y = (y0 + height - top_margin) - (i as f32) * step_y;
                let _ = write!(grid_ops, "{x_start:.2} {y:.2} m {x_end:.2} {y:.2} l S ");
            }
            grid_ops.push('Q');
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
            let _ = write!(
                grid_ops,
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
            grid_ops.push('Q');
        }
        BlankPageStyle::Dotted => {
            let step = (width / 32.0).clamp(10.0, 24.0);
            let dot = (height / 700.0).clamp(0.7, 1.3);
            let _ = write!(grid_ops, "q 0.68 0.72 0.78 rg ");
            let mut y = y0 + bottom_margin;
            while y <= y0 + height - top_margin {
                let mut x = x_start;
                while x <= x_end {
                    let _ = write!(
                        grid_ops,
                        "{:.2} {:.2} {dot:.2} {dot:.2} re f ",
                        x - dot * 0.5,
                        y - dot * 0.5
                    );
                    x += step;
                }
                y += step;
            }
            grid_ops.push('Q');
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

    if let Ok(parent_dict) = doc
        .get_object_mut(target_parent_id)
        .and_then(|o| o.as_dict_mut())
        && let Ok(kids) = parent_dict.get_mut(b"Kids").and_then(|o| o.as_array_mut())
    {
        let pos = insert_pos.unwrap_or(kids.len()).min(kids.len());
        kids.insert(pos, Object::Reference(new_page_id));
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
        .find_map(|(p_num, oid)| {
            if oid == new_page_id {
                Some(p_num)
            } else {
                None
            }
        })
        .unwrap_or(ref_page_num + 1);

    let notes = rebase_embedded_notes(pdf_path, &notes, |data| {
        shift_doc_data_on_insert(
            &mut data.notes,
            &mut data.annotations,
            &mut data.blank_pages,
            &mut data.sketches,
            actual_new_page_num,
        );
    });
    save_pdf_with_notes(&mut doc, pdf_path, "ins", &notes)?;
    Ok(actual_new_page_num)
}

pub fn delete_page_from_pdf(pdf_path: &Path, page_to_delete: u32, allow_signature_loss: bool) -> io::Result<()> {
    ensure_original_pdf_backup(pdf_path)?;

    let (clean, notes) = read_pdf_and_notes(pdf_path)?;
    let mut doc = load_lopdf(&clean, pdf_path, "del").map_err(|e| io::Error::other(e))?;
    drop(clean);
    ensure_not_signed(&doc, allow_signature_loss)?;

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

    let notes = rebase_embedded_notes(pdf_path, &notes, |data| {
        remap_doc_data_pages(data, &|page| match page.cmp(&page_to_delete) {
            std::cmp::Ordering::Less => Some(page),
            std::cmp::Ordering::Equal => None,
            std::cmp::Ordering::Greater => Some(page - 1),
        });
    });
    save_pdf_with_notes(&mut doc, pdf_path, "del", &notes)
}

/// Move a page to a new 1-based position while preserving
/// page objects and their PDF links and destinations.
/// Flatten the /Pages tree and copy inherited attributes
/// to each page before reordering.
pub fn reorder_pages_in_pdf(pdf_path: &Path, from_page: u32, to_page: u32, allow_signature_loss: bool) -> io::Result<()> {
    use lopdf::{Object, ObjectId};

    if from_page == to_page {
        return Ok(());
    }
    ensure_original_pdf_backup(pdf_path)?;
    let (clean, notes) = read_pdf_and_notes(pdf_path)?;
    let mut doc = load_lopdf(&clean, pdf_path, "reorder").map_err(|e| io::Error::other(e))?;
    drop(clean);
    ensure_not_signed(&doc, allow_signature_loss)?;

    let pages_map = doc.get_pages();
    let total = pages_map.len() as u32;
    if from_page == 0 || to_page == 0 || from_page > total || to_page > total {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Page index out of range",
        ));
    }
    let mut ordered: Vec<ObjectId> = (1..=total)
        .filter_map(|n| pages_map.get(&n).copied())
        .collect();
    drop(pages_map);
    if ordered.len() != total as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid PDF page tree",
        ));
    }
    let moved = ordered.remove((from_page - 1) as usize);
    ordered.insert((to_page - 1) as usize, moved);

    let root_id = doc
        .catalog()
        .and_then(|cat| cat.get(b"Pages"))
        .and_then(|o| o.as_reference())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

    // Collect the nearest inherited attributes for each page.
    const INHERITED: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];
    for &page_id in &ordered {
        let mut inherited: Vec<(&'static [u8], Object)> = Vec::new();
        let parent = doc
            .get_object(page_id)
            .and_then(|o| o.as_dict())
            .ok()
            .and_then(|d| d.get(b"Parent").and_then(|o| o.as_reference()).ok());
        for key in INHERITED {
            let present = doc
                .get_object(page_id)
                .and_then(|o| o.as_dict())
                .map(|d| d.has(key))
                .unwrap_or(false);
            if present {
                continue;
            }
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
        let page_dict = doc
            .get_object_mut(page_id)
            .and_then(|o| o.as_dict_mut())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        for (key, value) in inherited {
            page_dict.set(key, value);
        }
        page_dict.set("Parent", Object::Reference(root_id));
    }

    let root = doc
        .get_object_mut(root_id)
        .and_then(|o| o.as_dict_mut())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    root.set(
        "Kids",
        Object::Array(ordered.into_iter().map(Object::Reference).collect()),
    );
    root.set("Count", Object::Integer(total as i64));

    let notes = rebase_embedded_notes(pdf_path, &notes, |data| {
        remap_doc_data_pages(data, &|page| {
            Some(if page == from_page {
                to_page
            } else if from_page < to_page && page > from_page && page <= to_page {
                page - 1
            } else if from_page > to_page && page >= to_page && page < from_page {
                page + 1
            } else {
                page
            })
        });
    });
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

/// Atomic local checkpoint: retain the previous complete JSON until the next one is
/// safely written. This is the crash-recovery source of truth between PDF commits.
fn save_local_backup_checked(pdf_path: &Path, data: &DocData) -> io::Result<()> {
    if pdf_path.as_os_str().is_empty() { return Ok(()); }
    let backup_path = get_backup_path(pdf_path);
    let mut portable_backup = data.clone();
    portable_backup.source_path =
        Some(dunce::canonicalize(pdf_path).unwrap_or_else(|_| pdf_path.to_path_buf()));
    let json = serde_json::to_vec(&portable_backup)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let tmp = backup_path.with_extension("json.tmp");
    let result = (|| -> io::Result<()> {
        let mut file = File::create(&tmp)?;
        file.write_all(&json)?;
        file.sync_all()?;
        drop(file);
        replace_file(&tmp, &backup_path)
    })();
    if result.is_err() { let _ = fs::remove_file(&tmp); }
    if result.is_ok() { remember_backup(&backup_path, portable_backup); }
    result
}

fn save_local_backup(pdf_path: &Path, data: &DocData) {
    if let Err(err) = save_local_backup_checked(pdf_path, data) {
        eprintln!("Lapis: local notes checkpoint failed: {err}");
    }
}

struct CachedBackup {
    path: PathBuf,
    stamp: PdfStamp,
    data: DocData,
}

// The last backup this process read or wrote. It is reused only while the file's stamp (length,
// mtime, and inode/ctime on Unix) is unchanged, so any outside change falls back to the disk.
static BACKUP_CACHE: Mutex<Option<CachedBackup>> = Mutex::new(None);

fn remember_backup(path: &Path, data: DocData) {
    let Ok(stamp) = pdf_stamp(path) else { return; };
    if let Ok(mut cache) = BACKUP_CACHE.lock() {
        *cache = Some(CachedBackup { path: path.to_path_buf(), stamp, data });
    }
}

fn load_local_backup(pdf_path: &Path) -> Option<DocData> {
    if pdf_path.as_os_str().is_empty() {
        return None;
    }
    let backup_path = get_backup_path(pdf_path);
    let stamp = pdf_stamp(&backup_path).ok()?;
    if let Ok(cache) = BACKUP_CACHE.lock()
        && let Some(hit) = cache.as_ref().filter(|c| c.path == backup_path && c.stamp == stamp)
    {
        return Some(hit.data.clone());
    }
    let bytes = fs::read(&backup_path).ok()?;
    let data = parse_doc_data_bytes(&bytes)?;
    // Cache only if the file did not change while it was being read.
    if pdf_stamp(&backup_path).is_ok_and(|after| after == stamp)
        && let Ok(mut cache) = BACKUP_CACHE.lock()
    {
        *cache = Some(CachedBackup { path: backup_path, stamp, data: data.clone() });
    }
    Some(data)
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
        let last = m.entry(pdf_path.to_path_buf()).or_default();
        *last = (*last).max(rev);
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

/// The PDF gained a revision Lapis has not merged yet; writing now would drop it.
#[derive(Debug)]
struct ExternalModification;

impl std::fmt::Display for ExternalModification {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the PDF was modified outside Lapis; merge its annotations before saving")
    }
}

impl std::error::Error for ExternalModification {}

pub fn is_external_modification(err: &io::Error) -> bool {
    err.get_ref().is_some_and(|inner| inner.is::<ExternalModification>())
}

/// True when `base` has bytes Lapis neither wrote nor imported. `watermark` is the PDF length
/// recorded at the last import or commit; a Lapis tail that records the current length is ours.
fn externally_modified(base: &[u8], watermark: u64) -> bool {
    let len = base.len() as u64;
    watermark != 0
        && watermark != len
        && !(trailing_lapis_revision(base).is_some()
            && embedded_doc_data(base).is_some_and(|stored| stored.pdf_annots_file_len == len))
}

/// Imports annotations added by other programs into the live model (idempotent).
pub fn merge_external_annotations(
    pdf_path: &Path,
    annotations: &mut HashMap<u32, Vec<PageAnnotation>>,
) {
    import_external_annotations(pdf_path, annotations);
}

/// Records the current PDF length as seen. Call only after the merged model is checkpointed.
pub fn mark_pdf_observed(pdf_path: &Path) -> io::Result<()> {
    let _guard = checkpoint_lock();
    let Some(mut data) = load_local_backup(pdf_path) else { return Ok(()); };
    let len = fs::metadata(pdf_path)?.len();
    if data.pdf_annots_imported && data.pdf_annots_file_len == len { return Ok(()); }
    data.pdf_annots_imported = true;
    data.pdf_annots_file_len = len;
    save_local_backup_checked(pdf_path, &data)
}

/// Test seam: runs once between the atomic PDF replacement and the backup reconciliation, the
/// exact point where a UI checkpoint can interleave with a background commit.
#[cfg(test)]
static AFTER_REPLACE_HOOK: Mutex<Option<Box<dyn FnOnce() + Send>>> = Mutex::new(None);

#[cfg(test)]
fn run_after_replace_hook() {
    let hook = AFTER_REPLACE_HOOK.lock().unwrap().take();
    if let Some(hook) = hook { hook(); }
}

/// The PDF carries a digital signature that a page change would invalidate.
#[derive(Debug)]
struct SignedPdf;

impl std::fmt::Display for SignedPdf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("this PDF is digitally signed; changing its pages invalidates the signature")
    }
}

impl std::error::Error for SignedPdf {}

pub fn is_signed_pdf_error(err: &io::Error) -> bool {
    err.get_ref().is_some_and(|inner| inner.is::<SignedPdf>())
}

/// A signature value covers byte ranges of the file (`/ByteRange`); an empty signature field
/// does not, so it never blocks page edits.
fn pdf_has_signed_value(doc: &lopdf::Document) -> bool {
    doc.objects.values().any(|object| {
        let dictionary = match object {
            lopdf::Object::Dictionary(d) => Some(d),
            lopdf::Object::Stream(s) => Some(&s.dict),
            _ => None,
        };
        dictionary.is_some_and(|d| d.get(b"ByteRange").is_ok())
    })
}

/// Page edits rewrite the whole file, which invalidates signatures. Checked on the freshly
/// loaded document: pruning a deleted page can drop the signature objects. The caller decides
/// (after asking the user) whether to proceed.
fn ensure_not_signed(doc: &lopdf::Document, allow_signature_loss: bool) -> io::Result<()> {
    if !allow_signature_loss && pdf_has_signed_value(doc) {
        return Err(io::Error::other(SignedPdf));
    }
    Ok(())
}

fn same_content(a: &DocData, b: &DocData) -> bool {
    a.blank_pages == b.blank_pages
        && a.notes == b.notes
        && a.annotations == b.annotations
        && a.sketches == b.sketches
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
                .is_some_and(|n| n.starts_with(&prefix))
        })
        .collect();
    all.sort(); // timestamps with the same number of digits: lexicographic == chronological
    while all.len() > 5 {
        let _ = fs::remove_file(all.remove(0));
    }
}

/// Reads only the document id from the newest metadata block. It first scans the last 16 MiB;
/// very large legacy trailers fall back to a full read only when the fast scan misses.
pub(crate) fn read_trailing_doc_id(pdf_path: &Path) -> Option<String> {
    #[derive(Deserialize)]
    struct IdOnly { #[serde(default)] doc_id: Option<String> }

    const WINDOW: u64 = 16 * 1024 * 1024;
    let mut file = File::open(pdf_path).ok()?;
    let len = file.metadata().ok()?.len();
    let take = len.min(WINDOW);
    file.seek(SeekFrom::Start(len.saturating_sub(take))).ok()?;
    let mut tail = Vec::with_capacity(take as usize);
    file.read_to_end(&mut tail).ok()?;
    let block = find_embedded_block(&tail);
    let owned;
    let (bytes, block) = if let Some(block) = block {
        (&tail[..], block)
    } else if len > WINDOW {
        owned = fs::read(pdf_path).ok()?;
        let block = find_embedded_block(&owned)?;
        (&owned[..], block)
    } else {
        return None;
    };
    let body = bytes.get(block.body_start..block.body_end)?;
    let json = embedded_json_bytes(body);
    serde_json::from_slice::<IdOnly>(json.trim_ascii()).ok()?.doc_id
}

fn embedded_doc_data(bytes: &[u8]) -> Option<DocData> {
    let block = find_embedded_block(bytes)?;
    parse_embedded_data(&bytes[block.body_start..block.body_end])
}

#[cfg(test)]
fn read_trailing_doc_data(pdf_path: &Path) -> Option<DocData> {
    embedded_doc_data(&fs::read(pdf_path).ok()?)
}

// Cache only a complete terminal Lapis revision with matching file metadata.
fn read_opened_doc_data(path: &Path) -> Option<DocData> {
    let before = pdf_stamp(path).ok();
    let bytes = fs::read(path).ok()?;
    let data = embedded_doc_data(&bytes)?;
    if let Some(stamp) = before
        && pdf_stamp(path).ok().as_ref() == Some(&stamp)
        && stamp.len == bytes.len() as u64
        && data.pdf_annots_file_len == stamp.len
        && data.pdf_annots_imported
        && trailing_lapis_revision(&bytes).is_some()
    {
        remember_commit(path, stamp, &data);
    }
    Some(data)
}

/// Copy of the data without machine-specific fields, for embedding in the PDF.
fn portable(data: &DocData) -> DocData {
    let mut d = data.clone();
    d.source_path = None;
    d
}

pub fn load_doc_data(pdf_path: &Path) -> DocData {
    if pdf_path.as_os_str().is_empty() { return DocData::default(); }
    let key = crate::docid::resolve_key(pdf_path);
    let backup_opt = load_local_backup(pdf_path);
    let pdf_opt = read_opened_doc_data(pdf_path);
    let committed_len = pdf_opt.as_ref().and_then(|p| {
        let stamp = pdf_stamp(pdf_path).ok()?;
        (p.doc_id.as_deref() == Some(key.as_str()) && committed_matches(pdf_path, &stamp, p))
            .then_some(stamp.len)
    });
    let imported_flag = backup_opt.as_ref().is_some_and(|d| d.pdf_annots_imported)
        || pdf_opt.as_ref().is_some_and(|d| d.pdf_annots_imported);
    let imported_len = backup_opt.as_ref().map(|d| d.pdf_annots_file_len).unwrap_or(0)
        .max(pdf_opt.as_ref().map(|d| d.pdf_annots_file_len).unwrap_or(0));
    let mut data = match (backup_opt, pdf_opt) {
        (Some(b), Some(p)) if b.rev > 0 && p.rev > 0 => {
            if p.rev > b.rev { stash_conflict(&key, &b, &p); p }
            else { if b.rev > p.rev { stash_conflict(&key, &p, &b); } b }
        }
        (Some(b), Some(p)) if b.rev == 0 && p.rev > 0 => { stash_conflict(&key, &b, &p); p }
        (Some(mut b), Some(p)) => {
            for (k, v) in p.notes { b.notes.entry(k).or_insert(v); }
            for (k, v) in p.annotations { b.annotations.entry(k).or_insert(v); }
            for (k, v) in p.sketches { b.sketches.entry(k).or_insert(v); }
            b.blank_pages.extend(p.blank_pages);
            b
        }
        (Some(b), None) => b,
        (None, Some(p)) => p,
        (None, None) => DocData::default(),
    };

    // Import on first open and when the PDF's byte length changes (typically an external
    // incremental revision). Autosaves keep the last observed length so an app restart does not
    // re-import the same source annotations and undo local edits/deletions.
    data.pdf_annots_imported |= imported_flag;
    data.pdf_annots_file_len = committed_len.unwrap_or(data.pdf_annots_file_len.max(imported_len));
    let current_pdf_len = fs::metadata(pdf_path).map(|m| m.len()).unwrap_or(0);
    if !data.pdf_annots_imported || data.pdf_annots_file_len != current_pdf_len {
        import_external_annotations(pdf_path, &mut data.annotations);
        data.pdf_annots_imported = true;
        data.pdf_annots_file_len = current_pdf_len;
    }

    data.doc_id = Some(key);
    remember_rev(pdf_path, data.rev);
    save_local_backup(pdf_path, &data);
    data
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PdfStamp {
    len: u64,
    modified: SystemTime,
    created: Option<SystemTime>,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}

fn pdf_stamp(path: &Path) -> io::Result<PdfStamp> {
    let m = fs::metadata(path)?;
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    Ok(PdfStamp {
        len: m.len(),
        modified: m.modified()?,
        created: m.created().ok(),
        #[cfg(unix)]
        identity: (m.dev(), m.ino(), m.ctime(), m.ctime_nsec()),
    })
}

struct CommittedPdf {
    path: PathBuf,
    stamp: PdfStamp,
    data: DocData,
}

// Keep just the last matching terminal revision; never reuse it after a file change.
static LAST_COMMIT: Mutex<Option<CommittedPdf>> = Mutex::new(None);

fn remember_commit(path: &Path, stamp: PdfStamp, data: &DocData) {
    if let Ok(mut cached) = LAST_COMMIT.lock() {
        *cached = Some(CommittedPdf { path: path.to_path_buf(), stamp, data: data.clone() });
    }
}

fn committed_matches(path: &Path, stamp: &PdfStamp, data: &DocData) -> bool {
    LAST_COMMIT.lock().ok().is_some_and(|cached| {
        cached.as_ref().is_some_and(|saved| {
            saved.path == path && &saved.stamp == stamp
                && saved.data.doc_id == data.doc_id && same_content(&saved.data, data)
        })
    })
}

fn checkpoint_matches(previous: Option<&DocData>, data: &DocData) -> bool {
    previous.is_some_and(|old| {
        old.doc_id == data.doc_id
            && old.pdf_annots_imported == data.pdf_annots_imported
            && old.pdf_annots_file_len == data.pdf_annots_file_len
            && same_content(old, data)
    })
}

fn checkpoint_if_changed(path: &Path, previous: Option<&DocData>, data: &DocData) -> io::Result<()> {
    if !checkpoint_matches(previous, data) {
        save_local_backup_checked(path, data)?;
        remember_rev(path, data.rev);
    }
    Ok(())
}

fn collect_doc_data(
    pdf_path: &Path,
    notes: &HashMap<u32, String>,
    annotations: &HashMap<u32, Vec<PageAnnotation>>,
    blank_pages: HashSet<u32>,
    sketches: &HashMap<u32, SketchPage>,
    previous_backup: Option<&DocData>,
) -> DocData {
    let mut clean_notes = notes.clone();
    clean_notes.retain(|_, v| !v.trim().is_empty());
    let mut clean_anns = annotations.clone();
    clean_anns.retain(|_, v| !v.is_empty());
    let mut clean_pages = blank_pages;
    clean_pages.retain(|&v| v != 0);
    let mut clean_sketches = sketches.clone();
    clean_sketches.retain(|&k, s| k != 0 && !s.is_default());
    // load_doc_data establishes this flag before the app starts autosaving; reading the PDF
    // again here would turn every autosave into an unnecessary full-file scan.
    let pdf_annots_imported = previous_backup.is_some_and(|d| d.pdf_annots_imported);
    let pdf_annots_file_len = previous_backup.map(|d| d.pdf_annots_file_len).unwrap_or(0);
    DocData {
        notes: clean_notes,
        annotations: clean_anns,
        blank_pages: clean_pages,
        sketches: clean_sketches,
        doc_id: Some(crate::docid::resolve_key(pdf_path)),
        rev: next_rev(pdf_path),
        pdf_annots_imported,
        pdf_annots_file_len,
        ..Default::default()
    }
}

// Serialize local checkpoint updates, not the expensive PDF processing.
static CHECKPOINT_LOCK: Mutex<()> = Mutex::new(());

fn checkpoint_lock() -> std::sync::MutexGuard<'static, ()> {
    CHECKPOINT_LOCK.lock().unwrap_or_else(|err| err.into_inner())
}

// Merge only commit metadata into a newer local checkpoint, never its content.
fn finish_pdf_checkpoint(path: &Path, committed: &DocData) -> io::Result<()> {
    let _guard = checkpoint_lock();
    let previous = load_local_backup(path);
    let mut data = previous.as_ref()
        .filter(|d| d.rev >= committed.rev && d.doc_id == committed.doc_id)
        .cloned().unwrap_or_else(|| committed.clone());
    data.pdf_annots_imported = committed.pdf_annots_imported;
    data.pdf_annots_file_len = committed.pdf_annots_file_len;
    checkpoint_if_changed(path, previous.as_ref(), &data)
}

/// Autosaves only the local backup. PDF bytes are not touched while the document is open.
pub fn save_doc_data(
    pdf_path: &Path,
    notes: &HashMap<u32, String>,
    annotations: &HashMap<u32, Vec<PageAnnotation>>,
    blank_pages: HashSet<u32>,
    sketches: &HashMap<u32, SketchPage>,
) -> io::Result<()> {
    if pdf_path.as_os_str().is_empty() { return Ok(()); }
    let _guard = checkpoint_lock();
    let previous = load_local_backup(pdf_path);
    let data = collect_doc_data(pdf_path, notes, annotations, blank_pages, sketches, previous.as_ref());
    checkpoint_if_changed(pdf_path, previous.as_ref(), &data)
}

/// Capture a durable, ordered snapshot before starting the save worker.
pub fn prepare_pdf_commit(
    pdf_path: &Path,
    notes: &HashMap<u32, String>,
    annotations: &HashMap<u32, Vec<PageAnnotation>>,
    blank_pages: HashSet<u32>,
    sketches: &HashMap<u32, SketchPage>,
) -> io::Result<DocData> {
    let _guard = checkpoint_lock();
    let previous = load_local_backup(pdf_path);
    let mut data = collect_doc_data(pdf_path, notes, annotations, blank_pages, sketches, previous.as_ref());
    data.pdf_annots_imported = true;
    if let Some(previous) = previous.as_ref().filter(|d| d.rev > 0 && checkpoint_matches(Some(d), &data)) {
        data.rev = previous.rev;
    } else {
        // Reserve the revision durably before the worker starts.
        save_local_backup_checked(pdf_path, &data)?;
    }
    remember_rev(pdf_path, data.rev);
    Ok(data)
}

/// Synchronous fallback for exit and tests.
pub fn commit_doc_data(
    pdf_path: &Path,
    notes: &HashMap<u32, String>,
    annotations: &HashMap<u32, Vec<PageAnnotation>>,
    blank_pages: HashSet<u32>,
    sketches: &HashMap<u32, SketchPage>,
) -> io::Result<()> {
    if pdf_path.as_os_str().is_empty() { return Ok(()); }
    let data = prepare_pdf_commit(pdf_path, notes, annotations, blank_pages, sketches)?;
    commit_pdf_snapshot(pdf_path, data)
}

/// Commit the captured revision without replacing any later local edits.
pub fn commit_pdf_snapshot(pdf_path: &Path, mut data: DocData) -> io::Result<()> {
    let stamp = pdf_stamp(pdf_path)?;
    if committed_matches(pdf_path, &stamp, &data) {
        data.pdf_annots_file_len = stamp.len;
        return finish_pdf_checkpoint(pdf_path, &data);
    }
    let base = fs::read(pdf_path)?;
    if pdf_stamp(pdf_path)? != stamp {
        return Err(io::Error::other("PDF changed while reading; retry Save"));
    }
    commit_with_base(pdf_path, data, base, &stamp, false)
}

/// Keep one physical Lapis revision when it is the file's tail.
fn commit_with_base(
    pdf_path: &Path,
    mut data: DocData,
    base: Vec<u8>,
    source_stamp: &PdfStamp,
    force_rewrite: bool,
) -> io::Result<()> {
    let watermark = data.pdf_annots_file_len;
    let trailing = trailing_lapis_revision(&base);
    // Compare the already-read metadata before parsing the PDF.
    if !force_rewrite && trailing.is_some()
        && embedded_doc_data(&base).is_some_and(|stored| {
            same_content(&stored, &data)
                && stored.doc_id == data.doc_id
                && stored.pdf_annots_imported == data.pdf_annots_imported
                && stored.pdf_annots_file_len == base.len() as u64
        })
        && lopdf::Document::load_mem(&base).is_ok()
    {
        data.pdf_annots_file_len = base.len() as u64;
        if pdf_stamp(pdf_path)? == *source_stamp {
            finish_pdf_checkpoint(pdf_path, &data)?;
            remember_commit(pdf_path, source_stamp.clone(), &data);
            return Ok(());
        }
        return Err(io::Error::other("PDF changed while saving; retry Save"));
    }

    if !force_rewrite && externally_modified(&base, watermark) {
        return Err(io::Error::other(ExternalModification));
    }
    data.pdf_annots_file_len = base.len() as u64;

    // Reuse the successful prefix parse as the incremental document.
    let clean_len = terminal_embedded_block(&base).map_or(base.len(), |block| block.start);
    let prefix = trailing.and_then(|(_, len)| {
        (len <= clean_len).then_some(len).and_then(|len| {
            lopdf::IncrementalDocument::load_from(std::io::Cursor::new(&base[..len]))
                .ok().map(|doc| (len, doc))
        })
    });
    let (base_len, mut incremental) = match prefix {
        Some(parsed) => parsed,
        None => (clean_len, lopdf::IncrementalDocument::load_from(std::io::Cursor::new(&base[..clean_len]))
            .map_err(|e| io::Error::other(format!("cannot prepare incremental PDF update: {e}")))?),
    };
    drop(base);
    export_pdf_annotations(&mut incremental, &data.annotations)
        .map_err(|e| io::Error::other(format!("cannot export PDF annotations: {e}")))?;

    // Always commit an incremental revision: the uncompressed, unreferenced stream
    // carries the complete custom payload and remains discoverable by find_embedded_block.
    incremental.new_document.add_object(lopdf::Object::Stream(pending_length_stream(&data)?));
    let mut output = Vec::new();
    incremental.save_to(&mut output)
        .map_err(|e| io::Error::other(format!("cannot save incremental PDF update: {e}")))?;
    insert_revision_marker_before_startxref(&mut output, base_len)?;
    data.pdf_annots_file_len = finalize_pdf_length(&mut output, base_len)?;

    // Never replace a readable source PDF with an invalid incremental revision.
    // In particular, reject xref/startxref regressions before the atomic rename.
    lopdf::Document::load_mem(&output)
        .map_err(|e| io::Error::other(format!("incremental PDF validation failed: {e}")))?;
    let tmp = pdf_path.with_extension("lapis_commit_tmp.pdf");
    let result = (|| -> io::Result<()> {
        write_synced(&tmp, &output)?;
        if pdf_stamp(pdf_path)? != *source_stamp {
            return Err(io::Error::other("PDF changed while saving; retry Save"));
        }
        replace_file(&tmp, pdf_path)
    })();
    if result.is_err() { let _ = fs::remove_file(&tmp); }
    result?;
    #[cfg(test)]
    run_after_replace_hook();
    finish_pdf_checkpoint(pdf_path, &data)?;
    if let Ok(stamp) = pdf_stamp(pdf_path)
        && stamp.len == data.pdf_annots_file_len
    {
        remember_commit(pdf_path, stamp, &data);
    }
    Ok(())
}

/// Explicitly flatten obsolete PDF revisions and rebuild a single Lapis update.
/// This necessarily rewrites xref offsets; use only on demand, never after an
/// external editor update without the user's explicit action. Signed/encrypted
/// PDFs cannot be safely compacted by this method.
pub fn compact_doc_data(
    pdf_path: &Path,
    notes: &HashMap<u32, String>,
    annotations: &HashMap<u32, Vec<PageAnnotation>>,
    blank_pages: HashSet<u32>,
    sketches: &HashMap<u32, SketchPage>,
) -> io::Result<()> {
    if pdf_path.as_os_str().is_empty() { return Ok(()); }
    let stamp = pdf_stamp(pdf_path)?;
    let original = fs::read(pdf_path)?;
    // When Lapis was the last writer, undo its known incremental tail first.
    // This removes its metadata stream and annotation objects before lopdf's full
    // rewrite; the current local model is exported again by commit_with_base.
    // Never truncate across a later external editor revision.
    let source = trailing_lapis_revision(&original)
        .and_then(|(_, len)| lopdf::Document::load_mem(&original[..len]).ok().map(|_| &original[..len]))
        .unwrap_or(&original);
    let mut doc = lopdf::Document::load_mem(source)
        .map_err(|e| io::Error::other(format!("PDF cannot be compacted safely: {e}")))?;
    if doc.trailer.get(b"Encrypt").is_ok() {
        return Err(io::Error::other("encrypted PDFs are not eligible for compaction"));
    }
    // Compaction invalidates signatures even when their appearance survives.
    let signed = pdf_has_signed_value(&doc);
    if signed {
        return Err(io::Error::other("signed PDFs cannot be compacted without invalidating signatures"));
    }

    let expected_pages = doc.get_pages().len();
    remove_custom_data_streams(&mut doc);
    doc.prune_objects();
    let mut compacted = Vec::new();
    doc.save_to(&mut compacted)
        .map_err(|e| io::Error::other(format!("cannot rewrite PDF xrefs: {e}")))?;
    let verified = lopdf::Document::load_mem(&compacted)
        .map_err(|e| io::Error::other(format!("rewritten PDF failed validation: {e}")))?;
    if verified.get_pages().len() != expected_pages {
        return Err(io::Error::other("PDF page count changed during compaction"));
    }

    let data = prepare_pdf_commit(pdf_path, notes, annotations, blank_pages, sketches)?;
    // Flattening would export a model that lacks the external revision's annotations.
    if externally_modified(&original, data.pdf_annots_file_len) {
        return Err(io::Error::other(ExternalModification));
    }
    // Preserve the exact bytes of the original on disk so compaction can be undone.
    let backup = pdf_path.with_extension("lapis_before_compaction.pdf");
    if !backup.exists() { write_atomic(&backup, &original)?; }
    commit_with_base(pdf_path, data, compacted, &stamp, true)
}

/// If a Lapis marker is the final trailer, return its position and the byte length of the
/// PDF prefix that existed before that Lapis incremental revision.
fn trailing_lapis_revision(bytes: &[u8]) -> Option<(usize, usize)> {
    let pos = memchr::memmem::rfind(bytes, LAPIS_INCREMENTAL_MARKER)?;
    let digits_start = pos + LAPIS_INCREMENTAL_MARKER.len();
    let digits_end = bytes[digits_start..].iter().position(|b| *b == b'\n')? + digits_start;
    let base_len = std::str::from_utf8(&bytes[digits_start..digits_end])
        .ok()?.parse::<usize>().ok()?;
    if base_len > pos { return None; }
    let suffix = &bytes[digits_end + 1..];

    // Current layout: marker immediately before the FINAL startxref. Check the
    // whole tail, not just whether it contains %%EOF: external append-only edits
    // must prevent rollback, even if they contain another startxref/%%EOF.
    let suffix = suffix.trim_ascii_start();
    let new_layout = suffix.strip_prefix(b"startxref").is_some_and(|tail| {
        let tail = tail.trim_ascii_start();
        let num_end = tail.iter().position(|b| !b.is_ascii_digit()).unwrap_or(tail.len());
        num_end > 0 && tail[num_end..].trim_ascii_start().strip_prefix(b"%%EOF")
            .is_some_and(|end| end.iter().all(u8::is_ascii_whitespace))
    });

    // Old hybrid builds put the marker between the numeric xref offset and
    // %%EOF. Recognize these so their last revision can be safely migrated.
    let previous_layout = suffix.strip_prefix(b"%%EOF")
        .is_some_and(|end| end.iter().all(u8::is_ascii_whitespace))
        || find_embedded_block(suffix).is_some_and(|block| {
            block.start == 0 && suffix[block.end..].iter().all(u8::is_ascii_whitespace)
        });
    (new_layout || previous_layout).then_some((pos, base_len))
}

// ───────────────── PDF annotation bridge ─────────────────

fn pdf_num(o: &lopdf::Object) -> Option<f32> {
    match o { lopdf::Object::Integer(v) => Some(*v as f32), lopdf::Object::Real(v) => Some(*v), _ => None }
}

fn page_pdf_geometry(doc: &lopdf::Document, page_id: lopdf::ObjectId) -> Option<([f32; 4], i32)> {
    let media = page_inherited(doc, page_id, b"MediaBox").map(|o| resolve_obj(doc, o))?;
    let a = media.as_array().ok()?;
    if a.len() < 4 { return None; }
    let mut v = [0.0; 4];
    for i in 0..4 { v[i] = pdf_num(&resolve_obj(doc, a[i].clone()))?; }
    let mut rect = [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])];
    if let Some(crop) = page_inherited(doc, page_id, b"CropBox").map(|o| resolve_obj(doc, o)) {
        if let Ok(c) = crop.as_array() {
            if c.len() >= 4 {
                if let (Some(x0), Some(y0), Some(x1), Some(y1)) = (pdf_num(&resolve_obj(doc,c[0].clone())), pdf_num(&resolve_obj(doc,c[1].clone())), pdf_num(&resolve_obj(doc,c[2].clone())), pdf_num(&resolve_obj(doc,c[3].clone()))) {
                    let candidate = [rect[0].max(x0.min(x1)), rect[1].max(y0.min(y1)), rect[2].min(x0.max(x1)), rect[3].min(y0.max(y1))];
                    if candidate[2] > candidate[0] && candidate[3] > candidate[1] { rect = candidate; }
                }
            }
        }
    }
    let rotation = page_inherited(doc, page_id, b"Rotate").map(|o| resolve_obj(doc,o)).and_then(|o| o.as_i64().ok()).unwrap_or(0) as i32;
    Some((rect, ((rotation % 360 + 360) % 360)))
}

fn pdf_to_lapis_point(p: (f32, f32), rect: [f32; 4], rotation: i32) -> (f32, f32) {
    let (x0,y0,x1,y1) = (rect[0],rect[1],rect[2],rect[3]);
    let (w,h) = ((x1-x0).max(1.0),(y1-y0).max(1.0));
    let (dx,dy,dw,dh) = match rotation {
        90 => (p.1-y0, x1-p.0, h, w),
        180 => (x1-p.0, y1-p.1, w, h),
        270 => (y1-p.1, p.0-x0, h, w),
        _ => (p.0-x0, p.1-y0, w, h),
    };
    ((dx/dw).clamp(0.0,1.0), (1.0-dy/dh).clamp(0.0,1.0))
}

fn lapis_to_pdf_point(p: (f32, f32), rect: [f32; 4], rotation: i32) -> (f32, f32) {
    let (x0,y0,x1,y1) = (rect[0],rect[1],rect[2],rect[3]);
    let (w,h) = ((x1-x0).max(1.0),(y1-y0).max(1.0));
    let (dx,dy) = (p.0.clamp(0.0,1.0), 1.0-p.1.clamp(0.0,1.0));
    match rotation {
        90 => (x1-dy*w, y0+dx*h),
        180 => (x1-dx*w, y1-dy*h),
        270 => (x0+dy*w, y1-dx*h),
        _ => (x0+dx*w, y0+dy*h),
    }
}

fn annotation_color(doc: &lopdf::Document, dict: &lopdf::Dictionary) -> [u8;3] {
    // FreeText text color is commonly stored in /DA rather than /C. Prefer the
    // explicit nonstroking color operator so edits made in PDF editors are fingerprinted.
    if let Some(da) = dict.get(b"DA").ok().cloned().map(|o| resolve_obj(doc, o)) {
        if let lopdf::Object::String(bytes, _) = da {
            let tokens: Vec<String> = String::from_utf8_lossy(&bytes).split_whitespace().map(str::to_owned).collect();
            if let Some(index) = tokens.iter().position(|token| token == "rg") {
                if index >= 3 {
                    if let (Ok(r), Ok(g), Ok(b)) = (tokens[index-3].parse::<f32>(), tokens[index-2].parse::<f32>(), tokens[index-1].parse::<f32>()) {
                        return [(r.clamp(0.0,1.0)*255.0).round() as u8,
                                (g.clamp(0.0,1.0)*255.0).round() as u8,
                                (b.clamp(0.0,1.0)*255.0).round() as u8];
                    }
                }
            }
        }
    }
    let Some(value) = dict.get(b"C").ok().cloned().map(|o| resolve_obj(doc,o)) else { return [255, 220, 0]; };
    let Ok(a) = value.as_array() else { return [255,220,0]; };
    if a.len() < 3 { return [255,220,0]; }
    let mut c = [0;3];
    for i in 0..3 { c[i] = (pdf_num(&resolve_obj(doc,a[i].clone())).unwrap_or(1.0).clamp(0.0,1.0)*255.0).round() as u8; }
    c
}

/// FreeText font sizes are PDF text sizes (points). Edge and most editors put
/// the current font in /DA as `/FontName <size> Tf`; some also write a CSS-like
/// /DS style string. Do not infer font size from the annotation rectangle.
fn free_text_font_size(doc: &lopdf::Document, dict: &lopdf::Dictionary) -> f32 {
    fn valid_size(size: f32) -> Option<f32> {
        (size.is_finite() && size > 0.0).then_some(size.clamp(6.0, 96.0))
    }

    // Use the LAST valid Tf: a /DA string may set fonts more than once.
    if let Some(lopdf::Object::String(da, _)) = dict.get(b"DA").ok()
        .cloned().map(|object| resolve_obj(doc, object))
    {
        let tokens: Vec<&str> = std::str::from_utf8(&da)
            .unwrap_or("").split_whitespace().collect();
        for i in (2..tokens.len()).rev() {
            if tokens[i] == "Tf" && tokens[i - 2].starts_with('/') {
                if let Ok(size) = tokens[i - 1].parse::<f32>() {
                    if let Some(size) = valid_size(size) { return size; }
                }
            }
        }
    }

    // A few producers use the rich-text default style instead of /DA.
    if let Some(lopdf::Object::String(ds, _)) = dict.get(b"DS").ok()
        .cloned().map(|object| resolve_obj(doc, object))
    {
        let style = String::from_utf8_lossy(&ds);
        for declaration in style.split(';') {
            let Some((name, value)) = declaration.split_once(':') else { continue; };
            if !name.trim().eq_ignore_ascii_case("font-size") { continue; }
            let value = value.trim().to_ascii_lowercase();
            let size = value.strip_suffix("pt").unwrap_or(&value).trim();
            if let Ok(size) = size.parse::<f32>() {
                if let Some(size) = valid_size(size) { return size; }
            }
        }
    }
    16.0 // Compatibility with older annotations that specify no font size.
}

fn annotation_rect(doc: &lopdf::Document, dict: &lopdf::Dictionary) -> Option<[f32;4]> {
    let o = resolve_obj(doc, dict.get(b"Rect").ok()?.clone());
    let a = o.as_array().ok()?;
    if a.len() < 4 { return None; }
    let x0=pdf_num(&resolve_obj(doc,a[0].clone()))?; let y0=pdf_num(&resolve_obj(doc,a[1].clone()))?;
    let x1=pdf_num(&resolve_obj(doc,a[2].clone()))?; let y1=pdf_num(&resolve_obj(doc,a[3].clone()))?;
    Some([x0.min(x1),y0.min(y1),x0.max(x1),y0.max(y1)])
}

fn pdf_text(dict: &lopdf::Dictionary, key: &[u8]) -> String {
    dict.get(key).ok().and_then(|o| match o {
        lopdf::Object::String(bytes, _) => Some(decode_pdf_text(bytes)),
        _ => None,
    }).unwrap_or_default()
}

fn pdf_text_object(text: &str) -> lopdf::Object {
    let mut bytes = vec![0xFE, 0xFF];
    for unit in text.encode_utf16() { bytes.extend_from_slice(&unit.to_be_bytes()); }
    lopdf::Object::string_literal(bytes)
}

fn same_page_annotation(a: &PageAnnotation, b: &PageAnnotation) -> bool {
    a.kind == b.kind
        && a.color == b.color
        && a.text == b.text
        && a.bold == b.bold
        && a.italic == b.italic
        && (a.font_size - b.font_size).abs() < 0.001
        && (a.stroke_width - b.stroke_width).abs() < 0.001
        && a.points.len() == b.points.len()
        && a.points.iter().zip(&b.points).all(|(x, y)| {
            (x.0 - y.0).abs() < 0.0001 && (x.1 - y.1).abs() < 0.0001
        })
}

fn annotation_kind_name(kind: AnnotationKind) -> &'static str {
    match kind {
        AnnotationKind::Highlight => "Highlight", AnnotationKind::Underline => "Underline",
        AnnotationKind::Freehand => "Freehand", AnnotationKind::TextBox => "TextBox",
        AnnotationKind::Rect => "Rect", AnnotationKind::Circle => "Circle",
        AnnotationKind::Line => "Line", AnnotationKind::Arrow => "Arrow",
        AnnotationKind::Tape => "Tape", AnnotationKind::Image => "Image",
    }
}

fn annotation_kind_from_name(name: &str) -> Option<AnnotationKind> {
    Some(match name {
        "Highlight" => AnnotationKind::Highlight, "Underline" => AnnotationKind::Underline,
        "Freehand" => AnnotationKind::Freehand, "TextBox" => AnnotationKind::TextBox,
        "Rect" => AnnotationKind::Rect, "Circle" => AnnotationKind::Circle,
        "Line" => AnnotationKind::Line, "Arrow" => AnnotationKind::Arrow,
        "Tape" => AnnotationKind::Tape, "Image" => AnnotationKind::Image,
        _ => return None,
    })
}

/// Font sizes are stored relative to an 800-unit page height (the on-screen and slide-export
/// convention); PDF viewers read them as points, so they are scaled by the page height.
const FONT_UNITS_PAGE_H: f32 = 800.0;

/// Page size in PDF units as displayed (width and height swap on 90/270 degree pages).
fn displayed_page_size(rect: [f32; 4], rotation: i32) -> (f32, f32) {
    let (w, h) = ((rect[2] - rect[0]).max(1.0), (rect[3] - rect[1]).max(1.0));
    if rotation == 90 || rotation == 270 { (h, w) } else { (w, h) }
}

fn model_to_pdf_font_size(model: f32, displayed_h: f32) -> f32 {
    (model * displayed_h / FONT_UNITS_PAGE_H).clamp(4.0, 120.0)
}

fn pdf_to_model_font_size(points: f32, displayed_h: f32) -> f32 {
    (points * FONT_UNITS_PAGE_H / displayed_h.max(1.0)).clamp(4.0, 200.0)
}

/// A textbox starts with one invisible character that selects its translucent background.
const TEXTBOX_BG_MARKERS: [char; 2] = ['\u{200B}', '\u{200C}'];

fn textbox_leading_marker(text: &str) -> Option<char> {
    text.chars().next().filter(|c| TEXTBOX_BG_MARKERS.contains(c))
}

/// Background fill (color, opacity) matching the on-screen modes.
fn textbox_background(text: &str) -> Option<([u8; 3], f32)> {
    match textbox_leading_marker(text)? {
        '\u{200B}' => Some(([36, 38, 46], 200.0 / 255.0)),
        _ => Some(([245, 246, 250], 215.0 / 255.0)),
    }
}

/// The text other PDF viewers should show: no background marker, no `__`/`~~` formatting
/// delimiters, and a leading `- ` shown as a bullet, as the on-screen box renders it.
fn textbox_plain_text(text: &str) -> String {
    text.trim_start_matches(TEXTBOX_BG_MARKERS)
        .split('\n')
        .map(|line| {
            let (bullet, rest) = match line.strip_prefix("- ") {
                Some(rest) => ("\u{2022} ", rest),
                None => ("", line),
            };
            format!("{bullet}{}", rest.replace("__", "").replace("~~", ""))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Signature written by Lapis builds before textboxes ignored their height and markers.
fn annotation_signature_v1(ann: &PageAnnotation) -> String {
    let canonical_box = matches!(ann.kind,
        AnnotationKind::TextBox | AnnotationKind::Highlight | AnnotationKind::Rect
        | AnnotationKind::Circle | AnnotationKind::Tape
    );
    let mut points: Vec<(i64, i64)> = if canonical_box && !ann.points.is_empty() {
        let min_x = ann.points.iter().map(|p| p.0).fold(f32::INFINITY, f32::min);
        let min_y = ann.points.iter().map(|p| p.1).fold(f32::INFINITY, f32::min);
        let max_x = ann.points.iter().map(|p| p.0).fold(f32::NEG_INFINITY, f32::max);
        let max_y = ann.points.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max);
        vec![((min_x * 10_000.0).round() as i64, (min_y * 10_000.0).round() as i64),
             ((max_x * 10_000.0).round() as i64, (max_y * 10_000.0).round() as i64)]
    } else {
        ann.points.iter().map(|(x, y)|
            ((x * 10_000.0).round() as i64, (y * 10_000.0).round() as i64)
        ).collect()
    };
    points.sort_unstable();
    let mut payload = format!("{}|{:?}|{}", annotation_kind_name(ann.kind), ann.color, ann.text);
    for (x, y) in points { payload.push_str(&format!("|{x},{y}")); }
    let mut hash = 0xcbf29ce484222325u64;
    for byte in payload.as_bytes() { hash ^= *byte as u64; hash = hash.wrapping_mul(0x100000001b3); }
    format!("{hash:016x}")
}

/// Current signature. Textboxes ignore their bottom edge (grown to fit the wrapped text on
/// export) and the Lapis-only markers, so an exported box matches itself after re-import.
fn annotation_signature(ann: &PageAnnotation) -> String {
    if ann.kind != AnnotationKind::TextBox || ann.points.is_empty() {
        return annotation_signature_v1(ann);
    }
    let min_x = ann.points.iter().map(|p| p.0).fold(f32::INFINITY, f32::min);
    let min_y = ann.points.iter().map(|p| p.1).fold(f32::INFINITY, f32::min);
    let max_x = ann.points.iter().map(|p| p.0).fold(f32::NEG_INFINITY, f32::max);
    let q = |v: f32| (v * 10_000.0).round() as i64;
    let payload = format!(
        "TextBox2|{:?}|{}|{},{}|{}",
        ann.color, textbox_plain_text(&ann.text), q(min_x), q(min_y), q(max_x)
    );
    let mut hash = 0xcbf29ce484222325u64;
    for byte in payload.as_bytes() { hash ^= *byte as u64; hash = hash.wrapping_mul(0x100000001b3); }
    format!("{hash:016x}")
}

/// Windows-1252 byte for characters in the 0x80-0x9F block (WinAnsiEncoding).
fn winansi_high_byte(c: char) -> Option<u8> {
    Some(match c {
        '\u{20AC}' => 0x80, '\u{201A}' => 0x82, '\u{0192}' => 0x83, '\u{201E}' => 0x84,
        '\u{2026}' => 0x85, '\u{2020}' => 0x86, '\u{2021}' => 0x87, '\u{02C6}' => 0x88,
        '\u{2030}' => 0x89, '\u{0160}' => 0x8A, '\u{2039}' => 0x8B, '\u{0152}' => 0x8C,
        '\u{017D}' => 0x8E, '\u{2018}' => 0x91, '\u{2019}' => 0x92, '\u{201C}' => 0x93,
        '\u{201D}' => 0x94, '\u{2022}' => 0x95, '\u{2013}' => 0x96, '\u{2014}' => 0x97,
        '\u{02DC}' => 0x98, '\u{2122}' => 0x99, '\u{0161}' => 0x9A, '\u{203A}' => 0x9B,
        '\u{0153}' => 0x9C, '\u{017E}' => 0x9E, '\u{0178}' => 0x9F,
        _ => return None,
    })
}

/// Decodes a PDF text string: UTF-16BE or UTF-8 with a BOM, UTF-8 without one (common in
/// practice), otherwise PDFDocEncoding (Latin-1 plus the 0x80-0xA0 block).
fn decode_pdf_text(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let words: Vec<u16> = bytes[2..].chunks_exact(2).map(|p| u16::from_be_bytes([p[0], p[1]])).collect();
        return String::from_utf16_lossy(&words);
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    if let Ok(text) = std::str::from_utf8(bytes) { return text.to_owned(); }
    const HIGH: [char; 33] = [
        '\u{2022}', '\u{2020}', '\u{2021}', '\u{2026}', '\u{2014}', '\u{2013}', '\u{0192}',
        '\u{2044}', '\u{2039}', '\u{203A}', '\u{2212}', '\u{2030}', '\u{201E}', '\u{201C}',
        '\u{201D}', '\u{2018}', '\u{2019}', '\u{201A}', '\u{2122}', '\u{FB01}', '\u{FB02}',
        '\u{0141}', '\u{0152}', '\u{0160}', '\u{0178}', '\u{017D}', '\u{0131}', '\u{0142}',
        '\u{0153}', '\u{0161}', '\u{017E}', '\u{FFFD}', '\u{20AC}',
    ];
    bytes.iter().map(|&b| match b {
        0x80..=0xA0 => HIGH[(b - 0x80) as usize],
        _ => b as char,
    }).collect()
}

fn pdf_string_winansi(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        match ch {
            '(' => out.push_str(r"\("), ')' => out.push_str(r"\)"), '\\' => out.push_str(r"\\"),
            '\n' | '\r' => out.push(' '),
            c if (c as u32) >= 32 && (c as u32) <= 126 => out.push(c),
            c if (160..=255).contains(&(c as u32)) => out.push_str(&format!("\\{:03o}", c as u8)),
            c => match winansi_high_byte(c) {
                Some(byte) => out.push_str(&format!("\\{byte:03o}")),
                None => out.push('?'),
            },
        }
    }
    out
}

fn annotation_appearance_stream(
    text: &str,
    width: f32,
    height: f32,
    font_size: f32,
    ann: &PageAnnotation,
    background: Option<([u8; 3], f32)>,
) -> lopdf::Stream {
    use lopdf::{dictionary, Object};
    let font_size = font_size.clamp(4.0, 120.0);
    let leading = font_size * 1.2;
    let x = 3.0f32;
    let baseline = (height - 3.0 - font_size).max(1.0);
    let mut content = String::new();
    if let Some((c, _)) = background {
        content.push_str(&format!(
            "q /GSbg gs {:.5} {:.5} {:.5} rg 0 0 {:.3} {:.3} re f Q ",
            c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0,
            width.max(1.0), height.max(1.0)
        ));
    }
    content.push_str(&format!(
        "BT /Helv {:.3} Tf {:.5} {:.5} {:.5} rg {:.3} {:.3} Td ",
        font_size, ann.color[0] as f32 / 255.0, ann.color[1] as f32 / 255.0,
        ann.color[2] as f32 / 255.0, x, baseline
    ));
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 { content.push_str(&format!("0 -{leading:.3} Td ")); }
        content.push('(');
        content.push_str(&pdf_string_winansi(line));
        content.push_str(") Tj ");
    }
    content.push_str("ET");
    let base_font = match (ann.bold, ann.italic) {
        (true, true) => "Helvetica-BoldOblique", (true, false) => "Helvetica-Bold",
        (false, true) => "Helvetica-Oblique", (false, false) => "Helvetica",
    };
    let font = dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => base_font, "Encoding" => "WinAnsiEncoding" };
    let mut resources = dictionary! { "Font" => dictionary! { "Helv" => font } };
    if let Some((_, alpha)) = background {
        resources.set("ExtGState", dictionary! {
            "GSbg" => dictionary! { "Type" => "ExtGState", "ca" => Object::Real(alpha) }
        });
    }
    lopdf::Stream::new(dictionary! {
        "Type" => "XObject", "Subtype" => "Form", "FormType" => 1,
        "BBox" => vec![0.into(), 0.into(), Object::Real(width.max(1.0)), Object::Real(height.max(1.0))],
        "Resources" => resources,
    }, content.into_bytes())
}

fn import_external_annotations(pdf_path: &Path, target: &mut HashMap<u32, Vec<PageAnnotation>>) {
    let Ok((bytes,_)) = read_pdf_and_notes(pdf_path) else { return; };
    let Ok(doc) = lopdf::Document::load_mem(&bytes) else { return; };
    for (page_num, page_id) in doc.get_pages() {
        let Some((page_rect, rotation)) = page_pdf_geometry(&doc,page_id) else { continue; };
        let (_, disp_h) = displayed_page_size(page_rect, rotation);
        let Ok(page_dict) = doc.get_dictionary(page_id) else { continue; };
        let Some(annots_obj) = page_dict.get(b"Annots").ok().cloned() else { continue; };
        let annots_obj = resolve_obj(&doc,annots_obj);
        let Ok(annots) = annots_obj.as_array() else { continue; };
        for item in annots {
            let dict_obj = resolve_obj(&doc,item.clone());
            let Ok(dict) = dict_obj.as_dict() else { continue; };
            let is_lapis_exported = dict.get(b"LapisExported").ok().and_then(|o| o.as_bool().ok()) == Some(true);
            let subtype = dict.get(b"Subtype").ok().and_then(|o| o.as_name().ok()).unwrap_or(b"");
            let color = annotation_color(&doc,dict);
            let mut imported = Vec::new();
            match subtype {
                b"Ink" => {
                    let Some(ink) = dict.get(b"InkList").ok().cloned().map(|o| resolve_obj(&doc,o)) else { continue; };
                    let Ok(strokes) = ink.as_array() else { continue; };
                    for stroke in strokes {
                        let stroke = resolve_obj(&doc,stroke.clone());
                        let Ok(coords) = stroke.as_array() else { continue; };
                        let mut points = Vec::new();
                        for pair in coords.chunks(2) {
                            if pair.len()!=2 { continue; }
                            if let (Some(x),Some(y)) = (pdf_num(&resolve_obj(&doc,pair[0].clone())),pdf_num(&resolve_obj(&doc,pair[1].clone()))) {
                                points.push(pdf_to_lapis_point((x,y),page_rect,rotation));
                            }
                        }
                        if points.len() >= 2 { imported.push(PageAnnotation { kind: AnnotationKind::Freehand, color, points, text:String::new(), font_size:18.0, bold:false, italic:false, stroke_width:2.5 }); }
                    }
                }
                b"Highlight" => {
                    if let Some(q) = dict.get(b"QuadPoints").ok().cloned().map(|o| resolve_obj(&doc,o)) {
                        if let Ok(values) = q.as_array() {
                            let nums: Vec<f32> = values.iter().filter_map(|o| pdf_num(&resolve_obj(&doc,o.clone()))).collect();
                            for quad in nums.chunks(8) {
                                if quad.len()!=8 { continue; }
                                let pts = [(quad[0],quad[1]),(quad[2],quad[3]),(quad[4],quad[5]),(quad[6],quad[7])];
                                let norm: Vec<_> = pts.iter().map(|p|pdf_to_lapis_point(*p,page_rect,rotation)).collect();
                                let minx=norm.iter().map(|p|p.0).fold(1.0,f32::min); let maxx=norm.iter().map(|p|p.0).fold(0.0,f32::max);
                                let miny=norm.iter().map(|p|p.1).fold(1.0,f32::min); let maxy=norm.iter().map(|p|p.1).fold(0.0,f32::max);
                                imported.push(PageAnnotation { kind:AnnotationKind::Highlight,color,points:vec![(minx,miny),(maxx,maxy)],text:String::new(),font_size:18.0,bold:false,italic:false,stroke_width:2.5 });
                            }
                        }
                    }
                    if imported.is_empty() { if let Some(r)=annotation_rect(&doc,dict) { imported.push(PageAnnotation { kind:AnnotationKind::Highlight,color,points:vec![pdf_to_lapis_point((r[0],r[3]),page_rect,rotation),pdf_to_lapis_point((r[2],r[1]),page_rect,rotation)],text:String::new(),font_size:18.0,bold:false,italic:false,stroke_width:2.5 }); } }
                }
                b"FreeText" | b"Text" => {
                    if let Some(r)=annotation_rect(&doc,dict) {
                        imported.push(PageAnnotation { kind:AnnotationKind::TextBox,color,points:vec![pdf_to_lapis_point((r[0],r[3]),page_rect,rotation),pdf_to_lapis_point((r[2],r[1]),page_rect,rotation)],text:pdf_text(dict,b"Contents"),font_size:pdf_to_model_font_size(free_text_font_size(&doc,dict), disp_h),bold:false,italic:false,stroke_width:2.5 });
                    }
                }
                _ => {}
            }
            if let Some(kind_obj) = dict.get(b"LapisKind").ok() {
                let kind_name = match kind_obj {
                    lopdf::Object::String(bytes, _) => String::from_utf8_lossy(bytes).into_owned(),
                    lopdf::Object::Name(bytes) => String::from_utf8_lossy(bytes).into_owned(),
                    _ => String::new(),
                };
                if let Some(kind) = annotation_kind_from_name(&kind_name) {
                    for annotation in &mut imported { annotation.kind = kind; }
                }
            }
            let stored_sig = dict.get(b"LapisSig").ok().and_then(|o| match o {
                lopdf::Object::String(bytes, _) => Some(String::from_utf8_lossy(bytes).into_owned()),
                lopdf::Object::Name(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
                _ => None,
            });
            // Files written by older builds carry the previous signature scheme.
            let sig_matches = |ann: &PageAnnotation, sig: &str| {
                annotation_signature(ann) == sig || annotation_signature_v1(ann) == sig
            };
            if is_lapis_exported && let Some(sig) = stored_sig.as_deref() {
                // The local annotation this object was exported from, if it still exists.
                let local_idx = target.get(&page_num)
                    .and_then(|items| items.iter().position(|item| sig_matches(item, sig)));
                // Unchanged: it still has its stored signature, or the same content as the
                // local annotation (the export grows textboxes and drops Lapis-only markers).
                let unchanged = imported.iter().all(|a| sig_matches(a, sig))
                    || local_idx.is_some_and(|i| {
                        let local_sig = annotation_signature(&target[&page_num][i]);
                        imported.iter().all(|a| annotation_signature(a) == local_sig)
                    });
                if unchanged {
                    // The signature ignores the font size: if an external editor changed only
                    // the size, update the local box without resurrecting a deleted one.
                    if let (Some(i), Some(external)) = (local_idx, imported.first()) {
                        let local = &mut target.get_mut(&page_num).expect("page has annotations")[i];
                        if local.kind == AnnotationKind::TextBox && external.kind == AnnotationKind::TextBox {
                            let model = external.font_size;
                            // Older builds stored the PDF point size directly.
                            let legacy = model * disp_h / FONT_UNITS_PAGE_H;
                            if (local.font_size - model).abs() > 0.01 && (local.font_size - legacy).abs() > 0.01 {
                                local.font_size = model;
                            }
                        }
                    }
                    continue;
                }
                // Edited externally: take the new content but keep what the PDF cannot hold.
                if let Some(i) = local_idx && !imported.is_empty() {
                    let existing = target.get_mut(&page_num).expect("page has annotations");
                    let mut replacement = imported.remove(0);
                    let local = &existing[i];
                    if replacement.kind == AnnotationKind::TextBox && local.kind == AnnotationKind::TextBox {
                        replacement.bold = local.bold;
                        replacement.italic = local.italic;
                        replacement.stroke_width = local.stroke_width;
                        if let Some(marker) = textbox_leading_marker(&local.text)
                            && textbox_leading_marker(&replacement.text).is_none()
                        {
                            replacement.text.insert(0, marker);
                        }
                    }
                    existing[i] = replacement;
                }
            }
            if !imported.is_empty() {
                let existing = target.entry(page_num).or_default();
                for annotation in imported {
                    if !existing.iter().any(|current| same_page_annotation(current, &annotation)) {
                        existing.push(annotation);
                    }
                }
            }
        }
    }
}

fn pdf_rect_for_points(points: &[(f32,f32)], page_rect: [f32;4], rotation: i32) -> Option<[f32;4]> {
    if points.is_empty() { return None; }
    let mut minx=f32::INFINITY; let mut miny=f32::INFINITY; let mut maxx=f32::NEG_INFINITY; let mut maxy=f32::NEG_INFINITY;
    for p in points { let (x,y)=lapis_to_pdf_point(*p,page_rect,rotation); minx=minx.min(x); miny=miny.min(y); maxx=maxx.max(x); maxy=maxy.max(y); }
    let pad=0.0;
    Some([(minx-pad).max(page_rect[0]),(miny-pad).max(page_rect[1]),(maxx+pad).min(page_rect[2]),(maxy+pad).min(page_rect[3])])
}

fn pdf_num_obj(v:f32) -> lopdf::Object { lopdf::Object::Real(v) }
fn pdf_point_array(points:&[(f32,f32)], page_rect:[f32;4], rotation:i32) -> lopdf::Object {
    lopdf::Object::Array(points.iter().flat_map(|p| { let (x,y)=lapis_to_pdf_point(*p,page_rect,rotation); vec![pdf_num_obj(x),pdf_num_obj(y)] }).collect())
}

struct PdfAnnotationObject {
    object: lopdf::Object,
    appearance: Option<lopdf::Stream>,
}

fn wrap_annotation_text(
    text: &str,
    width: f32,
    font_size: f32,
    font_name: &str,
) -> String {
    let font = match mupdf::Font::new(font_name) {
        Ok(font) => Some(font),
        Err(err) => {
            eprintln!("Textbox font metrics unavailable: {err}");
            None
        }
    };

    let available = (width - 6.0).max(1.0);
    let normalized = text
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\t', "    ");
    let mut lines = Vec::new();

    for paragraph in normalized.split('\n') {
        let chars: Vec<char> = paragraph.chars().collect();
        if chars.is_empty() {
            lines.push(String::new());
            continue;
        }

        let widths: Vec<f32> = chars
            .iter()
            .map(|&ch| {
                font.as_ref()
                    .and_then(|font| {
                        font.encode_character(ch as i32)
                            .and_then(|glyph| font.advance_glyph(glyph))
                            .ok()
                    })
                    .filter(|advance| advance.is_finite() && *advance >= 0.0)
                    // Keep text visible if font measurement fails.
                    .unwrap_or(1.2)
                    * font_size
            })
            .collect();

        let mut start = 0;

        while start < chars.len() {
            let mut end = start;
            let mut used = 0.0;
            let mut word_break = None;

            while end < chars.len() {
                if used + widths[end] > available && end > start {
                    break;
                }

                used += widths[end];
                end += 1;

                if chars[end - 1] == ' ' {
                    word_break = Some(end);
                }
            }

            if end < chars.len() {
                if let Some(boundary) = word_break {
                    if chars[start..boundary].iter().any(|&ch| ch != ' ') {
                        end = boundary;
                    }
                }
            }

            lines.push(chars[start..end].iter().collect::<String>());
            start = end;
        }
    }

    lines.join("\n")
}

fn annotation_to_pdf_objects(ann:&PageAnnotation, page_rect:[f32;4], rotation:i32) -> Vec<PdfAnnotationObject> {
    use lopdf::{Object, dictionary};
    let sig = annotation_signature(ann);
    let kind_name = annotation_kind_name(ann.kind);
    if ann.points.is_empty() || ann.kind == AnnotationKind::Image { return Vec::new(); }
    let rgb:Vec<Object> = ann.color.iter().map(|c|Object::Real(*c as f32/255.0)).collect();
    let mut paths:Vec<Vec<(f32,f32)>> = Vec::new();
    match ann.kind {
        AnnotationKind::Highlight => {
            if ann.points.len()<2 { return Vec::new(); }
            let pts = [ann.points[0], *ann.points.last().unwrap()];
            let Some(r)=pdf_rect_for_points(&pts,page_rect,rotation) else { return Vec::new(); };
            // QuadPoints are expressed in PDF page space and ordered top-left, top-right,
            // bottom-left, bottom-right. Derive them from the final PDF rectangle so rotated
            // pages round-trip to the same normalized bounding box on import.
            let quad = [(r[0], r[3]), (r[2], r[3]), (r[0], r[1]), (r[2], r[1])];
            let q:Vec<Object> = quad.iter().flat_map(|(x,y)|vec![pdf_num_obj(*x),pdf_num_obj(*y)]).collect();
            return vec![PdfAnnotationObject { object: Object::Dictionary(dictionary!{
                "Type"=>"Annot", "Subtype"=>"Highlight", "Rect"=>vec![r[0].into(),r[1].into(),r[2].into(),r[3].into()],
                "QuadPoints"=>q, "C"=>rgb, "CA"=>Object::Real(0.38), "F"=>4, "LapisExported"=>true,
                "LapisSig"=>Object::string_literal(sig.into_bytes()), "LapisKind"=>Object::string_literal(kind_name.as_bytes().to_vec()),
            }), appearance: None }];
        }
        AnnotationKind::TextBox => {
                let Some(mut r) = pdf_rect_for_points(&ann.points, page_rect, rotation) else {
                    return Vec::new();
                };
                let (_, disp_h) = displayed_page_size(page_rect, rotation);
                let font_size = model_to_pdf_font_size(ann.font_size, disp_h);
                let font_name = match (ann.bold, ann.italic) {
                (true, true) => "Helvetica-BoldOblique",
                (true, false) => "Helvetica-Bold",
                (false, true) => "Helvetica-Oblique",
                (false, false) => "Helvetica",
            };
            let da = format!("/Helv {} Tf {} {} {} rg", font_size,
                ann.color[0] as f32 / 255.0, ann.color[1] as f32 / 255.0, ann.color[2] as f32 / 255.0);
            let font = dictionary! { "Type"=>"Font", "Subtype"=>"Type1", "BaseFont"=>font_name, "Encoding"=>"WinAnsiEncoding" };
            let dr = dictionary! { "Font"=>dictionary! { "Helv"=>font } };

            let leading = font_size * 1.2;
            let width = (r[2] - r[0]).max(1.0);
            // Background markers and inline formatting delimiters are internal to Lapis.
            let text = textbox_plain_text(&ann.text);
            let background = textbox_background(&ann.text);

            let wrapped = wrap_annotation_text(
                &text,
                width,
                font_size,
                font_name,
            );

            let line_count = wrapped.split('\n').count();
            let required_height =
                6.0 + font_size * 1.3 + leading * line_count.saturating_sub(1) as f32;
            // Fit the box to its wrapped text, as on screen; a taller stored box would show
            // as an empty band (and as extra background) in other viewers.
            let height = required_height;

            // Preserve the top edge.
            r[1] = r[3] - height;

            let appearance = annotation_appearance_stream(
                &wrapped,
                width,
                height,
                font_size,
                ann,
                background,
            );
            let mut annot = dictionary!{
                "Type"=>"Annot", "Subtype"=>"FreeText", "Rect"=>vec![r[0].into(),r[1].into(),r[2].into(),r[3].into()],
                "Contents"=>pdf_text_object(&text), "DA"=>Object::string_literal(da.into_bytes()), "DR"=>dr,
                "C"=>rgb, "F"=>4, "Q"=>0, "BS"=>dictionary!{"W"=>0}, "LapisExported"=>true,
                "LapisSig"=>Object::string_literal(sig.into_bytes()), "LapisKind"=>Object::string_literal(kind_name.as_bytes().to_vec()),
            };
            if let Some((c, _)) = background {
                // Interior color: viewers that redraw the box keep its background.
                annot.set("IC", vec![Object::Real(c[0] as f32 / 255.0), Object::Real(c[1] as f32 / 255.0), Object::Real(c[2] as f32 / 255.0)]);
            }
            return vec![PdfAnnotationObject { object: Object::Dictionary(annot), appearance: Some(appearance) }];
        }
        AnnotationKind::Rect | AnnotationKind::Tape => {
            if ann.points.len()<2 { return Vec::new(); }
            let a=ann.points[0]; let b=*ann.points.last().unwrap();
            paths.push(vec![a,(b.0,a.1),b,(a.0,b.1),a]);
        }
        AnnotationKind::Circle => {
            if ann.points.len()<2 { return Vec::new(); }
            let a=ann.points[0]; let b=*ann.points.last().unwrap();
            let cx=(a.0+b.0)*0.5; let cy=(a.1+b.1)*0.5;
            // On screen the circle is inscribed in the box (the smaller side), not an ellipse.
            let (disp_w, disp_h) = displayed_page_size(page_rect, rotation);
            let radius = ((b.0-a.0).abs()*disp_w).min((b.1-a.1).abs()*disp_h)*0.5;
            let (rx, ry) = (radius/disp_w, radius/disp_h);
            let path=(0..=32).map(|i| { let t=i as f32*std::f32::consts::TAU/32.0; (cx+rx*t.cos(),cy+ry*t.sin()) }).collect();
            paths.push(path);
        }
        _ => paths.push(ann.points.clone()),
    }
    let mut out=Vec::new();
    for path in paths {
        if path.len()<2 { continue; }
        let Some(r)=pdf_rect_for_points(&path,page_rect,rotation) else { continue; };
        let coords=pdf_point_array(&path,page_rect,rotation);
        out.push(PdfAnnotationObject { object: Object::Dictionary(dictionary!{
            "Type"=>"Annot", "Subtype"=>"Ink", "Rect"=>vec![r[0].into(),r[1].into(),r[2].into(),r[3].into()],
            "InkList"=>vec![coords], "C"=>rgb.clone(), "BS"=>dictionary!{"W"=>ann.stroke_width.clamp(0.5,24.0)},
            "CA"=>Object::Real(1.0), "F"=>4, "LapisExported"=>true,
            "LapisSig"=>Object::string_literal(sig.clone().into_bytes()), "LapisKind"=>Object::string_literal(kind_name.as_bytes().to_vec()),
        }), appearance: None });
    }
    out
}

fn export_pdf_annotations(
    incremental: &mut lopdf::IncrementalDocument,
    annotations: &HashMap<u32, Vec<PageAnnotation>>,
) -> Result<bool, String> {
    use lopdf::Object;
    let pages = incremental.get_prev_documents().get_pages();
    let mut changed = false;
    for (page_num, page_id) in pages {
        let anns = annotations.get(&page_num).map(Vec::as_slice).unwrap_or(&[]);
        let previous = incremental
            .get_prev_documents()
            .get_dictionary(page_id)
            .ok()
            .and_then(|d| d.get(b"Annots").ok())
            .cloned();
        let Some((rect, rotation)) = page_pdf_geometry(incremental.get_prev_documents(), page_id) else {
            continue;
        };
        let mut all: Vec<Object> = Vec::new();
        let mut had_lapis_export = false;
        if let Some(obj) = previous {
            let obj = resolve_obj(incremental.get_prev_documents(), obj);
            if let Ok(items) = obj.as_array() {
                for item in items {
                    let dict = resolve_obj(incremental.get_prev_documents(), item.clone());
                    let dict = dict.as_dict().ok();
                    let exported = dict
                        .and_then(|d| d.get(b"LapisExported").ok())
                        .and_then(|v| v.as_bool().ok())
                        == Some(true);
                    let subtype = dict
                        .and_then(|d| d.get(b"Subtype").ok())
                        .and_then(|v| v.as_name().ok())
                        .unwrap_or(b"");
                    let imported_supported = matches!(subtype, b"Ink" | b"Highlight" | b"FreeText" | b"Text");
                    if exported || imported_supported {
                        had_lapis_export = true;
                    } else {
                        all.push(item.clone());
                    }
                }
            }
        }
        let new_objects: Vec<PdfAnnotationObject> = anns
            .iter()
            .flat_map(|ann| annotation_to_pdf_objects(ann, rect, rotation))
            .collect();
        if !had_lapis_export && new_objects.is_empty() {
            continue;
        }
        incremental
            .opt_clone_object_to_new_document(page_id)
            .map_err(|e| e.to_string())?;
        for mut exported in new_objects {
            if let Some(appearance) = exported.appearance.take() {
                let appearance_id = incremental.new_document.add_object(Object::Stream(appearance));
                exported.object.as_dict_mut().map_err(|e| e.to_string())?.set(
                    "AP", lopdf::dictionary! { "N" => Object::Reference(appearance_id) }
                );
            }
            let id = incremental.new_document.add_object(exported.object);
            all.push(Object::Reference(id));
        }
        incremental
            .new_document
            .get_object_mut(page_id)
            .and_then(Object::as_dict_mut)
            .map_err(|e| e.to_string())?
            .set("Annots", Object::Array(all));
        changed = true;
    }
    Ok(changed)
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
        let Some(head) = core.get(..kw.len()) else {
            continue;
        }; // None se non è char boundary
        if core.len() <= kw.len() || !head.eq_ignore_ascii_case(kw) {
            continue;
        }
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
            .trim_start_matches([':', '-', '—', '–', ')', ']'])
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
    let rest = line
        .trim()
        .strip_prefix('#')?
        .trim_start_matches('#')
        .trim_start();
    rest.strip_prefix("Slide ")?
        .trim()
        .parse()
        .ok()
        .filter(|&n| n > 0)
}

/// Imports notes from a `.txt` or `.md` file, mapping each section to its slide number.
/// `max_pages` bounds the page numbers so out-of-range slides are ignored safely.
pub fn import_notes_from_file(path: &Path, max_pages: u32) -> io::Result<HashMap<u32, String>> {
    let text = fs::read_to_string(path)?;
    let strict = text
        .lines()
        .next()
        .is_some_and(|l| l.starts_with("<!-- Notes exported from Lapis"));

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
        let hdr = if strict {
            parse_lapis_header(line).map(|n| (n, None))
        } else {
            parse_slide_header(line)
        };
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
            if let Some(after_den) = rest.strip_prefix('{')
                && let Some(close_den) = after_den.find('}')
            {
                let den = &after_den[..close_den];
                let replacement = format!("({}/{})", num, den);
                let total_len = 6 + close_num + 1 + 1 + close_den + 1;
                s.replace_range(pos..pos + total_len, &replacement);
                continue;
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
        ("\\mathbb{R}", "ℝ"),
        ("\\mathbb{N}", "ℕ"),
        ("\\mathbb{Z}", "ℤ"),
        ("\\mathbb{Q}", "ℚ"),
        ("\\mathbb{C}", "ℂ"),
        ("\\mathbb{P}", "ℙ"),
        ("\\mathbb{E}", "𝔼"),
        ("\\mathbb{F}", "𝔽"),
        ("\\mathbb{1}", "𝟙"),
        ("\\Re", "ℜ"),
        ("\\Im", "ℑ"),
        ("\\ell", "ℓ"),
        ("\\hbar", "ℏ"),
        ("\\wp", "℘"),
        ("\\aleph", "ℵ"),
        // Frecce e logica
        ("\\Longleftrightarrow", "⟺"),
        ("\\Leftrightarrow", "⇔"),
        ("\\longrightarrow", "⟶"),
        ("\\Longrightarrow", "⟹"),
        ("\\longleftarrow", "⟵"),
        ("\\Longleftarrow", "⟸"),
        ("\\longmapsto", "⟼"),
        ("\\leftrightarrow", "↔"),
        ("\\rightleftharpoons", "⇌"),
        ("\\hookrightarrow", "↪"),
        ("\\hookleftarrow", "↩"),
        ("\\twoheadrightarrow", "↠"),
        ("\\rightarrow", "→"),
        ("\\Rightarrow", "⇒"),
        ("\\leftarrow", "←"),
        ("\\Leftarrow", "⇐"),
        ("\\uparrow", "↑"),
        ("\\Uparrow", "⇑"),
        ("\\downarrow", "↓"),
        ("\\Downarrow", "⇓"),
        ("\\updownarrow", "↕"),
        ("\\nearrow", "↗"),
        ("\\searrow", "↘"),
        ("\\swarrow", "↙"),
        ("\\nwarrow", "↖"),
        ("\\mapsto", "↦"),
        ("\\implies", "⇒"),
        ("\\impliedby", "⇐"),
        ("\\iff", "⇔"),
        ("\\to", "→"),
        ("\\gets", "←"),
        // Quantificatori e operatori logici
        ("\\forall", "∀"),
        ("\\exists", "∃"),
        ("\\nexists", "∄"),
        ("\\therefore", "∴"),
        ("\\because", "∵"),
        ("\\land", "∧"),
        ("\\wedge", "∧"),
        ("\\bigwedge", "⋀"),
        ("\\lor", "∨"),
        ("\\vee", "∨"),
        ("\\bigvee", "⋁"),
        ("\\neg", "¬"),
        ("\\lnot", "¬"),
        ("\\top", "⊤"),
        ("\\bot", "⊥"),
        ("\\vdash", "⊢"),
        ("\\models", "⊨"),
        // Insiemi e relazioni
        ("\\subseteq", "⊆"),
        ("\\supseteq", "⊇"),
        ("\\subsetneq", "⊊"),
        ("\\supsetneq", "⊋"),
        ("\\nsubseteq", "⊈"),
        ("\\nsupseteq", "⊉"),
        ("\\subset", "⊂"),
        ("\\supset", "⊃"),
        ("\\emptyset", "∅"),
        ("\\varnothing", "∅"),
        ("\\notin", "∉"),
        ("\\in", "∈"),
        ("\\ni", "∋"),
        ("\\bigcup", "⋃"),
        ("\\bigcap", "⋂"),
        ("\\bigsqcup", "⨆"),
        ("\\bigoplus", "⨁"),
        ("\\bigotimes", "⨂"),
        ("\\cup", "∪"),
        ("\\cap", "∩"),
        ("\\setminus", "∖"),
        // Analysis and calculation.
        ("\\iiint", "∭"),
        ("\\iint", "∬"),
        ("\\oint", "∮"),
        ("\\int", "∫"),
        ("\\sum", "∑"),
        ("\\prod", "∏"),
        ("\\coprod", "∐"),
        ("\\partial", "∂"),
        ("\\nabla", "∇"),
        ("\\infty", "∞"),
        ("\\infinity", "∞"),
        ("\\sqrt[3]", "∛"),
        ("\\sqrt[4]", "∜"),
        ("\\sqrt", "√"),
        ("\\lim", "lim"),
        ("\\sup", "sup"),
        ("\\inf", "inf"),
        ("\\max", "max"),
        ("\\min", "min"),
        ("\\det", "det"),
        ("\\dim", "dim"),
        ("\\ker", "ker"),
        ("\\deg", "deg"),
        ("\\sin", "sin"),
        ("\\cos", "cos"),
        ("\\tan", "tan"),
        ("\\arcsin", "arcsin"),
        ("\\arccos", "arccos"),
        ("\\arctan", "arctan"),
        ("\\sinh", "sinh"),
        ("\\cosh", "cosh"),
        ("\\tanh", "tanh"),
        ("\\log", "log"),
        ("\\ln", "ln"),
        ("\\exp", "exp"),
        // Confronti ed equivalenze
        ("\\approxeq", "≊"),
        ("\\approx", "≈"),
        ("\\simeq", "≃"),
        ("\\cong", "≅"),
        ("\\equiv", "≡"),
        ("\\not\\equiv", "≢"),
        ("\\propto", "∝"),
        ("\\sim", "∼"),
        ("\\asymp", "≍"),
        ("\\leqslant", "⩽"),
        ("\\geqslant", "⩾"),
        ("\\leq", "≤"),
        ("\\geq", "≥"),
        ("\\le", "≤"),
        ("\\ge", "≥"),
        ("\\neq", "≠"),
        ("\\ne", "≠"),
        ("\\ll", "≪"),
        ("\\gg", "≫"),
        ("\\prec", "≺"),
        ("\\succ", "≻"),
        ("\\preceq", "≼"),
        ("\\succeq", "≽"),
        ("\\perp", "⊥"),
        ("\\parallel", "∥"),
        ("\\mid", "∣"),
        ("\\nmid", "∤"),
        // Operatori binari e delimitatori
        ("\\otimes", "⊗"),
        ("\\oplus", "⊕"),
        ("\\ominus", "⊖"),
        ("\\odot", "⊙"),
        ("\\circ", "∘"),
        ("\\bullet", "∙"),
        ("\\times", "×"),
        ("\\div", "÷"),
        ("\\cdot", "·"),
        ("\\pm", "±"),
        ("\\mp", "∓"),
        ("\\star", "⋆"),
        ("\\ast", "∗"),
        ("\\langle", "⟨"),
        ("\\rangle", "⟩"),
        ("\\lceil", "⌈"),
        ("\\rceil", "⌉"),
        ("\\lfloor", "⌊"),
        ("\\rfloor", "⌋"),
        ("\\ldots", "…"),
        ("\\cdots", "⋯"),
        ("\\vdots", "⋮"),
        ("\\ddots", "⋱"),
        ("\\prime", "′"),
        ("\\degree", "°"),
        ("\\circ", "°"),
        ("\\triangle", "△"),
        ("\\square", "□"),
        ("\\diamond", "⋄"),
        ("\\angle", "∠"),
        // Alfabeto greco
        ("\\varepsilon", "ε"),
        ("\\epsilon", "ϵ"),
        ("\\vartheta", "ϑ"),
        ("\\theta", "θ"),
        ("\\Theta", "Θ"),
        ("\\varpi", "ϖ"),
        ("\\pi", "π"),
        ("\\Pi", "Π"),
        ("\\varrho", "ϱ"),
        ("\\rho", "ρ"),
        ("\\varsigma", "ς"),
        ("\\sigma", "σ"),
        ("\\Sigma", "Σ"),
        ("\\varphi", "φ"),
        ("\\phi", "ϕ"),
        ("\\Phi", "Φ"),
        ("\\alpha", "α"),
        ("\\beta", "β"),
        ("\\gamma", "γ"),
        ("\\Gamma", "Γ"),
        ("\\delta", "δ"),
        ("\\Delta", "Δ"),
        ("\\zeta", "ζ"),
        ("\\eta", "η"),
        ("\\iota", "ι"),
        ("\\kappa", "κ"),
        ("\\lambda", "λ"),
        ("\\Lambda", "Λ"),
        ("\\mu", "μ"),
        ("\\nu", "ν"),
        ("\\xi", "ξ"),
        ("\\Xi", "Ξ"),
        ("\\tau", "τ"),
        ("\\upsilon", "υ"),
        ("\\Upsilon", "Υ"),
        ("\\chi", "χ"),
        ("\\psi", "ψ"),
        ("\\Psi", "Ψ"),
        ("\\omega", "ω"),
        ("\\Omega", "Ω"),
        ("\\left(", "("),
        ("\\right)", ")"),
        ("\\left[", "["),
        ("\\right]", "]"),
        ("\\left\\{", "{"),
        ("\\right\\}", "}"),
        ("\\left|", "|"),
        ("\\right|", "|"),
        ("\\,", " "),
        ("\\;", " "),
        ("\\quad", "  "),
        ("\\qquad", "    "),
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

    out.replace(['{', '}'], "").replace(':', "∶")
}

/// Prepare notes for Preview, using the same card boundaries as the trainer.
pub fn preprocess_notes_preview(markdown: &str) -> String {
    fn quote(out: &mut String, text: &str) {
        for line in text.trim_matches('\n').lines() {
            out.push_str("> ");
            out.push_str(line);
            out.push('\n');
        }
    }

    if !markdown.contains("::") && !markdown.contains("Q:") && !markdown.contains("D:") {
        return preprocess_markdown_view(markdown);
    }
    let cards = flashcard_spans(0, markdown);
    if cards.is_empty() {
        return preprocess_markdown_view(markdown);
    }
    let mut out = String::with_capacity(markdown.len() + 128);
    let mut pos = 0;
    for (span, card, inline) in cards {
        out.push_str(&preprocess_markdown_view(&markdown[pos..span.start]));
        out.push('\n');
        // Prepare math and code before adding the blockquote prefix.
        let front = preprocess_markdown_view(&card.front);
        let back = preprocess_markdown_view(&card.back);
        if inline {
            // Avoid nesting emphasis around an already formatted title.
            let front = front.trim();
            if !front.contains(['*', '_', '\n']) {
                quote(&mut out, &format!("**{front}**  "));
            } else {
                quote(&mut out, &format!("{front}  "));
            }
        } else {
            out.push_str("> **Domanda**  \n");
            quote(&mut out, &front);
            out.push_str(">\n> **Risposta**  \n");
        }
        // Separate the answer from the title, including fenced code and display math.
        out.push_str(">\n");
        quote(&mut out, &back);
        out.push('\n');
        pos = span.end;
    }
    out.push_str(&preprocess_markdown_view(&markdown[pos..]));
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
        if rest.starts_with("```") && (in_code_block || !rest.trim_start_matches('`').contains('`')) {
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
    alias_code_fences(&preprocess_markdown_math_impl(markdown, true))
}

// Convert display and inline math into styled Markdown for egui_commonmark.

pub fn preprocess_markdown_math(markdown: &str) -> String {
    preprocess_markdown_math_impl(markdown, false)
}

fn preprocess_markdown_math_impl(markdown: &str, escape_shell: bool) -> String {
    fn push_literal(out: &mut String, text: &str, escape_shell: bool) {
        if !escape_shell {
            out.push_str(text);
            return;
        }
        for (i, ch) in text.char_indices() {
            if ch == '$' && text[i..].starts_with("$@") {
                let escapes = text.as_bytes()[..i].iter().rev().take_while(|&&b| b == b'\\').count();
                if escapes % 2 == 0 {
                    out.push('\\');
                }
            }
            out.push(ch);
        }
    }

    let mut result = String::with_capacity(markdown.len() + 64);
    let mut pos = 0;
    for range in code_and_math_ranges(markdown) {
        push_literal(&mut result, &markdown[pos..range.start], escape_shell);
        let span = &markdown[range.clone()];
        if span.starts_with("$$") {
            let formatted = format_latex_expr(&span[2..span.len() - 2]);
            result.push_str(&format!("\n\n**`{}`**\n\n", formatted));
        } else if span.starts_with('$') {
            let formatted = format_latex_expr(&span[1..span.len() - 1]);
            result.push_str(&format!("`{}`", formatted));
        } else {
            result.push_str(span);
        }
        pos = range.end;
    }
    push_literal(&mut result, &markdown[pos..], escape_shell);

    let mut final_out = String::with_capacity(result.len());
    let mut fence: Option<(char, usize)> = None;
    let lines: Vec<&str> = result.lines().collect();
    for (line_idx, &line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if let Some(marker) = trimmed.chars().next().filter(|c| *c == '`' || *c == '~') {
            let run = trimmed.chars().take_while(|&c| c == marker).count();
            if let Some((open, len)) = fence {
                if marker == open && run >= len && trimmed[run..].trim().is_empty() {
                    fence = None;
                    final_out.push_str(line);
                    final_out.push('\n');
                    continue;
                }
            } else if run >= 3 && (marker != '`' || !trimmed[run..].contains('`')) {
                fence = Some((marker, run));
            }
        }
        final_out.push_str(line);
        if fence.is_some() {
            final_out.push('\n');
            continue;
        }

        let is_blockquote = trimmed.starts_with('>');
        let next_is_blockquote = lines
            .get(line_idx + 1)
            .is_some_and(|next| next.trim().starts_with('>'));

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
    if let Some(&last) = raw.last()
        && filtered.last() != Some(&last)
    {
        filtered.push(last);
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

/// Byte ranges of `text` that hold code (fenced blocks, inline code) or LaTeX (`$$…$$`, `$…$`).
/// Sorted and disjoint. Inline math follows the rules of [`preprocess_markdown_math`]; an
/// unclosed fence runs to the end, as in Markdown, while unclosed math is plain text.
pub fn code_and_math_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    fn line_end(text: &str, pos: usize) -> usize {
        text[pos..].find('\n').map_or(text.len(), |p| pos + p + 1)
    }
    /// End of the fenced block opening at the line starting at `start`, if there is one.
    fn fence_end(text: &str, start: usize) -> Option<usize> {
        let first_end = line_end(text, start);
        let opening = text[start..first_end].trim_start();
        let marker = opening.chars().next().filter(|c| *c == '`' || *c == '~')?;
        let run = opening.chars().take_while(|&c| c == marker).count();
        if run < 3 || (marker == '`' && opening[run..].contains('`')) {
            return None;
        }
        let mut pos = first_end;
        while pos < text.len() {
            let end = line_end(text, pos);
            let closing = text[pos..end].trim_start();
            let len = closing.chars().take_while(|&c| c == marker).count();
            if len >= run && closing[len..].trim().is_empty() {
                return Some(end);
            }
            pos = end;
        }
        Some(text.len())
    }
    /// End of the first run of exactly `run` backticks in `from..limit`.
    fn closing_backticks(bytes: &[u8], from: usize, limit: usize, run: usize) -> Option<usize> {
        let mut j = from;
        while j < limit {
            if bytes[j] == b'`' {
                let mut k = j;
                while k < limit && bytes[k] == b'`' {
                    k += 1;
                }
                if k - j == run {
                    return Some(k);
                }
                j = k;
            } else {
                j += 1;
            }
        }
        None
    }

    let bytes = text.as_bytes();
    let mut ranges = Vec::new();
    let mut i = 0;
    let mut at_line_start = true;
    while i < bytes.len() {
        if at_line_start {
            at_line_start = false;
            if let Some(end) = fence_end(text, i) {
                ranges.push(i..end);
                i = end;
                at_line_start = true;
                continue;
            }
        }
        match bytes[i] {
            b'\n' => {
                at_line_start = true;
                i += 1;
            }
            b'\\' if matches!(bytes.get(i + 1), Some(b'$' | b'\\' | b'`')) => i += 2,
            b'`' => {
                let run = bytes[i..].iter().take_while(|&&b| b == b'`').count();
                match closing_backticks(bytes, i + run, line_end(text, i), run) {
                    Some(end) => {
                        ranges.push(i..end);
                        i = end;
                    }
                    None => i += run,
                }
            }
            b'$' if text[i..].starts_with("$@") => i += 2,
            b'$' if text[i..].starts_with("$$") => match text[i + 2..].find("$$") {
                Some(p) => {
                    let end = i + 2 + p + 2;
                    ranges.push(i..end);
                    i = end;
                }
                None => i += 2,
            },
            b'$' => {
                let after = i + 1;
                let closing = text[after..line_end(text, i)]
                    .find(['$', '`']).map(|p| after + p)
                    .filter(|&p| bytes[p] == b'$' && !text[p..].starts_with("$@")
                        && bytes[..p].iter().rev().take_while(|&&b| b == b'\\').count() % 2 == 0);
                match closing {
                    Some(close)
                        if close > after
                            && !text[after..close].starts_with(' ')
                            && !text[after..close].ends_with(' ') =>
                    {
                        ranges.push(i..close + 1);
                        i = close + 1;
                    }
                    _ => i += 1,
                }
            }
            _ => i += 1,
        }
    }
    ranges
}

/// Whether byte offset `byte` lies inside one of the sorted, disjoint `ranges`.
pub fn in_ranges(ranges: &[std::ops::Range<usize>], byte: usize) -> bool {
    let idx = ranges.partition_point(|r| r.end <= byte);
    ranges.get(idx).is_some_and(|r| r.start <= byte)
}

/// Offset in `hay` of the first `needle` outside code and math. `base` is `hay`'s offset in the note.
fn find_outside(
    hay: &str,
    base: usize,
    needle: &str,
    skip: &[std::ops::Range<usize>],
) -> Option<usize> {
    hay.match_indices(needle)
        .map(|(i, _)| i)
        .find(|&i| !in_ranges(skip, base + i))
}

/// Whether a note contains at least one flashcard (cheap pre-check; runs every frame).
pub fn page_has_flashcards(content: &str) -> bool {
    (content.contains("::") || content.contains("Q:") || content.contains("D:"))
        && !extract_flashcards_from_page(0, content).is_empty()
}

fn extract_flashcards_from_page(page: u32, content: &str) -> Vec<Flashcard> {
    flashcard_spans(page, content)
        .into_iter()
        .map(|(_, card, _)| card)
        .collect()
}

// The span covers the original source; the bool distinguishes inline and Q/A cards.
fn flashcard_spans(page: u32, content: &str) -> Vec<(std::ops::Range<usize>, Flashcard, bool)> {
    let mut cards = Vec::new();
    // Code and math never produce cards; offsets let us tell where a marker sits.
    let skip = code_and_math_ranges(content);
    let mut offset = 0;
    let mut lines = content
        .split_inclusive('\n')
        .map(|l| {
            let start = offset;
            offset += l.len();
            (start, l)
        })
        .peekable();
    while let Some((line_off, raw_line)) = lines.next() {
        let trimmed = strip_list_marker(raw_line);
        // `trimmed` is a subslice of `raw_line`.
        let base = line_off + (trimmed.as_ptr() as usize - raw_line.as_ptr() as usize);

        let question_start = if !in_ranges(&skip, base) {
            trimmed.strip_prefix("Q:")
                .or_else(|| trimmed.strip_prefix("**Q:**"))
                .or_else(|| trimmed.strip_prefix("D:"))
        } else {
            None
        };
        // Inline cards must not split a multiline math or code span.
        let line_end = line_off + raw_line.len();
        let first = skip.partition_point(|r| r.end <= line_off);
        let crosses_line = skip[first..]
            .iter()
            .take_while(|r| r.start < line_end)
            .any(|r| r.start < line_off || r.end > line_end);
        if !crosses_line && let Some(p) = find_outside(trimmed, base, "::", &skip) {
            let f = strip_title_emphasis(&trimmed[..p]);
            let b = trimmed[p + 2..].trim();
            if !f.is_empty() && !b.is_empty() {
                cards.push((
                    line_off..line_end,
                    Flashcard { page, front: f.to_string(), back: b.to_string() },
                    true,
                ));
                continue;
            }
        }

        // 2. Check `Q: ...` / `A: ...` block syntax
        if let Some(q_start) = question_start {
            let mut end = line_off + raw_line.len();
            let mut question = q_start.trim().to_string();
            let mut answer = String::new();
            let mut in_answer = false;

            while let Some(&(next_off, next_raw)) = lines.peek() {
                let next_trim = next_raw.trim();
                let lead = next_raw.len() - next_raw.trim_start().len();
                // Markers inside a code block or math are content, not syntax.
                let is_syntax = !in_ranges(&skip, next_off + lead);
                if next_trim.is_empty() && in_answer && !in_ranges(&skip, next_off) {
                    break;
                }
                let a_start = if is_syntax {
                    next_trim.strip_prefix("A:")
                        .or_else(|| next_trim.strip_prefix("**A:**"))
                        .or_else(|| next_trim.strip_prefix("R:"))
                } else {
                    None
                };
                if (is_syntax && (next_trim.starts_with("Q:")
                    || next_trim.starts_with("**Q:**") || next_trim.starts_with("D:")))
                    || (a_start.is_none() && find_outside(next_raw, next_off, "::", &skip).is_some())
                {
                    break;
                }

                let consumed = lines.next().unwrap().1.trim_end_matches(['\r', '\n']);
                end = next_off + next_raw.len();
                if let Some(a_start) = a_start {
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
                cards.push((
                    line_off..end,
                    Flashcard { page, front: question, back: answer },
                    false,
                ));
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
        if can_close && let Some(pos) = stack.iter().rposition(|&o| runs[o].1 == len) {
            let o = stack.remove(pos);
            matched[o] = true;
            matched[ri] = true;
            continue;
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
    use lopdf::{Document as LoDoc, Object, Stream, dictionary};

    let mut doc = LoDoc::with_version("1.5");
    let pages_id = doc.new_object_id();

    // Light ruled grid for 16:9 lecture slides (960 × 540).
    let mut grid_ops = String::from("q 0.88 0.90 0.94 RG 0.5 w ");
    for y in (24..520).step_by(24) {
        grid_ops.push_str(&format!("20 {} m 940 {} l S ", y, y));
    }
    grid_ops.push('Q');
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
        .map_err(|e| io::Error::other(e.to_string()))?;
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
    let fname = pdf_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("document.pdf");
    original_storage_dir().join(format!("{key}_{fname}.orig"))
}

/// Legacy path used only to migrate older backups.
fn legacy_original_backup_path(pdf_path: &Path) -> PathBuf {
    let parent = pdf_path.parent().unwrap_or_else(|| Path::new("."));
    let fname = pdf_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("document.pdf");
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
        if legacy.exists() {
            let _ = fs::remove_file(legacy);
        }
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
        if let Some(parent) = backup.parent() {
            fs::create_dir_all(parent)?;
        }
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
pub(crate) fn page_inherited(
    doc: &lopdf::Document,
    mut id: lopdf::ObjectId,
    key: &[u8],
) -> Option<lopdf::Object> {
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
        lopdf::Object::Reference(r) => doc.get_object(r).cloned().unwrap_or(lopdf::Object::Null),
        other => other,
    }
}

/// Create a Flate-compressed image XObject and return its object ID.
pub(crate) fn add_image_xobject(
    doc: &mut lopdf::Document,
    shot: &crate::screenshots::Shot,
) -> lopdf::ObjectId {
    use lopdf::{Object, Stream, dictionary};

    let (iw, ih) = (shot.width as i64, shot.height as i64);
    let n = (shot.width as usize) * (shot.height as usize);
    let mut rgb = Vec::with_capacity(n * 3);
    let mut alpha = Vec::with_capacity(n);
    let mut has_alpha = false;
    for px in shot.rgba.as_chunks::<4>().0.iter().take(n) {
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

    let mut res: Dictionary =
        match page_inherited(doc, page_id, b"Resources").map(|o| resolve_obj(doc, o)) {
            Some(Object::Dictionary(d)) => d,
            _ => Dictionary::new(),
        };
    let mut xobjects: Dictionary = match res
        .get(b"XObject")
        .ok()
        .cloned()
        .map(|o| resolve_obj(doc, o))
    {
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
                let Some(shot) = crate::screenshots::Shot::from_file(&media_dir.join(&ann.text))
                else {
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
    use crate::test_support::{check_pdf_with_qpdf, isolated_dir};
    use std::fs::OpenOptions;

    fn append_test_custom_data(path: &Path, data: &DocData) {
        let mut f = OpenOptions::new().append(true).open(path).unwrap();
        f.write_all(NOTES_START_TAG).unwrap();
        serde_json::to_writer(&mut f, data).unwrap();
        f.write_all(NOTES_END_TAG).unwrap();
    }

    fn valid_test_pdf(path: &Path) {
        use lopdf::{dictionary, Object};
        let mut doc = lopdf::Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        });
        doc.objects.insert(pages_id, Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![Object::Reference(page_id)],
            "Count" => 1,
        }));
        let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", catalog);
        doc.save(path).unwrap();
    }

    fn count_custom_sections(bytes: &[u8]) -> usize {
        bytes.windows(NOTES_START_TAG.len()).filter(|w| *w == NOTES_START_TAG).count()
    }

    #[test]
    fn edge_freetext_uses_pdf_font_size_instead_of_lapis_default() {
        use lopdf::{dictionary, Object};
        let doc = lopdf::Document::with_version("1.7");
        let da = dictionary! { "DA" => Object::string_literal(b"0 0 0 rg /Arial 11.5 Tf".to_vec()) };
        assert!((free_text_font_size(&doc, &da) - 11.5).abs() < 0.001);
        let multiple = dictionary! { "DA" => Object::string_literal(b"/Helv 9 Tf /Arial 23 Tf".to_vec()) };
        assert_eq!(free_text_font_size(&doc, &multiple), 23.0);
        let style = dictionary! { "DS" => Object::string_literal(b"font-family: Arial; font-size: 14pt; color: black".to_vec()) };
        assert_eq!(free_text_font_size(&doc, &style), 14.0);
        let missing = dictionary! {};
        assert_eq!(free_text_font_size(&doc, &missing), 16.0);
    }

    #[test]
    fn edge_freetext_import_and_font_size_only_external_edit_sync() {
        use lopdf::{dictionary, Object};
        let root = std::env::temp_dir().join(format!("lapis_edge_font_{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("font.pdf");
        valid_test_pdf(&path);
        let mut doc = lopdf::Document::load(&path).unwrap();
        let page_id = *doc.get_pages().get(&1).unwrap();
        let sample = PageAnnotation {
            kind: AnnotationKind::TextBox, color: [0, 0, 0],
            points: vec![(0.1, 0.1), (0.4, 0.3)], text: "Edge text".to_owned(),
            font_size: 16.0, bold: false, italic: false, stroke_width: 2.5,
        };
        let signature = annotation_signature(&sample);
        let annotation_id = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "FreeText",
            "Rect" => vec![Object::Real(61.2), Object::Real(554.4), Object::Real(244.8), Object::Real(712.8)],
            "Contents" => Object::string_literal(b"Edge text".to_vec()),
            "DA" => Object::string_literal(b"/Helv 26 Tf 0 0 0 rg".to_vec()),
            "LapisExported" => true,
            "LapisSig" => Object::string_literal(signature.into_bytes()),
            "LapisKind" => Object::string_literal(b"TextBox".to_vec()),
        });
        doc.get_object_mut(page_id).unwrap().as_dict_mut().unwrap()
            .set("Annots", vec![Object::Reference(annotation_id)]);
        doc.save(&path).unwrap();

        // Edge retains the Lapis signature when changing only the text size.
        // The previously saved local annotation must be updated in place.
        let mut local = HashMap::from([(1, vec![sample])]);
        import_external_annotations(&path, &mut local);
        assert_eq!(local[&1].len(), 1);
        // 26 pt on a 792 pt page is stored relative to the 800-unit page height.
        assert!((local[&1][0].font_size - 26.0 * 800.0 / 792.0).abs() < 0.01);
        let _ = fs::remove_dir_all(&root);
    }

    fn sample_textbox(text: &str, points: Vec<(f32, f32)>) -> PageAnnotation {
        PageAnnotation {
            kind: AnnotationKind::TextBox, color: [10, 20, 30], points,
            text: text.to_owned(), font_size: 20.0, bold: true, italic: true, stroke_width: 3.0,
        }
    }

    #[test]
    fn textbox_plain_text_matches_the_on_screen_rendering() {
        assert_eq!(textbox_plain_text("\u{200B}- a __b__ ~~c~~"), "\u{2022} a b c");
        assert_eq!(textbox_plain_text("- one\ntwo\n- three"), "\u{2022} one\ntwo\n\u{2022} three");
        // A line that starts with a delimiter is not a bullet.
        assert_eq!(textbox_plain_text("__- x__"), "- x");
        assert_eq!(textbox_background("\u{200B}x").map(|b| b.0), Some([36, 38, 46]));
        assert_eq!(textbox_background("\u{200C}x").map(|b| b.0), Some([245, 246, 250]));
        assert!(textbox_background("plain").is_none());
    }

    #[test]
    fn textbox_signature_ignores_height_and_lapis_markers() {
        let local = sample_textbox("\u{200B}hi __x__", vec![(0.1, 0.1), (0.4, 0.2)]);
        // What an export-then-import produces: grown box, cleaned text.
        let imported = sample_textbox("hi x", vec![(0.1, 0.1), (0.4, 0.5)]);
        assert_eq!(annotation_signature(&local), annotation_signature(&imported));
        assert_ne!(annotation_signature_v1(&local), annotation_signature_v1(&imported));
        let moved = sample_textbox("hi x", vec![(0.2, 0.1), (0.5, 0.5)]);
        assert_ne!(annotation_signature(&local), annotation_signature(&moved));
        let edited = sample_textbox("hi there", vec![(0.1, 0.1), (0.4, 0.2)]);
        assert_ne!(annotation_signature(&local), annotation_signature(&edited));
    }

    #[test]
    fn appearance_text_uses_winansi_for_typographic_characters() {
        assert_eq!(
            pdf_string_winansi("\u{2019} \u{201C} \u{201D} \u{2013} \u{2022} \u{E9} ("),
            r"\222 \223 \224 \226 \225 \351 \("
        );
        assert_eq!(pdf_string_winansi("\u{4E2D}"), "?");
    }

    #[test]
    fn pdf_text_without_a_bom_is_decoded_as_utf8_or_pdfdoc() {
        assert_eq!(decode_pdf_text(b"caf\xE9"), "caf\u{E9}");
        assert_eq!(decode_pdf_text("caf\u{E9}".as_bytes()), "caf\u{E9}");
        assert_eq!(decode_pdf_text(&[0xFE, 0xFF, 0x00, 0x41]), "A");
        assert_eq!(decode_pdf_text(&[0x80, 0x90]), "\u{2022}\u{2019}");
        assert_eq!(decode_pdf_text(&[0xEF, 0xBB, 0xBF, b'o', b'k']), "ok");
    }

    #[test]
    fn font_sizes_follow_the_page_height() {
        assert!((model_to_pdf_font_size(16.0, 540.0) - 10.8).abs() < 0.001);
        assert!((model_to_pdf_font_size(16.0, 800.0) - 16.0).abs() < 0.001);
        assert!((pdf_to_model_font_size(10.8, 540.0) - 16.0).abs() < 0.001);
        // Rotated pages use the displayed height (the PDF width).
        assert_eq!(displayed_page_size([0.0, 0.0, 540.0, 960.0], 90), (960.0, 540.0));
        assert_eq!(displayed_page_size([0.0, 0.0, 540.0, 960.0], 0), (540.0, 960.0));
    }

    #[test]
    fn exported_textbox_scales_font_and_keeps_its_background() {
        let ann = sample_textbox("\u{200B}Hello", vec![(0.1, 0.1), (0.5, 0.2)]);
        let objects = annotation_to_pdf_objects(&ann, [0.0, 0.0, 960.0, 540.0], 0);
        assert_eq!(objects.len(), 1);
        let dict = objects[0].object.as_dict().unwrap();
        let Ok(lopdf::Object::String(da, _)) = dict.get(b"DA") else { panic!("missing /DA") };
        let da = String::from_utf8_lossy(da).into_owned();
        let size: f32 = da.split_whitespace().nth(1).unwrap().parse().unwrap();
        assert!((size - 20.0 * 540.0 / 800.0).abs() < 0.001, "{da}");
        assert!(dict.get(b"IC").is_ok());
        let appearance = objects[0].appearance.as_ref().unwrap();
        assert!(String::from_utf8_lossy(&appearance.content).contains("/GSbg gs"));
        let Ok(lopdf::Object::String(contents, _)) = dict.get(b"Contents") else { panic!("missing /Contents") };
        assert_eq!(decode_pdf_text(contents), "Hello");
    }

    #[test]
    fn exported_textbox_is_fitted_to_its_text() {
        let page = [0.0, 0.0, 960.0, 540.0];
        let rect_of = |ann: &PageAnnotation| -> Vec<f32> {
            let objects = annotation_to_pdf_objects(ann, page, 0);
            let rect = objects[0].object.as_dict().unwrap().get(b"Rect").unwrap().as_array().unwrap();
            rect.iter().map(|o| pdf_num(o).unwrap()).collect()
        };
        // The sample box uses size 20, which is 13.5 pt on a 540 pt page.
        let pt = 20.0 * 540.0 / 800.0;
        // A tall stored box still exports at the height of one line.
        let tall = rect_of(&sample_textbox("Hi", vec![(0.1, 0.1), (0.5, 0.9)]));
        assert!(((tall[3] - tall[1]) - (6.0 + pt * 1.3)).abs() < 0.01);
        // The top edge stays where it was placed.
        assert!((tall[3] - 540.0 * 0.9).abs() < 0.01);
        // Each extra line adds one leading (1.2 x the text size).
        let three = rect_of(&sample_textbox("a\nb\nc", vec![(0.1, 0.1), (0.5, 0.9)]));
        assert!(((three[3] - three[1]) - (6.0 + pt * 1.3 + pt * 1.2 * 2.0)).abs() < 0.01);
    }

    #[test]
    fn exported_circle_is_inscribed_like_on_screen() {
        let ann = PageAnnotation {
            kind: AnnotationKind::Circle, color: [0, 0, 0],
            points: vec![(0.1, 0.1), (0.5, 0.2)], text: String::new(),
            font_size: 16.0, bold: false, italic: false, stroke_width: 2.5,
        };
        let objects = annotation_to_pdf_objects(&ann, [0.0, 0.0, 612.0, 792.0], 0);
        let rect = objects[0].object.as_dict().unwrap().get(b"Rect").unwrap().as_array().unwrap();
        let v: Vec<f32> = rect.iter().map(|o| pdf_num(o).unwrap()).collect();
        // Box is 244.8 x 79.2 pt: the circle takes the smaller side, in both directions.
        assert!(((v[2] - v[0]) - 79.2).abs() < 0.01 && ((v[3] - v[1]) - 79.2).abs() < 0.01);
    }

    fn export_to_file(path: &Path, annotations: &HashMap<u32, Vec<PageAnnotation>>) {
        let base = fs::read(path).unwrap();
        let mut incremental =
            lopdf::IncrementalDocument::load_from(std::io::Cursor::new(base)).unwrap();
        assert!(export_pdf_annotations(&mut incremental, annotations).unwrap());
        let mut out = Vec::new();
        incremental.save_to(&mut out).unwrap();
        fs::write(path, out).unwrap();
    }

    #[test]
    fn exported_textbox_keeps_its_formatting_after_an_external_save() {
        let root = std::env::temp_dir().join(format!("lapis_audit_tb_same_{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("tb.pdf");
        valid_test_pdf(&path);
        // A short box: the export grows it to fit the text, which used to look like an edit.
        let ann = sample_textbox("\u{200B}- hello __world__", vec![(0.1, 0.1), (0.4, 0.11)]);
        let annotations = HashMap::from([(1u32, vec![ann.clone()])]);
        export_to_file(&path, &annotations);

        let mut local = annotations.clone();
        import_external_annotations(&path, &mut local);
        assert_eq!(local[&1].len(), 1);
        let kept = &local[&1][0];
        assert_eq!(kept.text, ann.text);
        assert!(kept.bold && kept.italic);
        assert_eq!(kept.stroke_width, 3.0);
        assert!((kept.font_size - 20.0).abs() < 0.01);
        assert_eq!(kept.points, ann.points);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn externally_edited_textbox_text_keeps_background_and_formatting() {
        let root = std::env::temp_dir().join(format!("lapis_audit_tb_edit_{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("tb.pdf");
        valid_test_pdf(&path);
        let ann = sample_textbox("\u{200B}original", vec![(0.1, 0.1), (0.4, 0.2)]);
        let annotations = HashMap::from([(1u32, vec![ann])]);
        export_to_file(&path, &annotations);

        let mut doc = lopdf::Document::load(&path).unwrap();
        for object in doc.objects.values_mut() {
            if let lopdf::Object::Dictionary(d) = object {
                let is_free_text = d.get(b"Subtype").ok().and_then(|o| o.as_name().ok())
                    == Some(b"FreeText".as_slice());
                if is_free_text { d.set("Contents", pdf_text_object("changed")); }
            }
        }
        doc.save(&path).unwrap();

        let mut local = annotations.clone();
        import_external_annotations(&path, &mut local);
        assert_eq!(local[&1].len(), 1);
        let edited = &local[&1][0];
        assert_eq!(edited.text, "\u{200B}changed");
        assert!(edited.bold && edited.italic);
        assert_eq!(edited.stroke_width, 3.0);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn page_changes_remap_every_per_page_map() {
        let mut data = DocData::default();
        for page in 1..=3u32 {
            data.notes.insert(page, format!("note {page}"));
        }
        data.annotations.insert(2, vec![sample_textbox("x", vec![(0.1, 0.1), (0.2, 0.2)])]);
        data.blank_pages.insert(3);
        // Delete page 2.
        remap_doc_data_pages(&mut data, &|page| match page.cmp(&2) {
            std::cmp::Ordering::Less => Some(page),
            std::cmp::Ordering::Equal => None,
            std::cmp::Ordering::Greater => Some(page - 1),
        });
        assert_eq!(data.notes.len(), 2);
        assert_eq!(data.notes[&2], "note 3");
        assert!(data.annotations.is_empty());
        assert!(data.blank_pages.contains(&2) && data.blank_pages.len() == 1);
    }

    #[test]
    fn embedded_payload_is_rewritten_for_a_page_change() {
        let mut data = DocData::default();
        data.notes.insert(1, "first".to_owned());
        data.notes.insert(2, "second".to_owned());
        let mut payload = NOTES_START_TAG.to_vec();
        serde_json::to_writer(&mut payload, &data).unwrap();
        payload.extend_from_slice(NOTES_END_TAG);

        let root = isolated_dir!(embedded_payload_is_rewritten_for_a_page_change);
        let path = root.join("embedded_payload.pdf");
        let updated = rebase_embedded_notes(&path, &payload, |d| {
            shift_doc_data_on_insert(&mut d.notes, &mut d.annotations, &mut d.blank_pages, &mut d.sketches, 2);
        });
        let block = find_embedded_block(&updated).unwrap();
        let back = parse_embedded_data(&updated[block.body_start..block.body_end]).unwrap();
        assert_eq!(back.notes[&1], "first");
        assert_eq!(back.notes[&3], "second");
        assert!(!back.notes.contains_key(&2));
        assert!(back.blank_pages.contains(&2));
        // A payload that cannot be parsed is returned untouched.
        assert_eq!(rebase_embedded_notes(&path, b"garbage", |_| {}), b"garbage".to_vec());
    }

    fn save_test_pdf(path: &Path, count: u32) {
        write_save_fixture_pdf(path, count, 320, false);
    }

    fn write_save_fixture_pdf(path: &Path, count: u32, payload_repeats: usize, mixed_sizes: bool) {
        use lopdf::{Object, Stream, dictionary};
        let mut doc = lopdf::Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let mut kids = Vec::new();
        let padding = "% synthetic page payload for save performance\n".repeat(payload_repeats);
        for page in 1..=count {
            let mut content = format!("q\n0 0 m\n1 1 l\nS\nQ\n% page {page}\n").into_bytes();
            // Valid uncompressed page content; exercise a multi-megabyte PDF.
            content.extend_from_slice(padding.as_bytes());
            let content_id = doc.add_object(Stream::new(dictionary! {}, content));
            let (width, height, rotation) = match (mixed_sizes, page % 3) {
                (true, 1) => (960, 540, 0),
                (true, 2) => (612, 792, 90),
                _ => (612, 792, 0),
            };
            let page_id = doc.add_object(dictionary! {
                "Type" => "Page", "Parent" => Object::Reference(pages_id),
                "MediaBox" => vec![0.into(), 0.into(), width.into(), height.into()],
                "Resources" => dictionary! {}, "Contents" => Object::Reference(content_id),
            });
            if rotation != 0 {
                doc.get_dictionary_mut(page_id)
                    .unwrap()
                    .set("Rotate", rotation);
            }
            kids.push(Object::Reference(page_id));
        }
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages", "Kids" => kids, "Count" => Object::Integer(count as i64),
            }),
        );
        let catalog = doc.add_object(
            dictionary! { "Type" => "Catalog", "Pages" => Object::Reference(pages_id) },
        );
        doc.trailer.set("Root", catalog);
        doc.save(path).unwrap();
    }

    fn save_fixture(path: &Path, data: &DocData) {
        commit_doc_data(path, &data.notes, &data.annotations, data.blank_pages.clone(), &data.sketches).unwrap();
    }

    fn verify_save_fixture(path: &Path, expected: &DocData, pages: usize) {
        let bytes = fs::read(path).unwrap();
        let pdf = lopdf::Document::load_mem(&bytes).unwrap();
        assert_eq!(pdf.get_pages().len(), pages);
        let embedded = embedded_doc_data(&bytes).expect("missing embedded Lapis data");
        assert!(same_content(&embedded, expected), "embedded content differs");
        assert_eq!(embedded.pdf_annots_file_len, bytes.len() as u64);
        assert!(same_content(&load_local_backup(path).unwrap(), expected));
        assert!(trailing_lapis_revision(&bytes).is_some());
    }

    #[test]
    fn save_pipeline_large_document() {
        let root = isolated_dir!(save_pipeline_large_document);
        let path = root.join("large.pdf");
        save_test_pdf(&path, 180);
        let mut data = load_doc_data(&path);
        for page in (1..=180).step_by(3) {
            data.notes.insert(page, format!("# Page {page}\n{}", "Example notes. ".repeat(32)));
        }
        let mut pen = sample_textbox("", vec![(0.1, 0.1), (0.2, 0.3), (0.4, 0.2)]);
        pen.kind = AnnotationKind::Freehand;
        let mut highlight = pen.clone();
        highlight.kind = AnnotationKind::Highlight;
        highlight.points.truncate(2);
        highlight.color = [250, 204, 21];
        data.annotations.insert(1, vec![pen.clone(), highlight, sample_textbox("Test textbox", vec![(0.1, 0.1), (0.5, 0.3)])]);
        let mut sketch = SketchPage::new(BlankPageStyle::Grid);
        pen.points = vec![(12.0, 20.0), (40.0, 70.0)];
        sketch.strokes.push(pen);
        data.sketches.insert(80, sketch);
        println!("Fixture: 180 pages, {} bytes", fs::metadata(&path).unwrap().len());

        let start = std::time::Instant::now();
        save_fixture(&path, &data);
        println!("Initial modified save: {:?}", start.elapsed());
        verify_save_fixture(&path, &data, 180);
        let first = fs::read(&path).unwrap();
        let stamp = pdf_stamp(&path).unwrap();
        let backup = fs::read(get_backup_path(&path)).unwrap();
        let start = std::time::Instant::now();
        for _ in 0..3 { save_fixture(&path, &data); }
        println!("Three unchanged saves: {:?}", start.elapsed());
        assert_eq!(pdf_stamp(&path).unwrap(), stamp, "unchanged save replaced the file");
        assert_eq!(fs::read(&path).unwrap(), first);
        assert_eq!(fs::read(get_backup_path(&path)).unwrap(), backup);

        // Simulate a new process without the in-memory commit cache.
        *LAST_COMMIT.lock().unwrap() = None;
        let reopened = load_doc_data(&path);
        assert!(same_content(&reopened, &data));
        let start = std::time::Instant::now();
        save_fixture(&path, &reopened);
        println!("Commit on closing a reopened, unchanged PDF: {:?}", start.elapsed());
        assert_eq!(pdf_stamp(&path).unwrap(), stamp);

        data.notes.remove(&1);
        data.annotations.remove(&1);
        data.sketches.remove(&80);
        data.notes.insert(180, "Replacement note".into());
        let start = std::time::Instant::now();
        save_fixture(&path, &data);
        println!("Modified save after deletions: {:?}", start.elapsed());
        verify_save_fixture(&path, &data, 180);
        let updated = fs::read(&path).unwrap();
        assert_eq!(count_custom_sections(&updated), 1);
        assert_eq!(updated.windows(LAPIS_INCREMENTAL_MARKER.len()).filter(|w| *w == LAPIS_INCREMENTAL_MARKER).count(), 1);
        assert!(same_content(&load_doc_data(&path), &data));

        check_pdf_with_qpdf(&path);
    }

    #[test]
    #[ignore = "manual benchmark using synthetic PDFs"]
    fn persistence_benchmark_fixtures() {
        let root = isolated_dir!(persistence_benchmark_fixtures);
        fn timed<T>(case: &str, operation: &str, work: impl FnOnce() -> T) -> T {
            let start = std::time::Instant::now();
            let result = work();
            println!(
                "{case},{operation},{:.3}",
                start.elapsed().as_secs_f64() * 1000.0
            );
            result
        }

        println!("case,operation,milliseconds");
        for (name, pages, padding, mixed_sizes, study_step, strokes_per_page) in [
            ("small", 12, 6, false, 3, 1),
            ("medium", 180, 320, true, 3, 4),
            ("dense", 420, 1500, true, 1, 20),
        ] {
            let pdf = root.join(format!("{name}.pdf"));
            write_save_fixture_pdf(&pdf, pages, padding, mixed_sizes);
            println!(
                "{name}: {pages} pages, {} PDF bytes",
                fs::metadata(&pdf).unwrap().len()
            );
            let mut data = timed(name, "open PDF and load study data", || {
                assert_eq!(crate::inspect_pdf(&pdf).unwrap().0, pages);
                load_doc_data(&pdf)
            });
            timed(name, "first page CPU raster", || {
                let doc = mupdf::Document::open(pdf.to_str().unwrap()).unwrap();
                assert!(crate::render_to_image(&doc, 0, 1.0, false).is_some());
            });

            let points = (0..40)
                .map(|i| (0.05 + i as f32 * 0.02, 0.4 + (i % 5) as f32 * 0.02))
                .collect();
            let mut pen = sample_textbox("", points);
            pen.kind = AnnotationKind::Freehand;
            for page in (1..=pages).step_by(study_step) {
                data.notes.insert(
                    page,
                    format!("# Slide {page}\n\n{}", "Example study notes.\n".repeat(48)),
                );
                let strokes = (0..strokes_per_page)
                    .map(|line| {
                        let mut stroke = pen.clone();
                        for (_, y) in &mut stroke.points {
                            *y += line as f32 * 0.02;
                        }
                        stroke
                    })
                    .collect();
                data.annotations.insert(page, strokes);
                if page % 7 == 1 {
                    let mut sketch = SketchPage::new(BlankPageStyle::Grid);
                    let mut stroke = pen.clone();
                    for (x, y) in &mut stroke.points {
                        *x *= 400.0;
                        *y *= 300.0;
                    }
                    sketch.strokes.push(stroke);
                    data.sketches.insert(page, sketch);
                }
            }

            timed(name, "changed local checkpoint", || save_model(&pdf, &data));
            timed(name, "unchanged local checkpoint", || {
                save_model(&pdf, &data)
            });
            println!(
                "{name}: {} local backup bytes",
                fs::metadata(get_backup_path(&pdf)).unwrap().len()
            );
            timed(name, "changed PDF commit", || save_fixture(&pdf, &data));
            let stamp = pdf_stamp(&pdf).unwrap();
            timed(name, "unchanged PDF commit", || save_fixture(&pdf, &data));
            assert_eq!(pdf_stamp(&pdf).unwrap(), stamp);
            let reopened = timed(name, "reopen with study data", || {
                assert_eq!(crate::inspect_pdf(&pdf).unwrap().0, pages);
                load_doc_data(&pdf)
            });
            assert!(same_content(&reopened, &data));
            verify_save_fixture(&pdf, &data, pages as usize);
            check_pdf_with_qpdf(&pdf);
        }
    }

    #[test]
    fn save_pipeline_newer_edits_survive_worker_and_reopen() {
        let root = isolated_dir!(save_pipeline_newer_edits_survive_worker_and_reopen);
        let path = root.join("worker.pdf");
        save_test_pdf(&path, 3);
        let mut old = load_doc_data(&path);
        old.notes.insert(1, "Before background save".into());
        old.annotations.insert(1, vec![sample_textbox("Delete me", vec![(0.1, 0.1), (0.5, 0.3)])]);
        let snapshot = prepare_pdf_commit(&path, &old.notes, &old.annotations, old.blank_pages.clone(), &old.sketches).unwrap();
        let snapshot_rev = snapshot.rev;
        let original_bytes = fs::read(&path).unwrap();
        let mut latest = old.clone();
        latest.notes.insert(1, "Typed while saving".into());
        latest.annotations.clear();
        let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
        let worker_gate = gate.clone();
        let worker_path = path.clone();
        let worker = std::thread::spawn(move || {
            worker_gate.wait();
            commit_pdf_snapshot(&worker_path, snapshot)
        });
        // Keep the worker waiting until a newer checkpoint exists.
        let saved = save_doc_data(&path, &latest.notes, &latest.annotations, latest.blank_pages.clone(), &latest.sketches);
        gate.wait();
        worker.join().unwrap().unwrap();
        saved.unwrap();
        let embedded = read_trailing_doc_data(&path).unwrap();
        assert!(same_content(&embedded, &old));
        let backup = load_local_backup(&path).unwrap();
        assert!(backup.rev > snapshot_rev);
        assert!(same_content(&backup, &latest));
        assert_eq!(backup.pdf_annots_file_len, fs::metadata(&path).unwrap().len());

        // Simulate interruption after PDF replacement but before backup metadata reconciliation.
        let mut interrupted = backup;
        interrupted.pdf_annots_file_len = original_bytes.len() as u64;
        save_local_backup_checked(&path, &interrupted).unwrap();
        *LAST_COMMIT.lock().unwrap() = None;
        let recovered = load_doc_data(&path);
        assert!(same_content(&recovered, &latest));
        assert!(recovered.annotations.is_empty(), "deleted annotation returned");
        // Closing or another explicit save must commit the newer content, not the old snapshot.
        save_fixture(&path, &recovered);
        verify_save_fixture(&path, &latest, 3);
    }

    #[test]
    fn save_pipeline_failed_replacement_preserves_backup_and_pdf() {
        let root = isolated_dir!(save_pipeline_failed_replacement_preserves_backup_and_pdf);
        let path = root.join("failure.pdf");
        save_test_pdf(&path, 2);
        let mut data = load_doc_data(&path);
        save_fixture(&path, &data);
        let original = fs::read(&path).unwrap();
        data.notes.insert(1, "Recover after failed commit".into());
        let snapshot = prepare_pdf_commit(&path, &data.notes, &data.annotations, data.blank_pages.clone(), &data.sketches).unwrap();
        // A directory at the temporary output path forces a write error even as root.
        let blocked = path.with_extension("lapis_commit_tmp.pdf");
        fs::create_dir(&blocked).unwrap();
        let worker_path = path.clone();
        let result = std::thread::spawn(move || commit_pdf_snapshot(&worker_path, snapshot)).join().unwrap();
        assert!(result.is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        assert!(same_content(&load_local_backup(&path).unwrap(), &data));
        assert!(same_content(&load_doc_data(&path), &data));
        fs::remove_dir(&blocked).unwrap();
        save_fixture(&path, &data);
        verify_save_fixture(&path, &data, 2);
    }

    #[test]
    fn save_pipeline_external_revision_is_retained() {
        use lopdf::{dictionary, Object};
        let root = isolated_dir!(save_pipeline_external_revision_is_retained);
        let path = root.join("external.pdf");
        save_test_pdf(&path, 3);
        let mut data = load_doc_data(&path);
        data.notes.insert(1, "Original note".into());
        save_fixture(&path, &data);
        let saved = fs::read(&path).unwrap();
        let stamp = pdf_stamp(&path).unwrap();
        let mut incremental = lopdf::IncrementalDocument::load_from(std::io::Cursor::new(saved)).unwrap();
        let page_id = incremental.get_prev_documents().get_pages()[&2];
        incremental.opt_clone_object_to_new_document(page_id).unwrap();
        let annotation_id = incremental.new_document.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "FreeText",
            "Rect" => vec![10.into(), 20.into(), 100.into(), 60.into()],
            "Contents" => Object::string_literal(b"External edit".to_vec()),
            "DA" => Object::string_literal(b"/Helv 12 Tf 0 0 0 rg".to_vec()), "F" => 4,
        });
        incremental.new_document.get_object_mut(page_id).unwrap().as_dict_mut().unwrap()
            .set("Annots", vec![Object::Reference(annotation_id)]);
        let mut output = Vec::new();
        incremental.save_to(&mut output).unwrap();
        fs::write(&path, &output).unwrap();
        assert_ne!(pdf_stamp(&path).unwrap(), stamp);
        assert!(!committed_matches(&path, &pdf_stamp(&path).unwrap(), &data));
        let reopened = load_doc_data(&path);
        assert_eq!(reopened.notes[&1], "Original note");
        assert!(reopened.annotations[&2].iter().any(|a| a.text == "External edit"));
        save_fixture(&path, &reopened);
        verify_save_fixture(&path, &reopened, 3);
        assert!(fs::read(&path).unwrap().starts_with(&output), "external revision was truncated");
    }

    #[test]
    fn save_pipeline_page_structure_roundtrip() {
        let root = isolated_dir!(save_pipeline_page_structure_roundtrip);
        let path = root.join("structure.pdf");
        save_test_pdf(&path, 4);
        let mut data = load_doc_data(&path);
        data.notes.insert(2, "Move with page".into());
        data.sketches.insert(2, SketchPage::new(BlankPageStyle::Dotted));
        data.annotations.insert(2, vec![sample_textbox("Move too", vec![(0.1, 0.1), (0.5, 0.3)])]);
        save_fixture(&path, &data);
        let inserted = insert_blank_page_with_style_into_pdf(&path, 1, BlankPageStyle::Blank, false).unwrap();
        assert_eq!(inserted, 2);
        check_pdf_with_qpdf(&path);
        shift_doc_data_on_insert(&mut data.notes, &mut data.annotations, &mut data.blank_pages, &mut data.sketches, inserted);
        save_fixture(&path, &data);
        verify_save_fixture(&path, &data, 5);
        assert_eq!(data.notes[&3], "Move with page");
        reorder_pages_in_pdf(&path, 3, 1, false).unwrap();
        check_pdf_with_qpdf(&path);
        remap_doc_data_pages(&mut data, &|p| Some(match p { 3 => 1, 1 | 2 => p + 1, _ => p }));
        save_fixture(&path, &data);
        verify_save_fixture(&path, &data, 5);
        delete_page_from_pdf(&path, 1, false).unwrap();
        check_pdf_with_qpdf(&path);
        remap_doc_data_pages(&mut data, &|p| p.checked_sub(1).filter(|p| *p > 0));
        save_fixture(&path, &data);
        verify_save_fixture(&path, &data, 4);
        assert!(data.notes.is_empty() && data.annotations.is_empty() && data.sketches.is_empty());
        assert!(same_content(&load_doc_data(&path), &data));
    }

    fn append_external_freetext(path: &Path, page: u32, text: &str) {
        use lopdf::{Object, dictionary};
        append_external_annotation(
            path,
            page,
            dictionary! {
                "Type" => "Annot", "Subtype" => "FreeText",
                "Rect" => vec![10.into(), 20.into(), 100.into(), 60.into()],
                "Contents" => Object::string_literal(text.as_bytes().to_vec()),
                "DA" => Object::string_literal(b"/Helv 12 Tf 0 0 0 rg".to_vec()), "F" => 4,
            },
        );
    }

    fn append_external_annotation(path: &Path, page: u32, annotation: lopdf::Dictionary) {
        let saved = fs::read(path).unwrap();
        let mut incremental =
            lopdf::IncrementalDocument::load_from(std::io::Cursor::new(saved)).unwrap();
        let page_id = incremental.get_prev_documents().get_pages()[&page];
        incremental
            .opt_clone_object_to_new_document(page_id)
            .unwrap();
        let annotation_id = incremental.new_document.add_object(annotation);
        let page_dict = incremental
            .new_document
            .get_dictionary_mut(page_id)
            .unwrap();
        let mut annotations = page_dict
            .get(b"Annots")
            .map(|items| items.as_array().unwrap().clone())
            .unwrap_or_default();
        annotations.push(lopdf::Object::Reference(annotation_id));
        page_dict.set("Annots", annotations);
        let mut output = Vec::new();
        incremental.save_to(&mut output).unwrap();
        fs::write(path, &output).unwrap();
    }

    fn save_model(path: &Path, data: &DocData) {
        save_doc_data(path, &data.notes, &data.annotations, data.blank_pages.clone(), &data.sketches).unwrap();
    }

    #[test]
    fn importing_external_annotations_can_read_an_equal_length_edit() {
        let root = isolated_dir!(importing_external_annotations_can_read_an_equal_length_edit);
        let path = root.join("same-length.pdf");
        save_test_pdf(&path, 2);
        append_external_freetext(&path, 2, "original external text");
        let mut annotations = HashMap::new();
        merge_external_annotations(&path, &mut annotations);
        assert_eq!(annotations[&2][0].text, "original external text");

        let mut bytes = fs::read(&path).unwrap();
        let before = b"original external text";
        let after = b"modified external text";
        let offset = memchr::memmem::find(&bytes, before).expect("annotation text in fixture");
        bytes[offset..offset + before.len()].copy_from_slice(after);
        fs::write(&path, &bytes).unwrap();
        let mut annotations = HashMap::new();
        merge_external_annotations(&path, &mut annotations);
        assert_eq!(annotations[&2][0].text, "modified external text");
        check_pdf_with_qpdf(&path);
    }

    #[test]
    fn commit_keeps_unconverted_native_annotation_on_an_edited_page() {
        use lopdf::{Object, dictionary};

        let root = isolated_dir!(commit_keeps_unconverted_native_annotation_on_an_edited_page);
        let path = root.join("native.pdf");
        save_test_pdf(&path, 2);
        let square = dictionary! {
            "Type" => "Annot", "Subtype" => "Square",
            "Rect" => vec![10.into(), 20.into(), 100.into(), 60.into()],
            "NM" => Object::string_literal("external-square"),
            "T" => Object::string_literal("External author"),
            "Contents" => Object::string_literal("External annotation"),
            "F" => 4,
        };
        append_external_annotation(&path, 2, square.clone());
        let mut data = load_doc_data(&path);
        data.annotations.insert(
            2,
            vec![sample_textbox(
                "New Lapis annotation",
                vec![(0.1, 0.1), (0.5, 0.3)],
            )],
        );
        save_fixture(&path, &data);

        let output = lopdf::Document::load(&path).unwrap();
        let page = output.get_dictionary(output.get_pages()[&2]).unwrap();
        let annotations = page.get(b"Annots").unwrap().as_array().unwrap();
        assert_eq!(annotations.len(), 2);
        assert!(annotations.iter().any(|item| {
            output.get_dictionary(item.as_reference().unwrap()).unwrap() == &square
        }));
        check_pdf_with_qpdf(&path);
    }

    #[test]
    fn save_pipeline_external_annotation_is_merged_never_dropped() {
        let root = isolated_dir!(save_pipeline_external_annotation_is_merged_never_dropped);
        let path = root.join("external_merge.pdf");
        save_test_pdf(&path, 3);
        let mut data = load_doc_data(&path);
        data.notes.insert(1, "Original note".into());
        save_fixture(&path, &data);
        append_external_freetext(&path, 2, "External edit");
        let external = fs::read(&path).unwrap();

        // The live model has not seen the external revision: refuse and write nothing.
        data.notes.insert(1, "Edited note".into());
        let err = commit_doc_data(&path, &data.notes, &data.annotations, data.blank_pages.clone(), &data.sketches)
            .unwrap_err();
        assert!(is_external_modification(&err), "unexpected error: {err}");
        assert_eq!(fs::read(&path).unwrap(), external, "refused commit modified the PDF");
        assert!(!path.with_extension("lapis_commit_tmp.pdf").exists());
        assert_eq!(load_local_backup(&path).unwrap().notes[&1], "Edited note", "local edit lost");

        // Merge, checkpoint, acknowledge, retry.
        merge_external_annotations(&path, &mut data.annotations);
        save_model(&path, &data);
        mark_pdf_observed(&path).unwrap();
        save_fixture(&path, &data);
        verify_save_fixture(&path, &data, 3);
        assert!(data.annotations[&2].iter().any(|a| a.text == "External edit"));
        assert!(fs::read(&path).unwrap().starts_with(&external), "external revision was truncated");

        // A second commit and a reopen keep it.
        data.notes.insert(1, "Edited again".into());
        save_fixture(&path, &data);
        let reopened = load_doc_data(&path);
        assert_eq!(reopened.notes[&1], "Edited again");
        assert!(reopened.annotations[&2].iter().any(|a| a.text == "External edit"));
    }

    #[test]
    fn save_pipeline_own_tail_after_interrupted_checkpoint_is_not_external() {
        let root = isolated_dir!(save_pipeline_own_tail_after_interrupted_checkpoint_is_not_external);
        let path = root.join("own_tail.pdf");
        save_test_pdf(&path, 2);
        let mut data = load_doc_data(&path);
        data.notes.insert(1, "One".into());
        save_fixture(&path, &data);
        // Simulate a stop between the rename and the backup update: stale watermark.
        let mut backup = load_local_backup(&path).unwrap();
        backup.pdf_annots_file_len = 1;
        save_local_backup_checked(&path, &backup).unwrap();
        data.notes.insert(1, "Two".into());
        save_fixture(&path, &data);
        verify_save_fixture(&path, &data, 2);
    }

    #[test]
    fn save_pipeline_compaction_refuses_unmerged_external_revision() {
        let root = isolated_dir!(save_pipeline_compaction_refuses_unmerged_external_revision);
        let path = root.join("compact_external.pdf");
        save_test_pdf(&path, 3);
        let data = load_doc_data(&path);
        save_fixture(&path, &data);
        append_external_freetext(&path, 2, "External edit");
        let external = fs::read(&path).unwrap();
        let err = compact_doc_data(&path, &data.notes, &data.annotations, data.blank_pages.clone(), &data.sketches)
            .unwrap_err();
        assert!(is_external_modification(&err), "unexpected error: {err}");
        assert_eq!(fs::read(&path).unwrap(), external);
        assert!(!path.with_extension("lapis_before_compaction.pdf").exists());
    }

    #[test]
    fn save_pipeline_stale_commit_tmp_does_not_block_a_commit() {
        let root = isolated_dir!(save_pipeline_stale_commit_tmp_does_not_block_a_commit);
        let path = root.join("stale_tmp.pdf");
        save_test_pdf(&path, 2);
        let mut data = load_doc_data(&path);
        fs::write(path.with_extension("lapis_commit_tmp.pdf"), b"killed mid-write").unwrap();
        data.notes.insert(1, "After crash".into());
        save_fixture(&path, &data);
        verify_save_fixture(&path, &data, 2);
        assert!(!path.with_extension("lapis_commit_tmp.pdf").exists());
    }

    #[test]
    fn page_structure_rewrite_survives_a_lost_backup_update() {
        let root = isolated_dir!(page_structure_rewrite_survives_a_lost_backup_update);
        let path = root.join("structure_crash.pdf");
        save_test_pdf(&path, 4);
        let mut data = load_doc_data(&path);
        data.notes.insert(2, "Move with page".into());
        save_fixture(&path, &data);
        // An edit that exists only in the local backup (never committed to the PDF).
        data.notes.insert(1, "Latest local edit".into());
        save_model(&path, &data);
        let before = fs::read(get_backup_path(&path)).unwrap();
        let rev_before = load_local_backup(&path).unwrap().rev;

        let inserted = insert_blank_page_with_style_into_pdf(&path, 1, BlankPageStyle::Blank, false).unwrap();
        assert_eq!(inserted, 2);
        let after = load_local_backup(&path).unwrap();
        assert!(after.rev > rev_before, "backup revision must advance");
        assert_eq!(after.notes[&3], "Move with page");
        assert_eq!(after.notes[&1], "Latest local edit");
        assert_eq!(after.pdf_annots_file_len, fs::metadata(&path).unwrap().len());

        // Stop before the backup was rewritten: the PDF's payload must win and be complete.
        fs::write(get_backup_path(&path), &before).unwrap();
        *LAST_COMMIT.lock().unwrap() = None;
        let reopened = load_doc_data(&path);
        assert_eq!(reopened.notes[&1], "Latest local edit");
        assert_eq!(reopened.notes[&3], "Move with page");
        assert!(!reopened.notes.contains_key(&2));
        assert!(reopened.blank_pages.contains(&2));
    }

    fn add_test_object(path: &Path, dict: lopdf::Dictionary) {
        let mut doc = lopdf::Document::load(path).unwrap();
        doc.add_object(dict);
        doc.save(path).unwrap();
    }

    #[test]
    fn page_structure_ops_ask_before_invalidating_a_signature() {
        use lopdf::dictionary;
        let root = isolated_dir!(page_structure_ops_ask_before_invalidating_a_signature);
        let path = root.join("structure_signed.pdf");
        save_test_pdf(&path, 3);
        add_test_object(&path, dictionary! {
            "Type" => "Sig", "Filter" => "Adobe.PPKLite",
            "ByteRange" => vec![0.into(), 1.into(), 2.into(), 3.into()],
        });
        let original = fs::read(&path).unwrap();
        let insert = insert_blank_page_with_style_into_pdf(&path, 1, BlankPageStyle::Blank, false).unwrap_err();
        let delete = delete_page_from_pdf(&path, 1, false).unwrap_err();
        let reorder = reorder_pages_in_pdf(&path, 1, 2, false).unwrap_err();
        assert!(is_signed_pdf_error(&insert) && is_signed_pdf_error(&delete) && is_signed_pdf_error(&reorder));
        assert_eq!(fs::read(&path).unwrap(), original, "a refused edit modified the PDF");
        assert!(!path.with_extension("ins_tmp.pdf").exists());

        // Once the user has confirmed, the edit proceeds and the file stays valid.
        let inserted = insert_blank_page_with_style_into_pdf(&path, 1, BlankPageStyle::Blank, true).unwrap();
        assert_eq!(inserted, 2);
        assert_eq!(lopdf::Document::load(&path).unwrap().get_pages().len(), 4);
        delete_page_from_pdf(&path, 1, true).unwrap();
        reorder_pages_in_pdf(&path, 1, 2, true).unwrap();
        assert_eq!(lopdf::Document::load(&path).unwrap().get_pages().len(), 3);
    }

    #[test]
    fn an_empty_signature_field_does_not_block_page_edits() {
        use lopdf::dictionary;
        let root = isolated_dir!(an_empty_signature_field_does_not_block_page_edits);
        let path = root.join("structure_sigfield.pdf");
        save_test_pdf(&path, 3);
        add_test_object(&path, dictionary! { "FT" => "Sig", "T" => lopdf::Object::string_literal("Signature1") });
        insert_blank_page_with_style_into_pdf(&path, 1, BlankPageStyle::Blank, false).unwrap();
        delete_page_from_pdf(&path, 1, false).unwrap();
        reorder_pages_in_pdf(&path, 1, 2, false).unwrap();
    }

    #[test]
    fn backup_cache_never_hides_an_outside_change() {
        let root = isolated_dir!(backup_cache_never_hides_an_outside_change);
        let path = root.join("backup_cache.pdf");
        save_test_pdf(&path, 2);
        let mut data = load_doc_data(&path);
        data.notes.insert(1, "alpha".into());
        save_model(&path, &data);
        assert_eq!(load_local_backup(&path).unwrap().notes[&1], "alpha");
        // Another writer replaces the file in place with a same-length value.
        let backup = get_backup_path(&path);
        let json = fs::read_to_string(&backup).unwrap();
        assert!(json.contains("alpha"));
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(&backup, json.replace("alpha", "omega")).unwrap();
        assert_eq!(load_local_backup(&path).unwrap().notes[&1], "omega", "stale cached backup");
        fs::remove_file(&backup).unwrap();
        assert!(load_local_backup(&path).is_none());
    }

    #[test]
    fn structural_rewrite_embeds_the_final_length_after_one_serialization() {
        let root = isolated_dir!(structural_rewrite_embeds_the_final_length_after_one_serialization);
        let path = root.join("structure_len.pdf");
        save_test_pdf(&path, 3);
        let mut data = load_doc_data(&path);
        data.notes.insert(2, "keep".into());
        save_fixture(&path, &data);
        delete_page_from_pdf(&path, 1, false).unwrap();
        let bytes = fs::read(&path).unwrap();
        let embedded = embedded_doc_data(&bytes).unwrap();
        assert_eq!(embedded.pdf_annots_file_len, bytes.len() as u64);
        assert_eq!(embedded.notes[&1], "keep");
        assert_eq!(lopdf::Document::load_mem(&bytes).unwrap().get_pages().len(), 2);
        assert_eq!(count_custom_sections(&bytes), 1);
    }

    /// The scan used before `memchr`, kept as the oracle for the differential test.
    fn naive_find_embedded_block(bytes: &[u8]) -> Option<EmbeddedBlock> {
        let candidates = [
            (NOTES_START_TAG, NOTES_END_TAG),
            (LEGACY_NOTES_START_TAG, LEGACY_NOTES_END_TAG),
        ];
        let mut found: Option<EmbeddedBlock> = None;
        for (start_tag, end_tag) in candidates {
            let mut from = 0;
            while from < bytes.len() {
                let Some(rel) = bytes[from..].windows(start_tag.len()).position(|w| w == start_tag) else { break };
                let start = from + rel;
                let body_start = start + start_tag.len();
                let Some(end_rel) = bytes[body_start..].windows(end_tag.len()).position(|w| w == end_tag) else {
                    from = start + start_tag.len();
                    continue;
                };
                let body_end = body_start + end_rel;
                let end = body_end + end_tag.len();
                let candidate = EmbeddedBlock { start, body_start, body_end, end };
                if found.is_none_or(|old| candidate.start > old.start) { found = Some(candidate); }
                from = end;
            }
        }
        found
    }

    #[test]
    fn fast_scans_match_the_naive_scans_on_adversarial_input() {
        // Deterministic pseudo-random streams built from tag fragments, whole tags and noise.
        let pieces: Vec<&[u8]> = vec![
            NOTES_START_TAG, NOTES_END_TAG, LEGACY_NOTES_START_TAG, LEGACY_NOTES_END_TAG,
            LAPIS_INCREMENTAL_MARKER, b"startxref\n", b"%%EOF\n", PDF_LEN_FIELD,
            &NOTES_START_TAG[..NOTES_START_TAG.len() / 2], &NOTES_END_TAG[1..],
            &LEGACY_NOTES_START_TAG[2..], b"\n", b"{}", b"xxxx",
        ];
        let mut seed = 0x2545F4914F6CDD1Du64;
        let mut next = move || { seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17; seed };
        for _ in 0..4000 {
            let mut bytes = Vec::new();
            for _ in 0..(next() % 14) { bytes.extend_from_slice(pieces[(next() % pieces.len() as u64) as usize]); }
            let (fast, slow) = (find_embedded_block(&bytes), naive_find_embedded_block(&bytes));
            assert_eq!(
                fast.map(|b| (b.start, b.body_start, b.body_end, b.end)),
                slow.map(|b| (b.start, b.body_start, b.body_end, b.end)),
                "diverged on {:?}", String::from_utf8_lossy(&bytes)
            );
            let naive_rfind = |needle: &[u8]| bytes.windows(needle.len()).rposition(|w| w == needle);
            assert_eq!(memchr::memmem::rfind(&bytes, b"startxref"), naive_rfind(b"startxref"));
            assert_eq!(memchr::memmem::rfind(&bytes, LAPIS_INCREMENTAL_MARKER), naive_rfind(LAPIS_INCREMENTAL_MARKER));
        }
    }

    #[test]
    fn save_pipeline_edit_between_pdf_replace_and_reconcile_is_never_lost() {
        let root = isolated_dir!(save_pipeline_edit_between_pdf_replace_and_reconcile_is_never_lost);
        let path = root.join("mid_commit_edit.pdf");
        save_test_pdf(&path, 3);
        let mut data = load_doc_data(&path);
        data.notes.insert(1, "snapshot note".into());
        let snapshot = prepare_pdf_commit(&path, &data.notes, &data.annotations, data.blank_pages.clone(), &data.sketches).unwrap();
        let snapshot_rev = snapshot.rev;

        // The UI checkpoints a newer edit at the exact moment the worker has replaced the PDF.
        let mut newer = data.notes.clone();
        newer.insert(1, "edit during commit".into());
        newer.insert(2, "second page edit".into());
        let (hook_path, annotations, sketches) = (path.clone(), data.annotations.clone(), data.sketches.clone());
        let hook_notes = newer.clone();
        *AFTER_REPLACE_HOOK.lock().unwrap() = Some(Box::new(move || {
            save_doc_data(&hook_path, &hook_notes, &annotations, HashSet::new(), &sketches).unwrap();
        }));
        commit_pdf_snapshot(&path, snapshot).unwrap();
        assert!(AFTER_REPLACE_HOOK.lock().unwrap().is_none(), "hook did not run");

        // The PDF holds the snapshot; the backup keeps the newer edit and the commit metadata.
        let embedded = embedded_doc_data(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(embedded.notes[&1], "snapshot note");
        let backup = load_local_backup(&path).unwrap();
        assert_eq!(backup.notes[&1], "edit during commit", "newer edit overwritten by the worker");
        assert_eq!(backup.notes[&2], "second page edit");
        assert!(backup.rev > snapshot_rev, "backup revision went backwards");
        assert_eq!(backup.pdf_annots_file_len, fs::metadata(&path).unwrap().len());
        assert!(backup.pdf_annots_imported);

        // Reopening prefers the newer local data, and the follow-up commit persists it.
        *LAST_COMMIT.lock().unwrap() = None;
        let reopened = load_doc_data(&path);
        assert_eq!(reopened.notes[&1], "edit during commit");
        commit_doc_data(&path, &newer, &data.annotations, HashSet::new(), &data.sketches).unwrap();
        let embedded = embedded_doc_data(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(embedded.notes[&1], "edit during commit");
        assert_eq!(embedded.notes[&2], "second page edit");
        assert_eq!(count_custom_sections(&fs::read(&path).unwrap()), 1);
    }

    #[test]
    fn compaction_is_blocked_only_by_real_signature_values() {
        use lopdf::dictionary;
        let root = isolated_dir!(compaction_is_blocked_only_by_real_signature_values);
        let path = root.join("compact_signature.pdf");
        save_test_pdf(&path, 3);
        let data = load_doc_data(&path);
        save_fixture(&path, &data);
        // An empty signature field does not block compaction.
        add_test_object(&path, dictionary! { "FT" => "Sig", "T" => lopdf::Object::string_literal("Signature1") });
        mark_pdf_observed(&path).unwrap();
        compact_doc_data(&path, &data.notes, &data.annotations, data.blank_pages.clone(), &data.sketches).unwrap();
        // A signed value does, and the file is left alone.
        add_test_object(&path, dictionary! {
            "Type" => "Sig", "ByteRange" => vec![0.into(), 1.into(), 2.into(), 3.into()],
        });
        mark_pdf_observed(&path).unwrap();
        let before = fs::read(&path).unwrap();
        let err = compact_doc_data(&path, &data.notes, &data.annotations, data.blank_pages.clone(), &data.sketches).unwrap_err();
        assert!(err.to_string().contains("signed"), "unexpected error: {err}");
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn repeated_commits_keep_one_custom_block_and_one_rollback_marker() {
        let root = isolated_dir!(repeated_commits_keep_one_custom_block_and_one_rollback_marker);
        let path = root.join("example.pdf");
        valid_test_pdf(&path);
        let mut notes = HashMap::new();
        notes.insert(1, "first".to_owned());
        let _ = load_doc_data(&path);
        commit_doc_data(&path, &notes, &HashMap::new(), HashSet::new(), &HashMap::new()).unwrap();
        let first = fs::read(&path).unwrap();
        assert_eq!(count_custom_sections(&first), 1);
        assert_eq!(first.windows(LAPIS_INCREMENTAL_MARKER.len())
            .filter(|w| *w == LAPIS_INCREMENTAL_MARKER).count(), 1);
        assert_eq!(read_trailing_doc_data(&path).unwrap().pdf_annots_file_len, first.len() as u64);
        let backup_before = fs::read(get_backup_path(&path)).unwrap();
        // An explicit save immediately followed by an OS close is a no-op.
        save_doc_data(&path, &notes, &HashMap::new(), HashSet::new(), &HashMap::new()).unwrap();
        assert_eq!(fs::read(get_backup_path(&path)).unwrap(), backup_before);
        commit_doc_data(&path, &notes, &HashMap::new(), HashSet::new(), &HashMap::new()).unwrap();
        assert_eq!(fs::read(&path).unwrap(), first);
        assert_eq!(fs::read(get_backup_path(&path)).unwrap(), backup_before);
        // A changed on-disk file must invalidate the fast path, even with the same notes.
        let stamp = pdf_stamp(&path).unwrap();
        let data = read_trailing_doc_data(&path).unwrap();
        assert!(committed_matches(&path, &stamp, &data));
        let mut changed = first.clone();
        changed.push(b'\n');
        fs::write(&path, &changed).unwrap();
        assert!(!committed_matches(&path, &pdf_stamp(&path).unwrap(), &data));
        fs::write(&path, &first).unwrap();
        notes.insert(1, "updated".to_owned());
        commit_doc_data(&path, &notes, &HashMap::new(), HashSet::new(), &HashMap::new()).unwrap();
        let updated = fs::read(&path).unwrap();
        assert_eq!(count_custom_sections(&updated), 1);
        assert_eq!(updated.windows(LAPIS_INCREMENTAL_MARKER.len())
            .filter(|w| *w == LAPIS_INCREMENTAL_MARKER).count(), 1);
        assert_eq!(read_trailing_doc_data(&path).unwrap().notes[&1], "updated");
        assert!(lopdf::Document::load_mem(&updated).is_ok());
    }

    #[test]
    fn reserved_pdf_length_preserves_stream_size_and_numeric_format() {
        let data = DocData::default();
        let mut payload = pending_length_stream(&data).unwrap().content;
        let before = payload.len();
        assert_eq!(finalize_pdf_length(&mut payload, 0).unwrap(), before as u64);
        assert_eq!(payload.len(), before);
        let block = find_embedded_block(&payload).unwrap();
        assert_eq!(parse_embedded_data(&payload[block.body_start..block.body_end])
            .unwrap().pdf_annots_file_len, before as u64);
    }

    #[test]
    fn compaction_after_lapis_last_revision_keeps_one_metadata_stream() {
        let root = isolated_dir!(compaction_after_lapis_last_revision_keeps_one_metadata_stream);
        let path = root.join("example.pdf");
        valid_test_pdf(&path);
        let mut notes = HashMap::new();
        notes.insert(1, "saved by Lapis".to_owned());
        let _ = load_doc_data(&path);
        commit_doc_data(&path, &notes, &HashMap::new(), HashSet::new(), &HashMap::new()).unwrap();
        assert!(trailing_lapis_revision(&fs::read(&path).unwrap()).is_some());
        notes.insert(1, "latest locally".to_owned());
        save_doc_data(&path, &notes, &HashMap::new(), HashSet::new(), &HashMap::new()).unwrap();
        compact_doc_data(&path, &notes, &HashMap::new(), HashSet::new(), &HashMap::new()).unwrap();
        let output = fs::read(&path).unwrap();
        assert_eq!(count_custom_sections(&output), 1);
        assert_eq!(output.windows(LAPIS_INCREMENTAL_MARKER.len())
            .filter(|w| *w == LAPIS_INCREMENTAL_MARKER).count(), 1);
        assert_eq!(read_trailing_doc_data(&path).unwrap().notes[&1], "latest locally");
        assert_eq!(lopdf::Document::load_mem(&output).unwrap().get_pages().len(), 1);
        assert!(path.with_extension("lapis_before_compaction.pdf").exists());
    }

    #[test]
    fn local_checkpoints_survive_without_a_pdf_commit() {
        let root = isolated_dir!(local_checkpoints_survive_without_a_pdf_commit);
        let path = root.join("example.pdf");
        valid_test_pdf(&path);
        let original = fs::read(&path).unwrap();
        let _ = load_doc_data(&path);
        let mut notes = HashMap::new();
        notes.insert(1, "first edit".into());
        save_doc_data(&path, &notes, &HashMap::new(), HashSet::new(), &HashMap::new()).unwrap();
        notes.insert(1, "second edit".into());
        save_doc_data(&path, &notes, &HashMap::new(), HashSet::new(), &HashMap::new()).unwrap();
        assert_eq!(fs::read(&path).unwrap(), original, "autosave must leave PDF untouched");
        assert_eq!(load_local_backup(&path).unwrap().notes[&1], "second edit");
        assert_eq!(load_doc_data(&path).notes[&1], "second edit");
    }

    #[test]
    fn compaction_removes_historical_lapis_sections_and_preserves_latest_notes() {
        let root = isolated_dir!(compaction_removes_historical_lapis_sections_and_preserves_latest_notes);
        let path = root.join("example.pdf");
        valid_test_pdf(&path);
        let mut notes = HashMap::new();
        notes.insert(1, "before".to_owned());
        let _ = load_doc_data(&path);
        commit_doc_data(&path, &notes, &HashMap::new(), HashSet::new(), &HashMap::new()).unwrap();
        // Simulate a PDF with a historical Lapis object retained by an external revision.
        // Using a full PDF rewrite here keeps the extra stream structurally valid.
        let mut doc = lopdf::Document::load(&path).unwrap();
        let old = read_trailing_doc_data(&path).unwrap();
        doc.add_object(lopdf::Object::Stream(custom_data_stream(&old).unwrap()));
        doc.save(&path).unwrap();
        assert_eq!(count_custom_sections(&fs::read(&path).unwrap()), 2);
        // The rewrite changed the length; the app acknowledges it after merging.
        mark_pdf_observed(&path).unwrap();
        notes.insert(1, "after".to_owned());
        compact_doc_data(&path, &notes, &HashMap::new(), HashSet::new(), &HashMap::new()).unwrap();
        let output = fs::read(&path).unwrap();
        assert_eq!(count_custom_sections(&output), 1);
        assert_eq!(lopdf::Document::load_mem(&output).unwrap().get_pages().len(), 1);
        assert_eq!(read_trailing_doc_data(&path).unwrap().notes[&1], "after");
        assert!(path.with_extension("lapis_before_compaction.pdf").exists());
    }

    #[test]
    fn legacy_payload_migrates_all_original_document_fields() {
        let mut old = DocData::default();
        old.notes.insert(2, "lecture".to_string());
        old.annotations.insert(2, vec![PageAnnotation {
            kind: AnnotationKind::Freehand,
            color: [11, 22, 33],
            points: vec![(0.1, 0.2), (0.3, 0.4)],
            text: String::new(), font_size: 18.0, bold: false, italic: false, stroke_width: 2.5,
        }]);
        old.blank_pages.insert(3);
        old.sketches.insert(4, SketchPage::new(BlankPageStyle::Grid));
        old.doc_id = Some("legacy-test-id".to_string());
        old.rev = 123;
        let mut bytes = b"%PDF-1.4\n%%EOF\n".to_vec();
        bytes.extend_from_slice(LEGACY_NOTES_START_TAG);
        // The original JSON predates PDF-native import tracking fields.
        let mut legacy_json = serde_json::to_value(&old).unwrap();
        let fields = legacy_json.as_object_mut().unwrap();
        fields.remove("pdf_annots_imported");
        fields.remove("pdf_annots_file_len");
        bytes.extend_from_slice(&serde_json::to_vec(&legacy_json).unwrap());
        bytes.extend_from_slice(LEGACY_NOTES_END_TAG);
        let block = find_embedded_block(&bytes).unwrap();
        let restored = parse_embedded_data(&bytes[block.body_start..block.body_end]).unwrap();
        assert_eq!(restored.notes[&2], "lecture");
        assert_eq!(restored.annotations[&2][0].points[1], (0.3, 0.4));
        assert!(restored.blank_pages.contains(&3));
        assert_eq!(restored.sketches[&4].paper, BlankPageStyle::Grid);
        assert_eq!(restored.doc_id.as_deref(), Some("legacy-test-id"));
        assert_eq!(restored.rev, 123);
        assert!(!restored.pdf_annots_imported);
        assert_eq!(restored.pdf_annots_file_len, 0);
    }

    fn card_fronts(note: &str) -> Vec<String> {
        let mut m = HashMap::new();
        m.insert(1u32, note.to_string());
        extract_flashcards_from_notes(&m)
            .into_iter()
            .map(|c| c.front)
            .collect()
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
        assert_eq!(
            (c[0].front.as_str(), c[0].back.as_str()),
            ("What is *p?", "a pointer")
        );
    }

    fn skipped(text: &str) -> Vec<&str> {
        code_and_math_ranges(text)
            .into_iter()
            .map(|r| &text[r])
            .collect()
    }

    #[test]
    fn code_and_math_ranges_cover_code_and_latex_only() {
        assert_eq!(
            skipped("a `x y` b $m n$ c\n```\ncode\n```\nd $$\nz\n$$ e"),
            ["`x y`", "$m n$", "```\ncode\n```\n", "$$\nz\n$$"]
        );
        // Currency and unmatched markers are ordinary text.
        assert!(skipped("costs $5 and $6 total, `open and $$ open").is_empty());
        assert_eq!(skipped("``a ` b`` and `c"), ["``a ` b``"]);
        assert_eq!(skipped("```\nunclosed"), ["```\nunclosed"]);
        assert_eq!(skipped("é `ü` $α$ ok"), ["`ü`", "$α$"]);
        let r = code_and_math_ranges("a `b` c");
        assert!(in_ranges(&r, 2) && in_ranges(&r, 4) && !in_ranges(&r, 1) && !in_ranges(&r, 5));
    }

    #[test]
    fn markdown_math_preserves_shell_arguments_and_code() {
        let input = "Use `$@` and `$1` with $x$.\n```sh\necho \"$@\" $$ $1\n```\n~~~sh\necho $$ $@\n~~~\n$@ and $@; $y$";
        let rendered = preprocess_markdown_view(input);
        assert!(rendered.contains("`$@` and `$1` with `x`."));
        assert!(rendered.contains("```sh\necho \"$@\" $$ $1\n```"));
        assert!(rendered.contains("~~~sh\necho $$ $@\n~~~"));
        assert!(rendered.contains("\\$@ and \\$@; `y`"));
        assert!(preprocess_markdown_math("$$x$$").contains("**`x`**"));
        assert!(preprocess_markdown_math("$@ and $@").starts_with("$@ and $@"));
        assert_eq!(skipped("$@ and $@; $x$"), ["$x$"]);
    }

    #[test]
    fn flashcards_ignore_code_and_math() {
        assert!(card_fronts("```cpp\nstd::cout :: x\n```").is_empty());
        assert!(card_fronts("~~~\na :: b\n~~~").is_empty());
        assert!(card_fronts("Use `std::vector` for arrays").is_empty());
        assert!(card_fronts("Inline $a :: b$ math").is_empty());
        assert!(card_fronts("$$\nx :: y\n$$").is_empty());
        assert!(card_fronts("$$\nx\n$$ :: not an inline card").is_empty());
        assert!(card_fronts("```\nQ: not a card\nA: still not\n```").is_empty());
        // A real marker next to code still works.
        assert_eq!(
            card_fronts("`std::vector` :: dynamic array"),
            ["`std::vector`"]
        );
        assert_eq!(card_fronts("Vector :: uses `a::b` inside"), ["Vector"]);
    }

    #[test]
    fn preview_recognizes_inline_code_and_math_cards() {
        let note = "`word` :: something\n$a::b$ :: relation\n**Term** :: `x::y`\n```a ` b``` :: code";
        let cards = extract_flashcards_from_page(1, note);
        assert_eq!(cards.len(), 4);
        let preview = preprocess_notes_preview(note);
        assert!(preview.contains("> **`word`**"));
        assert!(preview.contains("> something"));
        assert!(preview.contains("> **Term**"));
        assert!(preview.contains("> `x::y`"));
        assert!(preview.contains("> **```a ` b```**"));
        assert!(!preview.contains(" :: relation"));
        assert!(preview.contains("> relation"));
    }

    #[test]
    fn preview_preserves_non_cards_and_formatted_answers() {
        let note = "`a::b`\n$$\nx :: y\n$$\n~~~rust\nQ: a :: b\n~~~\nQ: incomplete";
        assert!(extract_flashcards_from_page(1, note).is_empty());
        assert_eq!(preprocess_notes_preview(note), preprocess_markdown_view(note));

        let note = "Q: What is `$@`?\nA: Example $x^2$:\n```sh\n  echo \"$@\"\n\nQ: not a card\n```\n$$\ny^2\n$$\n\nD: Next\nR: Answer";
        let cards = extract_flashcards_from_page(1, note);
        assert_eq!(cards.len(), 2);
        assert!(cards[0].back.contains("  echo \"$@\""));
        let preview = preprocess_notes_preview(note);
        assert!(preview.contains("> ```sh\n>   echo \"$@\"\n> \n> Q: not a card\n> ```"));
        assert!(preview.contains("> **`y²`**"));
        assert!(preview.contains("> Next"));
        assert!(preview.contains("> Answer"));
    }

    #[test]
    fn flashcard_answers_may_contain_code() {
        let mut m = HashMap::new();
        m.insert(
            1u32,
            "Q: Print?\nA: Use:\n```cpp\nstd::cout << 1;\n\nQ: x\n```\ndone".to_string(),
        );
        let c = extract_flashcards_from_notes(&m);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].front, "Print?");
        assert_eq!(
            c[0].back,
            "Use:\n```cpp\nstd::cout << 1;\n\nQ: x\n```\ndone"
        );
        assert!(page_has_flashcards("Q: a\nA: b"));
        assert!(!page_has_flashcards("```\na :: b\n```"));
    }

    #[test]
    fn stray_asterisks_are_escaped_for_display() {
        assert_eq!(escape_stray_asterisks("*args"), "\\*args");
        assert_eq!(escape_stray_asterisks("A*"), "A\\*");
        assert_eq!(escape_stray_asterisks("a * b"), "a \\* b");
        assert_eq!(
            escape_stray_asterisks("**kwargs and *args"),
            "\\*\\*kwargs and \\*args"
        );
        // Real emphasis, list markers and code spans are left alone.
        assert_eq!(
            escape_stray_asterisks("a **bold** and *it* word"),
            "a **bold** and *it* word"
        );
        assert_eq!(escape_stray_asterisks("* item *x"), "* item \\*x");
        assert_eq!(escape_stray_asterisks("`a*b` and c*"), "`a*b` and c\\*");
    }

    use super::*;

    #[test]
    fn pdf_annotation_coordinates_round_trip_for_page_rotations() {
        let rect = [17.0, 23.0, 617.0, 823.0];
        for rotation in [0, 90, 180, 270] {
            for point in [(0.0, 0.0), (0.25, 0.7), (1.0, 1.0)] {
                let pdf = lapis_to_pdf_point(point, rect, rotation);
                let restored = pdf_to_lapis_point(pdf, rect, rotation);
                assert!((restored.0 - point.0).abs() < 0.0001);
                assert!((restored.1 - point.1).abs() < 0.0001);
            }
        }
    }

    #[test]
    fn incremental_marker_is_only_rollback_eligible_at_the_file_tail() {
        let base = b"%PDF-1.4\n%%EOF\n";
        let mut bytes = base.to_vec();
        bytes.extend_from_slice(b"new xref and objects\n");
        let marker_pos = bytes.len();
        bytes.extend_from_slice(LAPIS_INCREMENTAL_MARKER);
        bytes.extend_from_slice(base.len().to_string().as_bytes());
        bytes.extend_from_slice(b"\n%%EOF\n");
        assert_eq!(trailing_lapis_revision(&bytes), Some((marker_pos, base.len())));
        bytes.extend_from_slice(b"external incremental revision");
        assert!(trailing_lapis_revision(&bytes).is_none());

        // New format keeps the final startxref/offset/%%EOF intact.
        let mut bytes = base.to_vec();
        bytes.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \ntrailer\n<</Size 1>>\nstartxref\n15\n%%EOF\n");
        insert_revision_marker_before_startxref(&mut bytes, base.len()).unwrap();
        let marker_pos = bytes.windows(LAPIS_INCREMENTAL_MARKER.len())
            .rposition(|w| w == LAPIS_INCREMENTAL_MARKER).unwrap();
        assert_eq!(trailing_lapis_revision(&bytes), Some((marker_pos, base.len())));
        assert!(bytes.ends_with(b"startxref\n15\n%%EOF\n"));
        bytes.extend_from_slice(b"% external revision\n");
        assert!(trailing_lapis_revision(&bytes).is_none());
    }

    #[test]
    fn large_custom_metadata_is_stored_outside_the_eof_tail() {
        let mut data = DocData::default();
        data.notes.insert(1, "x".repeat(4096));
        let stream = custom_data_stream(&data).unwrap();
        assert!(stream.dict.get(b"Filter").is_err());
        assert!(find_embedded_block(&stream.content).is_some());

        // The synthetic PDF prefix before startxref is exactly 9 bytes.
        // A rollback offset must point into that prefix, never past the marker.
        let base_len = b"%PDF-1.4\n".len();
        let mut pdf = b"%PDF-1.4\nstartxref\n0\n%%EOF\n".to_vec();
        insert_revision_marker_before_startxref(&mut pdf, base_len).unwrap();
        let eof = pdf.windows(5).rposition(|w| w == b"%%EOF").unwrap();
        assert!(pdf.len() - eof <= 64);
        assert!(pdf.windows(b"startxref\n0\n%%EOF".len())
            .any(|w| w == b"startxref\n0\n%%EOF"));
        let marker_pos = pdf.windows(LAPIS_INCREMENTAL_MARKER.len())
            .rposition(|w| w == LAPIS_INCREMENTAL_MARKER).unwrap();
        assert_eq!(trailing_lapis_revision(&pdf), Some((marker_pos, base_len)));

        // Do not relax the parser to accept an invalid rollback boundary.
        let mut invalid = pdf.clone();
        let digits_start = marker_pos + LAPIS_INCREMENTAL_MARKER.len();
        invalid.splice(digits_start..digits_start + 1, b"12".iter().copied());
        assert!(trailing_lapis_revision(&invalid).is_none());
    }

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
        let root = isolated_dir!(identity_follows_the_file_and_newest_revision_wins);
        let pdf_a = root.join("a.pdf");
        fs::write(&pdf_a, b"%PDF-1.4\n%%EOF\n").unwrap();

        // Autosave is local-only; a close-time commit stores the portable block in the PDF.
        let mut notes = HashMap::new();
        notes.insert(1u32, "first".to_string());
        let _ = load_doc_data(&pdf_a);
        save_doc_data(
            &pdf_a,
            &notes,
            &HashMap::new(),
            HashSet::new(),
            &HashMap::new(),
        )
        .unwrap();
        let id_a = crate::docid::resolve_key(&pdf_a);
        assert!(read_trailing_doc_id(&pdf_a).is_none());
        assert!(load_local_backup(&pdf_a).unwrap().rev > 0);
        let initial = load_local_backup(&pdf_a).unwrap();
        append_test_custom_data(&pdf_a, &portable(&initial));
        assert_eq!(read_trailing_doc_id(&pdf_a).as_deref(), Some(id_a.as_str()));

        // 1) spostato/rinominato: stesso id, note intatte
        let sub = root.join("sub");
        fs::create_dir_all(&sub).unwrap();
        let pdf_b = sub.join("renamed.pdf");
        fs::rename(&pdf_a, &pdf_b).unwrap();
        assert_eq!(crate::docid::resolve_key(&pdf_b), id_a);
        assert_eq!(
            load_doc_data(&pdf_b).notes.get(&1).map(String::as_str),
            Some("first")
        );

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
        append_test_custom_data(&pdf_b, &remote);
        let loaded = load_doc_data(&pdf_b);
        assert_eq!(loaded.notes[&1], "edited elsewhere");
        assert_eq!(
            load_local_backup(&pdf_b).unwrap().notes[&1],
            "edited elsewhere"
        );
        let stashed = fs::read_dir(get_backup_dir())
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(&format!("{id_a}.conflict-"))
            })
            .count();
        assert_eq!(stashed, 1, "la versione scartata deve essere conservata");

        // 4) un salvataggio successivo ha sempre revisione maggiore di quella caricata
        let mut n2 = HashMap::new();
        n2.insert(1u32, "local edit".to_string());
        save_doc_data(
            &pdf_b,
            &n2,
            &HashMap::new(),
            HashSet::new(),
            &HashMap::new(),
        )
        .unwrap();
        assert!(load_local_backup(&pdf_b).unwrap().rev > remote.rev);

        // 5) A newer local backup wins without rewriting the PDF during open.
        let mut newer = DocData::default();
        newer.notes.insert(1, "local newer".into());
        newer.rev = now_ms() + 60_000;
        save_local_backup(&pdf_b, &newer);
        assert_eq!(load_doc_data(&pdf_b).notes[&1], "local newer");
        assert_eq!(read_trailing_doc_data(&pdf_b).unwrap().notes[&1], "edited elsewhere");

        // 6) dati senza revisione (versioni precedenti): regola storica, il backup vince
        //    e le pagine mancanti arrivano dal PDF
        let mut old_pdf = DocData::default();
        old_pdf.notes.insert(2, "only in pdf".into());
        append_test_custom_data(&pdf_c, &old_pdf); // rev 0
        let mut old_bak = DocData::default();
        old_bak.notes.insert(1, "in backup".into());
        save_local_backup(&pdf_c, &old_bak); // rev 0
        let merged = load_doc_data(&pdf_c);
        assert_eq!(merged.notes.len(), 2);
        assert_eq!(merged.notes[&1], "in backup");

    }
}

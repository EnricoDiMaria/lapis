//! PDF validation and table of contents extraction.

use mupdf::Document;
use std::path::Path;

#[derive(Clone)]
pub(crate) struct TocItem {
    pub(crate) title: String,
    pub(crate) page: u32,
}

/// Validate a candidate before replacing the active document or its lock.
pub(crate) fn inspect_pdf(path: &Path) -> Result<(u32, Vec<TocItem>, (f32, f32)), String> {
    let doc =
        Document::open(path.to_str().ok_or("Invalid file path")?).map_err(|e| e.to_string())?;
    if !doc.is_pdf() {
        return Err("The selected file is not a PDF".into());
    }
    if doc.needs_password().map_err(|e| e.to_string())? {
        return Err("Password-protected PDFs must be unlocked before opening".into());
    }
    let count = doc.page_count().map_err(|e| e.to_string())?;
    if count <= 0 {
        return Err("The PDF has no pages".into());
    }
    let bounds = doc
        .load_page(0)
        .and_then(|p| p.bounds())
        .map_err(|e| e.to_string())?;
    let (w, h) = (bounds.x1 - bounds.x0, bounds.y1 - bounds.y0);
    if !w.is_finite() || !h.is_finite() || w <= 0.0 || h <= 0.0 {
        return Err("The first PDF page has invalid dimensions".into());
    }
    Ok((count as u32, outline(&doc), (w, h)))
}

pub(crate) fn outline(doc: &Document) -> Vec<TocItem> {
    let mut outline = Vec::new();
    if let Ok(nodes) = doc.outlines() {
        for node in nodes {
            let idx = node
                .dest
                .as_ref()
                .map(|d| d.loc.page_number as i32)
                .unwrap_or(0);
            outline.push(TocItem {
                title: node.title.clone(),
                page: (idx.max(0) as u32) + 1,
            });
        }
    }
    outline
}

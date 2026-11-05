use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

use flate2::write::ZlibEncoder;
use flate2::Compression;
use lopdf::content::{Content, Operation};
use lopdf::{dictionary, Document as LoDocument, Object, ObjectId, Stream};
use mupdf::{Colorspace, Document as MuDocument, Matrix, Page as MuPage, TextPageFlags};

use crate::notes::{AnnotationKind, PageAnnotation, prepare_image_xobjects};
use crate::state::ExportSettings;

pub const CANVAS_W: f64 = 960.0;
pub const CANVAS_H: f64 = 540.0;
const MARGIN: f64 = 18.0;

#[derive(Clone, Copy)]
struct Palette {
    page_bg: (f64, f64, f64),
    panel_bg: Option<(f64, f64, f64)>, // None = stesso colore della pagina
    divider: (f64, f64, f64),
    text: (f64, f64, f64),
    muted: (f64, f64, f64),
    code_bg: (f64, f64, f64),
}

const LIGHT: Palette = Palette {
    page_bg: (1.0, 1.0, 1.0), panel_bg: None,
    divider: (0.88, 0.89, 0.91), text: (0.12, 0.13, 0.16),
    muted: (0.48, 0.50, 0.56), code_bg: (0.95, 0.955, 0.965),
};
const DARK: Palette = Palette {
    page_bg: (0.075, 0.082, 0.110), panel_bg: Some((0.105, 0.114, 0.153)),
    divider: (0.22, 0.25, 0.35), text: (0.86, 0.89, 0.95),
    muted: (0.60, 0.64, 0.74), code_bg: (0.16, 0.19, 0.28),
};

fn fill_rect(ops: &mut Vec<Operation>, x: f64, y: f64, w: f64, h: f64, c: (f64, f64, f64)) {
    ops.push(Operation::new("q", vec![]));
    ops.push(Operation::new("rg", vec![c.0.into(), c.1.into(), c.2.into()]));
    ops.push(Operation::new("re", vec![x.into(), y.into(), w.into(), h.into()]));
    ops.push(Operation::new("f", vec![]));
    ops.push(Operation::new("Q", vec![]));
}

#[derive(Clone, Copy, Default, PartialEq)]
struct SpanStyle {
    bold: bool,
    italic: bool,
    code: bool,
}

#[derive(Clone)]
struct StyledSpan {
    text: String,
    style: SpanStyle,
}

#[derive(Clone)]
enum NoteLineKind {
    Blank,
    Rule,
    Heading(u8, Vec<StyledSpan>),
    Bullet(usize, Vec<StyledSpan>),
    Quote(Vec<StyledSpan>),
    CodeLine(String),
    Paragraph(Vec<StyledSpan>),
}

#[derive(Clone)]
struct LayoutLine {
    kind: NoteLineKind,
    height: f64,
    font_size: f64,
}

/// Computes exact split coordinates so notes never overflow the right edge of the 16:9 page.
pub fn compute_export_geometry(slide_width_pct: f32) -> (f64, f64, f64) {
    let slide_ratio = (slide_width_pct as f64 / 100.0).clamp(0.40, 0.80);
    let split_x = (CANVAS_W * slide_ratio).round();
    let text_x = split_x + MARGIN;
    let avail_text_w = (CANVAS_W - text_x - MARGIN).max(100.0);
    (split_x, text_x, avail_text_w)
}

/// Fallback: every page is rasterised on a fixed 16:9 canvas (used only when the source PDF
/// cannot be processed by `export_vector`).
fn export_rasterized(
    source_pdf: &Path,
    output_pdf: &Path,
    notes: &HashMap<u32, String>,
    annotations: &HashMap<u32, Vec<PageAnnotation>>,
    settings: &ExportSettings,
    hidden_pages: &std::collections::HashSet<u32>,
) -> Result<(), Box<dyn std::error::Error>> {
    let no_notes = HashMap::new();
    let no_anns = HashMap::new();
    let notes = if settings.content.includes_notes() { notes } else { &no_notes };
    let annotations = if settings.content.includes_annotations() { annotations } else { &no_anns };
    let src_str = source_pdf
        .to_str()
        .ok_or("Invalid UTF-8 path for source PDF")?;
    let mu_doc = MuDocument::open(src_str)?;
    let total_pages = mu_doc.page_count()?.max(1) as u32;

    let mut pdf = LoDocument::with_version("1.5");
    let pages_id = pdf.new_object_id();

    // Embed JetBrainsMono Nerd Font (or standard WinAnsi monospace fallback descriptor)
    let font_regular_id = embed_monospace_font(&mut pdf, "JetBrainsMono-Regular")?;
    let font_bold_id = embed_monospace_font(&mut pdf, "JetBrainsMono-Bold")?;
    let font_italic_id = embed_monospace_font(&mut pdf, "JetBrainsMono-Italic")?;

    let (split_x, text_x, avail_text_w) = compute_export_geometry(settings.slide_width_pct);
    // Base font size 20.0 maps to 13.0pt PDF body size so 12..32pt scales smoothly
    let font_scale = (settings.base_font_size as f64 / 20.0).clamp(0.60, 1.65);

    let mut page_ids: Vec<ObjectId> = Vec::new();
    let media_dir = crate::audio::AudioDocIndex::media_dir_for_pdf(source_pdf).join("screenshots");
    let mut img_cache = std::collections::HashMap::new();
    for p_num in 1..=total_pages {
        if hidden_pages.contains(&p_num) {
            continue;
        }
        let mu_page = mu_doc.load_page((p_num - 1) as i32)?;
        let raw_note = notes.get(&p_num).map(|s| s.as_str()).unwrap_or("").trim();
        let has_note = !raw_note.is_empty();
        let pal = if has_note { LIGHT } else { DARK };
        let page_anns = annotations
            .get(&p_num)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);

        let prepared = prepare_image_xobjects(&mut pdf, page_anns, &media_dir, &mut img_cache);
        // Render slide to RGB pixmap at crisp 2.0x scale and compress into an Image XObject
        let (img_id, pix_w, pix_h) = render_slide_xobject(&mut pdf, &mu_page, 2.0)?;

        if !has_note {
            let avail_w = CANVAS_W - (MARGIN * 2.0); // Oppure CANVAS_W puro se vuoi zero margini
            let avail_h = CANVAS_H - (MARGIN * 2.0); // Oppure CANVAS_H puro
            let (sx, sy, sw, sh) =
                fit_rect_in_box(0.0, 0.0, CANVAS_W, CANVAS_H, avail_w, avail_h, pix_w, pix_h);

            let mut ops = Vec::new();
            draw_background(&mut ops, false, split_x, &pal);
            draw_slide_xobject(&mut ops, sx, sy, sw, sh);
            append_selectable_pdf_text_ops(&mut ops, &mu_page, sx, sy, sw, sh);
            draw_pdf_annotations(&mut ops, page_anns, &prepared.names, sx, sy, sw, sh, sw / (pix_w as f64 / 2.0));

            let page_id = add_export_page(
                &mut pdf,
                pages_id,
                img_id,
                font_regular_id,
                font_bold_id,
                font_italic_id,
                ops,
            )?;
            prepared.register_on(&mut pdf, page_id);
            page_ids.push(page_id);
        } else {
            // Side-by-side 16:9 split layout: Slide on the left, paginated Markdown notes on the right
            let left_box_w = (split_x - MARGIN * 2.0).max(120.0);
            let left_box_h = CANVAS_H - MARGIN * 2.0;
            let (sx, sy, sw, sh) = fit_rect_in_box(
                0.0, 0.0, split_x, CANVAS_H, left_box_w, left_box_h, pix_w, pix_h,
            );

            // Preprocess $LaTeX$ and $$LaTeX$$ expressions before laying out lines
            let processed_md = crate::notes::preprocess_markdown_math(raw_note);
            let layout_lines = layout_note_lines(&processed_md, avail_text_w, font_scale);
            let note_chunks = paginate_note_lines(&layout_lines, CANVAS_H - MARGIN * 2.4);
            let total_parts = note_chunks.len().max(1);

            for (part_idx, chunk) in note_chunks.into_iter().enumerate() {
                let mut ops = Vec::new();
                draw_background(&mut ops, true, split_x, &pal);
                draw_slide_xobject(&mut ops, sx, sy, sw, sh);
                ops.push(Operation::new("q", vec![]));
                ops.push(Operation::new("RG", vec![pal.divider.0.into(), pal.divider.1.into(), pal.divider.2.into()]));
                ops.push(Operation::new("w", vec![0.6.into()]));
                ops.push(Operation::new("re", vec![sx.into(), sy.into(), sw.into(), sh.into()]));
                ops.push(Operation::new("S", vec![]));
                ops.push(Operation::new("Q", vec![]));
                append_selectable_pdf_text_ops(&mut ops, &mu_page, sx, sy, sw, sh);
                draw_pdf_annotations(&mut ops, page_anns, &prepared.names, sx, sy, sw, sh, sw / (pix_w as f64 / 2.0));

                // Right column header badge
                let header_y = CANVAS_H - MARGIN - 11.0;
                let header_str = if total_parts > 1 {
                    format!(
                        "Slide {} / {}  (Notes {}/{})",
                        p_num,
                        total_pages,
                        part_idx + 1,
                        total_parts
                    )
                } else {
                    format!("Slide {} / {}", p_num, total_pages)
                };
                push_styled_span(
                    &mut ops,
                    &header_str,
                    text_x,
                    header_y,
                    8.5 * font_scale.clamp(0.85, 1.2),
                    pal.muted,
                    SpanStyle::default(),
                    false,
                );

                // Subtle horizontal divider under header
                let div_y = header_y - 6.0;
                ops.push(Operation::new("q", vec![]));
                ops.push(Operation::new(
                    "RG",
                    vec![pal.divider.0.into(), pal.divider.1.into(), pal.divider.2.into()],
                ));
                ops.push(Operation::new("w", vec![0.5.into()]));
                ops.push(Operation::new("m", vec![text_x.into(), div_y.into()]));
                ops.push(Operation::new(
                    "l",
                    vec![(CANVAS_W - MARGIN).into(), div_y.into()],
                ));
                ops.push(Operation::new("S", vec![]));
                ops.push(Operation::new("Q", vec![]));

                // Render Markdown lines inside right column
                let mut cur_y = div_y - 14.0;
                for line in chunk {
                    cur_y -= line.font_size;
                    render_layout_line(&mut ops, &line, text_x, cur_y, avail_text_w, &pal);
                    cur_y -= line.height - line.font_size;
                }

                let page_id = add_export_page(
                    &mut pdf,
                    pages_id,
                    img_id,
                    font_regular_id,
                    font_bold_id,
                    font_italic_id,
                    ops,
                )?;
                prepared.register_on(&mut pdf, page_id);
                page_ids.push(page_id);
            }
        }
    }

    let kids: Vec<Object> = page_ids.iter().copied().map(Object::Reference).collect();
    pdf.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => kids,
            "Count" => page_ids.len() as i64,
        }),
    );

    let catalog_id = pdf.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    pdf.trailer.set("Root", catalog_id);
    pdf.compress();
    pdf.save(output_pdf)?;

    Ok(())
}

fn embed_monospace_font(
    pdf: &mut LoDocument,
    base_name: &str,
) -> Result<ObjectId, Box<dyn std::error::Error>> {
    // Standard Courier / WinAnsi metrics ensure 100% compatibility and crisp monospace kerning
    let ps_font = if base_name.contains("Bold") {
        "Courier-Bold"
    } else if base_name.contains("Italic") {
        "Courier-Oblique"
    } else {
        "Courier"
    };

    let font_id = pdf.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => ps_font,
        "Encoding" => "WinAnsiEncoding",
    });
    Ok(font_id)
}

fn render_slide_xobject(
    pdf: &mut LoDocument,
    page: &MuPage,
    scale: f32,
) -> Result<(ObjectId, f64, f64), Box<dyn std::error::Error>> {
    let pix = page.to_pixmap(
        &Matrix::new_scale(scale, scale),
        &Colorspace::device_rgb(),
        false,
        false,
    )?;
    let w = pix.width() as usize;
    let h = pix.height() as usize;
    let samples = pix.samples();

    // MuPDF RGB pixmap might have stride padding; pack tight 3-byte RGB rows
    let stride = pix.stride() as usize;
    let mut rgb_tight = Vec::with_capacity(w * h * 3);
    for row in 0..h {
        let start = row * stride;
        let end = start + w * 3;
        if end <= samples.len() {
            rgb_tight.extend_from_slice(&samples[start..end]);
        }
    }

    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&rgb_tight)?;
    let compressed = encoder.finish()?;

    let img_dict = dictionary! {
        "Type" => "XObject",
        "Subtype" => "Image",
        "Width" => w as i64,
        "Height" => h as i64,
        "ColorSpace" => "DeviceRGB",
        "BitsPerComponent" => 8,
        "Filter" => "FlateDecode",
    };

    let img_id = pdf.add_object(Stream::new(img_dict, compressed));
    Ok((img_id, w as f64, h as f64))
}

fn fit_rect_in_box(
    box_x: f64,
    box_y: f64,
    box_w: f64,
    box_h: f64,
    max_w: f64,
    max_h: f64,
    img_w: f64,
    img_h: f64,
) -> (f64, f64, f64, f64) {
    let ar = (img_w / img_h.max(1.0)).max(0.1);
    let mut w = max_w;
    let mut h = w / ar;
    if h > max_h {
        h = max_h;
        w = h * ar;
    }
    let sx = box_x + (box_w - w) * 0.5;
    let sy = box_y + (box_h - h) * 0.5;
    (sx, sy, w, h)
}

fn draw_background(ops: &mut Vec<Operation>, split: bool, split_x: f64, pal: &Palette) {
    ops.push(Operation::new("q", vec![]));
    ops.push(Operation::new(
        "rg",
        vec![pal.page_bg.0.into(), pal.page_bg.1.into(), pal.page_bg.2.into()],
    ));
    ops.push(Operation::new(
        "re",
        vec![0.into(), 0.into(), CANVAS_W.into(), CANVAS_H.into()],
    ));
    ops.push(Operation::new("f", vec![]));

    if split {
        if let Some(pbg) = pal.panel_bg {
            ops.push(Operation::new(
                "rg",
                vec![pbg.0.into(), pbg.1.into(), pbg.2.into()],
            ));
            ops.push(Operation::new(
                "re",
                vec![
                    split_x.into(),
                    0.into(),
                    (CANVAS_W - split_x).into(),
                    CANVAS_H.into(),
                ],
            ));
            ops.push(Operation::new("f", vec![]));
        }

        // Vertical separator line at `split_x`
        ops.push(Operation::new(
            "RG",
            vec![pal.divider.0.into(), pal.divider.1.into(), pal.divider.2.into()],
        ));
        ops.push(Operation::new("w", vec![0.6.into()]));
        ops.push(Operation::new("m", vec![split_x.into(), 24.0.into()]));
        ops.push(Operation::new("l", vec![split_x.into(), (CANVAS_H - 24.0).into()]));
        ops.push(Operation::new("S", vec![]));
    }
    ops.push(Operation::new("Q", vec![]));
}

fn draw_slide_xobject(ops: &mut Vec<Operation>, sx: f64, sy: f64, sw: f64, sh: f64) {
    ops.push(Operation::new("q", vec![]));
    ops.push(Operation::new(
        "cm",
        vec![
            sw.into(),
            0.into(),
            0.into(),
            sh.into(),
            sx.into(),
            sy.into(),
        ],
    ));
    ops.push(Operation::new("Do", vec![Object::Name(b"Im0".to_vec())]));
    ops.push(Operation::new("Q", vec![]));
}

/// Injects an invisible (`3 Tr`) selectable and searchable text layer over the slide rect
/// so exported PDFs preserve 100% of the original PDF's selectable text!
fn append_selectable_pdf_text_ops(
    ops: &mut Vec<Operation>,
    page: &MuPage,
    slide_x: f64,
    slide_y: f64,
    slide_w: f64,
    slide_h: f64,
) {
    let Ok(bounds) = page.bounds() else { return };
    let pw = (bounds.x1 - bounds.x0) as f64;
    let ph = (bounds.y1 - bounds.y0) as f64;
    if pw <= 0.0 || ph <= 0.0 {
        return;
    }
    let Ok(tp) = page.to_text_page(TextPageFlags::empty()) else {
        return;
    };

    ops.push(Operation::new("q", vec![]));
    ops.push(Operation::new("BT", vec![]));
    ops.push(Operation::new("Tr", vec![Object::Integer(3)])); // 3 = Invisible selectable text

    for block in tp.blocks() {
        for line in block.lines() {
            let mut line_str = String::new();
            let mut first_quad = None;
            let mut last_quad = None;

            for ch in line.chars() {
                if let Some(c) = ch.char() {
                    if first_quad.is_none() {
                        first_quad = Some(ch.quad());
                    }
                    last_quad = Some(ch.quad());
                    line_str.push(c);
                }
            }

            let trimmed = line_str.trim();
            if trimmed.is_empty() {
                continue;
            }

            if let (Some(fq), Some(lq)) = (first_quad, last_quad) {
                let nx = ((fq.ll.x - bounds.x0) as f64 / pw).clamp(0.0, 1.0);
                let ny = ((fq.ll.y - bounds.y0) as f64 / ph).clamp(0.0, 1.0);
                let span_w_norm =
                    (((lq.lr.x - fq.ll.x).abs() as f64) / pw).clamp(0.005, 1.0);
                let char_h_pt =
                    (((fq.ll.y - fq.ul.y).abs() as f64 / ph) * slide_h).clamp(5.0, 42.0);

                let pdf_x = slide_x + nx * slide_w;
                let pdf_y = slide_y + (1.0 - ny) * slide_h;

                let win_bytes = encode_winansi_lossy(trimmed);
                if win_bytes.is_empty() {
                    continue;
                }

                // Adjust horizontal scaling (`Tz`) so the invisible Courier span matches the exact width of the PDF line
                let natural_w = (win_bytes.len() as f64) * char_h_pt * 0.60;
                let target_w = span_w_norm * slide_w;
                let h_scale = if natural_w > 1.0 {
                    ((target_w / natural_w) * 100.0).clamp(40.0, 220.0)
                } else {
                    100.0
                };

                ops.push(Operation::new(
                    "Tf",
                    vec![Object::Name(b"F1".to_vec()), (char_h_pt as f32).into()],
                ));
                ops.push(Operation::new("Tz", vec![(h_scale as f32).into()]));
                ops.push(Operation::new(
                    "Tm",
                    vec![
                        1.0.into(),
                        0.0.into(),
                        0.0.into(),
                        1.0.into(),
                        (pdf_x as f32).into(),
                        (pdf_y as f32).into(),
                    ],
                ));
                ops.push(Operation::new("Tj", vec![Object::string_literal(win_bytes)]));
            }
        }
    }

    ops.push(Operation::new("ET", vec![]));
    ops.push(Operation::new("Q", vec![]));
}

fn draw_pdf_annotations(
    ops: &mut Vec<Operation>,
    anns: &[PageAnnotation],
    images: &std::collections::HashMap<String, String>,
    sx: f64,
    sy: f64,
    sw: f64,
    sh: f64,
    k: f64,
) {
    if anns.is_empty() {
        return;
    }

    let to_pdf = |(nx, ny): (f32, f32)| -> (f64, f64) {
        (sx + (nx as f64) * sw, sy + (1.0 - (ny as f64)) * sh)
    };

    for ann in anns {
        let r = (ann.color[0] as f64) / 255.0;
        let g = (ann.color[1] as f64) / 255.0;
        let b = (ann.color[2] as f64) / 255.0;
        // Pen/shape width: the width chosen in the app, scaled with the slide (k = 1 on original pages).
        let lw = (ann.stroke_width as f64 * k).max(0.4);
        match ann.kind {
            AnnotationKind::Highlight => {
                if ann.points.len() >= 2 {
                    let (x1, y1) = to_pdf(ann.points[0]);
                    let (x2, y2) = to_pdf(*ann.points.last().unwrap());
                    let rx = x1.min(x2);
                    let ry = y1.min(y2);
                    let rw = (x2 - x1).abs().max(2.0 * k);
                    let rh = (y2 - y1).abs().max(4.0 * k);
                    // Semi-transparent fill, like on screen.
                    ops.push(Operation::new("q", vec![]));
                    ops.push(Operation::new("gs", vec![Object::Name(b"GsHl".to_vec())]));
                    ops.push(Operation::new("rg", vec![r.into(), g.into(), b.into()]));
                    ops.push(Operation::new("re", vec![rx.into(), ry.into(), rw.into(), rh.into()]));
                    ops.push(Operation::new("f", vec![]));
                    ops.push(Operation::new("Q", vec![]));
                }
            }
            AnnotationKind::Underline => {
                if ann.points.len() >= 2 {
                    let (x1, y1) = to_pdf(ann.points[0]);
                    let (x2, _) = to_pdf(*ann.points.last().unwrap());
                    ops.push(Operation::new("q", vec![]));
                    ops.push(Operation::new("RG", vec![r.into(), g.into(), b.into()]));
                    ops.push(Operation::new("w", vec![lw.into()]));
                    ops.push(Operation::new("J", vec![1.into()]));
                    ops.push(Operation::new("j", vec![1.into()]));
                    ops.push(Operation::new("m", vec![x1.into(), y1.into()]));
                    ops.push(Operation::new("l", vec![x2.into(), y1.into()]));
                    ops.push(Operation::new("S", vec![]));
                    ops.push(Operation::new("Q", vec![]));
                }
            }
            AnnotationKind::Freehand => {
                if ann.points.len() >= 2 {
                    ops.push(Operation::new("q", vec![]));
                    ops.push(Operation::new("RG", vec![r.into(), g.into(), b.into()]));
                    ops.push(Operation::new("w", vec![lw.into()]));
                    ops.push(Operation::new("J", vec![1.into()]));
                    ops.push(Operation::new("j", vec![1.into()]));
                    let (x0, y0) = to_pdf(ann.points[0]);
                    ops.push(Operation::new("m", vec![x0.into(), y0.into()]));
                    for &pt in &ann.points[1..] {
                        let (px, py) = to_pdf(pt);
                        ops.push(Operation::new("l", vec![px.into(), py.into()]));
                    }
                    ops.push(Operation::new("S", vec![]));
                    ops.push(Operation::new("Q", vec![]));
                }
            }
            AnnotationKind::Rect => {
                if ann.points.len() >= 2 {
                    let (x1, y1) = to_pdf(ann.points[0]);
                    let (x2, y2) = to_pdf(*ann.points.last().unwrap());
                    ops.push(Operation::new("q", vec![]));
                    ops.push(Operation::new("RG", vec![r.into(), g.into(), b.into()]));
                    ops.push(Operation::new("w", vec![lw.into()]));
                    ops.push(Operation::new("J", vec![1.into()]));
                    ops.push(Operation::new("j", vec![1.into()]));
                    ops.push(Operation::new(
                        "re",
                        vec![
                            x1.min(x2).into(),
                            y1.min(y2).into(),
                            (x2 - x1).abs().into(),
                            (y2 - y1).abs().into(),
                        ],
                    ));
                    ops.push(Operation::new("S", vec![]));
                    ops.push(Operation::new("Q", vec![]));
                }
            }
            AnnotationKind::Circle => {
                if ann.points.len() >= 2 {
                    let (x1, y1) = to_pdf(ann.points[0]);
                    let (x2, y2) = to_pdf(*ann.points.last().unwrap());
                    let cx = (x1 + x2) * 0.5;
                    let cy = (y1 + y2) * 0.5;
                    let rad = (x2 - x1).abs().min((y2 - y1).abs()) * 0.5;
                    let k = 0.5522847498;
                    let ox = rad * k;
                    let oy = rad * k;

                    ops.push(Operation::new("q", vec![]));
                    ops.push(Operation::new("RG", vec![r.into(), g.into(), b.into()]));
                    ops.push(Operation::new("w", vec![lw.into()]));
                    ops.push(Operation::new("J", vec![1.into()]));
                    ops.push(Operation::new("j", vec![1.into()]));
                    ops.push(Operation::new("m", vec![(cx + rad).into(), cy.into()]));
                    ops.push(Operation::new(
                        "c",
                        vec![
                            (cx + rad).into(),
                            (cy + oy).into(),
                            (cx + ox).into(),
                            (cy + rad).into(),
                            cx.into(),
                            (cy + rad).into(),
                        ],
                    ));
                    ops.push(Operation::new(
                        "c",
                        vec![
                            (cx - ox).into(),
                            (cy + rad).into(),
                            (cx - rad).into(),
                            (cy + oy).into(),
                            (cx - rad).into(),
                            cy.into(),
                        ],
                    ));
                    ops.push(Operation::new(
                        "c",
                        vec![
                            (cx - rad).into(),
                            (cy - oy).into(),
                            (cx - ox).into(),
                            (cy - rad).into(),
                            cx.into(),
                            (cy - rad).into(),
                        ],
                    ));
                    ops.push(Operation::new(
                        "c",
                        vec![
                            (cx + ox).into(),
                            (cy - rad).into(),
                            (cx + rad).into(),
                            (cy - oy).into(),
                            (cx + rad).into(),
                            cy.into(),
                        ],
                    ));
                    ops.push(Operation::new("S", vec![]));
                    ops.push(Operation::new("Q", vec![]));
                }
            }
            AnnotationKind::Line | AnnotationKind::Arrow => {
                if ann.points.len() >= 2 {
                    let (x1, y1) = to_pdf(ann.points[0]);
                    let (x2, y2) = to_pdf(*ann.points.last().unwrap());
                    ops.push(Operation::new("q", vec![]));
                    ops.push(Operation::new("RG", vec![r.into(), g.into(), b.into()]));
                    ops.push(Operation::new("w", vec![lw.into()]));
                    ops.push(Operation::new("J", vec![1.into()]));
                    ops.push(Operation::new("j", vec![1.into()]));
                    ops.push(Operation::new("m", vec![x1.into(), y1.into()]));
                    ops.push(Operation::new("l", vec![x2.into(), y2.into()]));

                    if ann.kind == AnnotationKind::Arrow {
                        let dx = x2 - x1;
                        let dy = y2 - y1;
                        let len = (dx * dx + dy * dy).sqrt();
                        if len > 4.0 {
                            let ux = dx / len;
                            let uy = dy / len;
                            let px = -uy;
                            let py = ux;
                            let hl = (lw * 4.2).clamp(9.0 * k, 18.0 * k);
                            let w1x = x2 - ux * hl + px * (hl * 0.55);
                            let w1y = y2 - uy * hl + py * (hl * 0.55);
                            let w2x = x2 - ux * hl - px * (hl * 0.55);
                            let w2y = y2 - uy * hl - py * (hl * 0.55);
                            ops.push(Operation::new("m", vec![x2.into(), y2.into()]));
                            ops.push(Operation::new("l", vec![w1x.into(), w1y.into()]));
                            ops.push(Operation::new("m", vec![x2.into(), y2.into()]));
                            ops.push(Operation::new("l", vec![w2x.into(), w2y.into()]));
                        }
                    }
                    ops.push(Operation::new("S", vec![]));
                    ops.push(Operation::new("Q", vec![]));
                }
            }
            
            AnnotationKind::TextBox => {
                if let Some(&pt) = ann.points.first() {
                    let (bg_mode, clean_text) = if let Some(rest) = ann.text.strip_prefix('\u{200B}') {
                        (1u8, rest)
                    } else if let Some(rest) = ann.text.strip_prefix('\u{200C}') {
                        (2u8, rest)
                    } else {
                        (0u8, ann.text.as_str())
                    };
                    if clean_text.trim().is_empty() {
                        continue;
                    }

                    let (tx, ty_top) = to_pdf(pt);
                    let right_x = ann
                        .points
                        .get(1)
                        .map(|&p| to_pdf(p).0)
                        .unwrap_or(tx + sw * 0.30);
                    let box_w = (right_x - tx).max(20.0);

                    let pdf_sz = ((ann.font_size as f64) * (sh / 800.0)).clamp(4.0, 120.0);
                    let pad = (pdf_sz * 0.35).round().clamp(2.0, 64.0);
                    let avail = (box_w - 2.0 * pad).max(10.0);
                    let line_step = pdf_sz * 1.2;
                    let base_style = SpanStyle {
                        bold: ann.bold,
                        italic: ann.italic,
                        code: false,
                    };

                    // 1. Pre-calcola le righe wrappate tenendo traccia di __ (underline) e ~~ (strike)
                    // Usiamo due flag temporanei codificati dentro un prefisso invisibile dello span
                    // così `wrap_styled_spans` spezza le parole correttamente senza cambiare struct!
                    let mut all_wrapped_lines: Vec<Vec<(StyledSpan, bool, bool)>> = Vec::new();
                    let mut u_on = false;
                    let mut s_on = false;
                    let mut max_line_w: f64 = 0.0;

                    for raw_line in clean_text.split('\n') {
                        let line_str = if let Some(rest) = raw_line.strip_prefix("- ") {
                            format!("• {rest}")
                        } else {
                            raw_line.to_string()
                        };

                        // Spezza la riga sui token __ e ~~
                        let mut segs: Vec<(String, bool, bool)> = Vec::new();
                        let bytes = line_str.as_bytes();
                        let mut i = 0;
                        let mut cur = String::new();
                        while i < bytes.len() {
                            if i + 1 < bytes.len() && bytes[i] == b'_' && bytes[i + 1] == b'_' {
                                if !cur.is_empty() {
                                    segs.push((std::mem::take(&mut cur), u_on, s_on));
                                }
                                u_on = !u_on;
                                i += 2;
                            } else if i + 1 < bytes.len() && bytes[i] == b'~' && bytes[i + 1] == b'~' {
                                if !cur.is_empty() {
                                    segs.push((std::mem::take(&mut cur), u_on, s_on));
                                }
                                s_on = !s_on;
                                i += 2;
                            } else if let Some(ch) = line_str[i..].chars().next() {
                                cur.push(ch);
                                i += ch.len_utf8();
                            } else {
                                break;
                            }
                        }
                        if !cur.is_empty() || segs.is_empty() {
                            segs.push((cur, u_on, s_on));
                        }

                        // Passiamo ogni segmento a wrap_styled_spans codificando (u, s) in style.code / bold
                        // oppure wrappando gli StyledSpan e mappando (u, s)
                        let spans: Vec<StyledSpan> = segs
                            .iter()
                            .map(|(t, u, s)| StyledSpan {
                                // Prefisso di 1 char invisibile di controllo per trasportare u/s attraverso wrap_styled_spans
                                text: match (*u, *s) {
                                    (false, false) => t.clone(),
                                    (true, false) => format!("\u{200C}{t}"),
                                    (false, true) => format!("\u{200D}{t}"),
                                    (true, true) => format!("\u{2060}{t}"),
                                },
                                style: base_style,
                            })
                            .collect();

                        let mut cur_u = segs.first().map_or(false, |x| x.1);
                        let mut cur_s = segs.first().map_or(false, |x| x.2);

                        for wrapped in wrap_styled_spans(&spans, avail, pdf_sz) {
                            let mut row: Vec<(StyledSpan, bool, bool)> = Vec::new();
                            let mut row_chars = 0usize;
                            for mut sp in wrapped {
                                if let Some(rest) = sp.text.strip_prefix('\u{200C}') {
                                    cur_u = true;
                                    cur_s = false;
                                    sp.text = rest.to_string();
                                } else if let Some(rest) = sp.text.strip_prefix('\u{200D}') {
                                    cur_u = false;
                                    cur_s = true;
                                    sp.text = rest.to_string();
                                } else if let Some(rest) = sp.text.strip_prefix('\u{2060}') {
                                    cur_u = true;
                                    cur_s = true;
                                    sp.text = rest.to_string();
                                }
                                row_chars += sp.text.chars().count();
                                row.push((sp, cur_u, cur_s));
                            }
                            max_line_w = max_line_w.max((row_chars as f64) * pdf_sz * 0.60);
                            all_wrapped_lines.push(row);
                        }
                    }

                    // 2. Sfondo (se attivo con il pulsante Bg)
                    if bg_mode != 0 {
                        let (br, bg_c, bb) = if bg_mode == 1 {
                            (0.16, 0.17, 0.20) // grigetto scuro
                        } else {
                            (0.95, 0.95, 0.97) // chiaro
                        };
                        let bg_w = (max_line_w + 2.0 * pad).min(box_w).max(16.0);
                        let bg_h = (all_wrapped_lines.len() as f64) * line_step + 2.0 * pad;
                        let bg_y = ty_top - bg_h;

                        ops.push(Operation::new("q", vec![]));
                        ops.push(Operation::new("rg", vec![br.into(), bg_c.into(), bb.into()]));
                        ops.push(Operation::new(
                            "re",
                            vec![tx.into(), bg_y.into(), bg_w.into(), bg_h.into()],
                        ));
                        ops.push(Operation::new("f", vec![]));
                        ops.push(Operation::new("Q", vec![]));
                    }

                    // 3. Disegno testo + linee di sottolineato e barrato
                    let mut y = ty_top - pad - pdf_sz;
                    let stroke_w = (pdf_sz * 0.065).max(0.8);

                    for wrapped in &all_wrapped_lines {
                        let mut cx = tx + pad;
                        for (sp, is_u, is_s) in wrapped {
                            let next_cx = push_styled_span(
                                ops,
                                &sp.text,
                                cx,
                                y,
                                pdf_sz,
                                (r, g, b),
                                sp.style,
                                true,
                            );
                            if (*is_u || *is_s) && next_cx > cx {
                                ops.push(Operation::new("q", vec![]));
                                ops.push(Operation::new("RG", vec![r.into(), g.into(), b.into()]));
                                ops.push(Operation::new("w", vec![stroke_w.into()]));
                                if *is_u {
                                    let uy = y - pdf_sz * 0.12;
                                    ops.push(Operation::new("m", vec![cx.into(), uy.into()]));
                                    ops.push(Operation::new("l", vec![next_cx.into(), uy.into()]));
                                    ops.push(Operation::new("S", vec![]));
                                }
                                if *is_s {
                                    let sy_line = y + pdf_sz * 0.28;
                                    ops.push(Operation::new("m", vec![cx.into(), sy_line.into()]));
                                    ops.push(Operation::new("l", vec![next_cx.into(), sy_line.into()]));
                                    ops.push(Operation::new("S", vec![]));
                                }
                                ops.push(Operation::new("Q", vec![]));
                            }
                            cx = next_cx;
                        }
                        y -= line_step;
                    }
                }
            }
            AnnotationKind::Image => {
                if ann.points.len() >= 2 {
                    if let Some(name) = images.get(&ann.text) {
                        let (x1, y1) = to_pdf(ann.points[0]);
                        let (x2, y2) = to_pdf(ann.points[1]);
                        ops.push(Operation::new("q", vec![]));
                        ops.push(Operation::new(
                            "cm",
                            vec![
                                (x2 - x1).abs().into(),
                                0.into(),
                                0.into(),
                                (y2 - y1).abs().into(),
                                x1.min(x2).into(),
                                y1.min(y2).into(),
                            ],
                        ));
                        ops.push(Operation::new("Do", vec![lopdf::Object::Name(name.as_bytes().to_vec())]));
                        ops.push(Operation::new("Q", vec![]));
                    }
                }
            }
            AnnotationKind::Tape => {
                if ann.points.len() >= 2 {
                    let (x1, y1) = to_pdf(ann.points[0]);
                    let (x2, y2) = to_pdf(*ann.points.last().unwrap());
                    let rx = x1.min(x2);
                    let ry = y1.min(y2);
                    let rw = (x2 - x1).abs().max(2.0 * k);
                    let rh = (y2 - y1).abs().max(4.0 * k);
                    // Occlusion tape: (almost) opaque fill with a white border.
                    ops.push(Operation::new("q", vec![]));
                    ops.push(Operation::new("gs", vec![Object::Name(b"GsTape".to_vec())]));
                    ops.push(Operation::new("rg", vec![r.into(), g.into(), b.into()]));
                    ops.push(Operation::new("re", vec![rx.into(), ry.into(), rw.into(), rh.into()]));
                    ops.push(Operation::new("f", vec![]));
                    ops.push(Operation::new("Q", vec![]));
                    ops.push(Operation::new("q", vec![]));
                    ops.push(Operation::new("RG", vec![1.0.into(), 1.0.into(), 1.0.into()]));
                    ops.push(Operation::new("w", vec![(1.5 * k).into()]));
                    ops.push(Operation::new("re", vec![rx.into(), ry.into(), rw.into(), rh.into()]));
                    ops.push(Operation::new("S", vec![]));
                    ops.push(Operation::new("Q", vec![]));
                }
            }
        }
    }
}

fn layout_note_lines(markdown: &str, avail_w: f64, font_scale: f64) -> Vec<LayoutLine> {
    let body_sz = (12.0 * font_scale).clamp(8.0, 24.0);
    let h1_sz = (17.0 * font_scale).clamp(11.0, 30.0);
    let h2_sz = (15.0 * font_scale).clamp(10.0, 27.0);
    let h3_sz = (13.5 * font_scale).clamp(9.0, 25.0);
    let code_sz = (11.0 * font_scale).clamp(7.5, 22.0);

    let mut out = Vec::new();
    let mut in_code_block = false;

    for raw_line in markdown.lines() {
        let trimmed_end = raw_line.trim_end();
        let trimmed = trimmed_end.trim_start();

        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }

        if in_code_block {
            // Exact character budget for monospace code lines (`0.60 * code_sz` per glyph)
            let max_chars = ((avail_w - 16.0) / (code_sz * 0.60)).floor().max(10.0) as usize;
            for wrapped in wrap_plain_str(trimmed_end, max_chars) {
                out.push(LayoutLine {
                    kind: NoteLineKind::CodeLine(wrapped),
                    font_size: code_sz,
                    height: code_sz * 1.38,
                });
            }
            continue;
        }

        if trimmed.is_empty() {
            out.push(LayoutLine {
                kind: NoteLineKind::Blank,
                font_size: body_sz * 0.5,
                height: body_sz * 0.65,
            });
            continue;
        }

        if trimmed == "---" || trimmed == "***" || trimmed == "___" {
            out.push(LayoutLine {
                kind: NoteLineKind::Rule,
                font_size: body_sz * 0.5,
                height: body_sz * 0.9,
            });
            continue;
        }

        // Headings (#, ##, ###)
        if let Some(h3) = trimmed.strip_prefix("### ") {
            let spans = parse_inline_spans(h3);
            for w_spans in wrap_styled_spans(&spans, avail_w, h3_sz) {
                out.push(LayoutLine {
                    kind: NoteLineKind::Heading(3, w_spans),
                    font_size: h3_sz,
                    height: h3_sz * 1.42,
                });
            }
            continue;
        }
        if let Some(h2) = trimmed.strip_prefix("## ") {
            let spans = parse_inline_spans(h2);
            for w_spans in wrap_styled_spans(&spans, avail_w, h2_sz) {
                out.push(LayoutLine {
                    kind: NoteLineKind::Heading(2, w_spans),
                    font_size: h2_sz,
                    height: h2_sz * 1.45,
                });
            }
            continue;
        }
        if let Some(h1) = trimmed.strip_prefix("# ") {
            let spans = parse_inline_spans(h1);
            for w_spans in wrap_styled_spans(&spans, avail_w, h1_sz) {
                out.push(LayoutLine {
                    kind: NoteLineKind::Heading(1, w_spans),
                    font_size: h1_sz,
                    height: h1_sz * 1.50,
                });
            }
            continue;
        }

        // Blockquote (> ...)
        if let Some(q_body) = trimmed.strip_prefix("> ") {
            let spans = parse_inline_spans(q_body);
            let quote_avail = (avail_w - 18.0).max(60.0);
            for w_spans in wrap_styled_spans(&spans, quote_avail, body_sz) {
                out.push(LayoutLine {
                    kind: NoteLineKind::Quote(w_spans),
                    font_size: body_sz,
                    height: body_sz * 1.38,
                });
            }
            continue;
        }

        // Bullet lists (- ..., * ..., + ...)
        let leading_spaces = trimmed_end.len() - trimmed.len();
        let indent_lvl = leading_spaces / 2;
        if let Some(b_body) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("+ "))
        {
            let indent_pt = (indent_lvl as f64) * 12.0 + 14.0;
            let bullet_avail = (avail_w - indent_pt).max(60.0);
            let spans = parse_inline_spans(b_body);
            for (idx, w_spans) in wrap_styled_spans(&spans, bullet_avail, body_sz)
                .into_iter()
                .enumerate()
            {
                if idx == 0 {
                    out.push(LayoutLine {
                        kind: NoteLineKind::Bullet(indent_lvl, w_spans),
                        font_size: body_sz,
                        height: body_sz * 1.36,
                    });
                } else {
                    // Continuation lines of a bullet stay aligned with the bullet text
                    let mut indented = vec![StyledSpan {
                        text: " ".repeat(indent_lvl * 2 + 2),
                        style: SpanStyle::default(),
                    }];
                    indented.extend(w_spans);
                    out.push(LayoutLine {
                        kind: NoteLineKind::Paragraph(indented),
                        font_size: body_sz,
                        height: body_sz * 1.36,
                    });
                }
            }
            continue;
        }

        // Standard Paragraph
        let spans = parse_inline_spans(trimmed);
        for w_spans in wrap_styled_spans(&spans, avail_w, body_sz) {
            out.push(LayoutLine {
                kind: NoteLineKind::Paragraph(w_spans),
                font_size: body_sz,
                height: body_sz * 1.36,
            });
        }
    }

    out
}

/// Parses `**bold**`, `*italic*`, and `` `inline code` `` into styled spans.
fn parse_inline_spans(line: &str) -> Vec<StyledSpan> {
    let mut spans = Vec::new();
    let mut cur = String::new();
    let mut style = SpanStyle::default();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '`' {
            if !cur.is_empty() {
                spans.push(StyledSpan {
                    text: std::mem::take(&mut cur),
                    style,
                });
            }
            style.code = !style.code;
            i += 1;
            continue;
        }
        if !style.code && i + 1 < chars.len() && chars[i] == '*' && chars[i + 1] == '*' {
            if !cur.is_empty() {
                spans.push(StyledSpan {
                    text: std::mem::take(&mut cur),
                    style,
                });
            }
            style.bold = !style.bold;
            i += 2;
            continue;
        }
        if !style.code && chars[i] == '*' {
            if !cur.is_empty() {
                spans.push(StyledSpan {
                    text: std::mem::take(&mut cur),
                    style,
                });
            }
            style.italic = !style.italic;
            i += 1;
            continue;
        }
        cur.push(chars[i]);
        i += 1;
    }

    if !cur.is_empty() {
        spans.push(StyledSpan { text: cur, style });
    }
    spans
}

/// Word-wraps styled spans strictly within `avail_w` (using Courier 0.60em glyph width)
/// so no word is ever clipped off the right side of the page!
fn wrap_styled_spans(
    spans: &[StyledSpan],
    avail_w: f64,
    font_size: f64,
) -> Vec<Vec<StyledSpan>> {
    // Courier Type1 glyph advance width is exactly 0.60 * font_size
    let char_w = font_size * 0.605;
    let max_cols = ((avail_w / char_w).floor() as usize).max(10);

    // Flatten into `(word_or_space, SpanStyle)` tokens
    let mut tokens: Vec<(String, SpanStyle)> = Vec::new();
    for sp in spans {
        let mut cur_tok = String::new();
        let mut in_space = false;
        for ch in sp.text.chars() {
            let is_sp = ch.is_whitespace();
            if !cur_tok.is_empty() && is_sp != in_space {
                tokens.push((std::mem::take(&mut cur_tok), sp.style));
            }
            in_space = is_sp;
            cur_tok.push(ch);
        }
        if !cur_tok.is_empty() {
            tokens.push((cur_tok, sp.style));
        }
    }

    let mut lines: Vec<Vec<StyledSpan>> = Vec::new();
    let mut cur_line: Vec<StyledSpan> = Vec::new();
    let mut cur_cols = 0usize;

    for (tok, st) in tokens {
        // Measure length AFTER ASCII/WinAnsi transliteration so math symbols don't exceed column budget
        let norm_tok = normalize_for_pdf(&tok);
        let tok_len = norm_tok.chars().count();

        if cur_cols == 0 && norm_tok.trim().is_empty() {
            continue;
        }

        if cur_cols + tok_len <= max_cols {
            push_or_merge_span(&mut cur_line, &norm_tok, st);
            cur_cols += tok_len;
        } else if tok_len > max_cols {
            // Very long unbroken token (e.g. URL or long formula): split cleanly at `max_cols`
            let mut rem: Vec<char> = norm_tok.chars().collect();
            while !rem.is_empty() {
                let space_left = max_cols.saturating_sub(cur_cols);
                if space_left == 0 {
                    lines.push(std::mem::take(&mut cur_line));
                    cur_cols = 0;
                    continue;
                }
                let take_n = space_left.min(rem.len());
                let chunk: String = rem.drain(..take_n).collect();
                push_or_merge_span(&mut cur_line, &chunk, st);
                cur_cols += take_n;
                if !rem.is_empty() {
                    lines.push(std::mem::take(&mut cur_line));
                    cur_cols = 0;
                }
            }
        } else {
            if !cur_line.is_empty() {
                lines.push(std::mem::take(&mut cur_line));
            }
            let trimmed_start = norm_tok.trim_start();
            if !trimmed_start.is_empty() {
                cur_cols = trimmed_start.chars().count();
                push_or_merge_span(&mut cur_line, trimmed_start, st);
            } else {
                cur_cols = 0;
            }
        }
    }

    if !cur_line.is_empty() {
        lines.push(cur_line);
    }
    if lines.is_empty() {
        lines.push(Vec::new());
    }
    lines
}

fn push_or_merge_span(line: &mut Vec<StyledSpan>, text: &str, style: SpanStyle) {
    if let Some(last) = line.last_mut() {
        if last.style == style {
            last.text.push_str(text);
            return;
        }
    }
    line.push(StyledSpan {
        text: text.to_string(),
        style,
    });
}

fn wrap_plain_str(s: &str, max_chars: usize) -> Vec<String> {
    let norm = normalize_for_pdf(s);
    if norm.is_empty() {
        return vec![String::new()];
    }
    let chars: Vec<char> = norm.chars().collect();
    chars
        .chunks(max_chars.max(8))
        .map(|c| c.iter().collect())
        .collect()
}

fn paginate_note_lines(lines: &[LayoutLine], max_col_h: f64) -> Vec<Vec<LayoutLine>> {
    let mut pages = Vec::new();
    let mut cur_page = Vec::new();
    let mut used_h = 0.0;

    for line in lines {
        if used_h + line.height > max_col_h && !cur_page.is_empty() {
            pages.push(std::mem::take(&mut cur_page));
            used_h = 0.0;
        }
        used_h += line.height;
        cur_page.push(line.clone());
    }
    if !cur_page.is_empty() {
        pages.push(cur_page);
    }
    if pages.is_empty() {
        pages.push(Vec::new());
    }
    pages
}

fn render_layout_line(ops: &mut Vec<Operation>, line: &LayoutLine, text_x: f64, y: f64, avail_w: f64, pal: &Palette) {
    let sz = line.font_size;
    let char_w = sz * 0.60;
    match &line.kind {
        NoteLineKind::Blank => {}
        NoteLineKind::Rule => fill_rect(ops, text_x, y + sz * 0.4, avail_w, 0.6, pal.divider),
        NoteLineKind::Heading(_, spans) => {
            let mut cx = text_x;
            for sp in spans {
                let mut st = sp.style; st.bold = true;
                cx = push_styled_span(ops, &sp.text, cx, y, sz, pal.text, st, true);
            }
        }
        NoteLineKind::Bullet(indent, spans) => {
            let ind = *indent as f64 * 12.0;
            fill_rect(ops, text_x + ind + 4.0, y + sz * 0.28, 2.6, 2.6, pal.muted);
            let mut cx = text_x + ind + 13.0;
            for sp in spans { cx = push_styled_span(ops, &sp.text, cx, y, sz, pal.text, sp.style, true); }
        }
        NoteLineKind::Quote(spans) => {
            fill_rect(ops, text_x, y - 2.0, 1.2, sz * 1.15, pal.divider);
            let mut cx = text_x + 9.0;
            for sp in spans { cx = push_styled_span(ops, &sp.text, cx, y, sz, pal.muted, sp.style, true); }
        }
        NoteLineKind::CodeLine(code) => {
            fill_rect(ops, text_x, y - 3.0, avail_w, sz * 1.32, pal.code_bg);
            push_styled_span(ops, code, text_x + 6.0, y, sz, pal.text,
                SpanStyle { code: true, ..Default::default() }, true);
        }
        NoteLineKind::Paragraph(spans) => {
            let mut cx = text_x;
            for sp in spans {
                if sp.style.code {
                    let w = sp.text.chars().count() as f64 * char_w + 4.0;
                    fill_rect(ops, cx - 1.5, y - 2.0, w, sz * 1.15, pal.code_bg);
                }
                cx = push_styled_span(ops, &sp.text, cx, y, sz, pal.text, sp.style, true);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn push_styled_span(
    ops: &mut Vec<Operation>,
    text: &str,
    x: f64,
    y: f64,
    font_size: f64,
    rgb: (f64, f64, f64),
    style: SpanStyle,
    already_normalized: bool,
) -> f64 {
    let norm = if already_normalized {
        text.to_string()
    } else {
        normalize_for_pdf(text)
    };
    let bytes = encode_winansi_lossy(&norm);
    let advance = (bytes.len() as f64) * font_size * 0.60;
    if bytes.is_empty() {
        return x;
    }

    let font_key: &[u8] = if style.bold {
        b"F2"
    } else if style.italic {
        b"F3"
    } else {
        b"F1"
    };

    ops.push(Operation::new("q", vec![]));
    ops.push(Operation::new("BT", vec![]));
    ops.push(Operation::new(
        "rg",
        vec![rgb.0.into(), rgb.1.into(), rgb.2.into()],
    ));
    ops.push(Operation::new(
        "Tf",
        vec![Object::Name(font_key.to_vec()), (font_size as f32).into()],
    ));
    ops.push(Operation::new(
        "Tm",
        vec![
            1.0.into(),
            0.0.into(),
            0.0.into(),
            1.0.into(),
            (x as f32).into(),
            (y as f32).into(),
        ],
    ));
    ops.push(Operation::new("Tj", vec![Object::string_literal(bytes)]));
    ops.push(Operation::new("ET", vec![]));
    ops.push(Operation::new("Q", vec![]));

    x + advance
}

/// Converts Unicode mathematical symbols outside WinAnsi (CP1252) into clean readable equivalents
/// so Type1 PDF WinAnsi fonts never print '?' for math formulas!
fn normalize_for_pdf(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            // Superscripts & Subscripts inside WinAnsi or readable ASCII
            '⁰' => out.push_str("^0"),
            '¹' => out.push('¹'),
            '²' => out.push('²'),
            '³' => out.push('³'),
            '⁴' => out.push_str("^4"),
            '⁵' => out.push_str("^5"),
            '⁶' => out.push_str("^6"),
            '⁷' => out.push_str("^7"),
            '⁸' => out.push_str("^8"),
            '⁹' => out.push_str("^9"),
            '⁺' => out.push_str("^+"),
            '⁻' => out.push_str("^-"),
            'ⁿ' => out.push_str("^n"),
            'ⁱ' => out.push_str("^i"),
            '₀' => out.push_str("_0"),
            '₁' => out.push_str("_1"),
            '₂' => out.push_str("_2"),
            '₃' => out.push_str("_3"),
            '₄' => out.push_str("_4"),
            '₅' => out.push_str("_5"),
            '₆' => out.push_str("_6"),
            '₇' => out.push_str("_7"),
            '₈' => out.push_str("_8"),
            '₉' => out.push_str("_9"),
            'ᵢ' => out.push_str("_i"),
            'ⱼ' => out.push_str("_j"),
            'ₖ' => out.push_str("_k"),
            'ₙ' => out.push_str("_n"),
            'ₓ' => out.push_str("_x"),
            // Arrows & Logic
            '→' | '⟶' => out.push_str("->"),
            '←' | '⟵' => out.push_str("<-"),
            '↔' => out.push_str("<->"),
            '⇒' | '⟹' => out.push_str("=>"),
            '⇐' | '⟸' => out.push_str("<="),
            '⇔' | '⟺' => out.push_str("<=>"),
            '↦' | '⟼' => out.push_str("|->"),
            // Relations & Calculus
            '≤' | '⩽' => out.push_str("<="),
            '≥' | '⩾' => out.push_str(">="),
            '≠' => out.push_str("!="),
            '≈' | '≃' => out.push_str("~="),
            '≡' => out.push_str("=="),
            '∞' => out.push_str("inf"),
            '∀' => out.push_str("forall "),
            '∃' => out.push_str("exists "),
            '∄' => out.push_str("!exists "),
            '∈' => out.push_str(" in "),
            '∉' => out.push_str(" !in "),
            '⊂' => out.push_str(" subset "),
            '⊆' => out.push_str(" subseteq "),
            '∪' => out.push_str(" U "),
            '∩' => out.push_str(" ^ "),
            '∅' => out.push_str("{}"),
            '∑' => out.push_str("sum"),
            '∏' => out.push_str("prod"),
            '∫' => out.push_str("int"),
            '∂' => out.push('d'),
            '∇' => out.push_str("grad"),
            '√' => out.push_str("sqrt"),
            // Blackboard sets & Greek letters
            'ℝ' => out.push('R'),
            'ℕ' => out.push('N'),
            'ℤ' => out.push('Z'),
            'ℚ' => out.push('Q'),
            'ℂ' => out.push('C'),
            'α' => out.push_str("alpha"),
            'β' => out.push_str("beta"),
            'γ' => out.push_str("gamma"),
            'Γ' => out.push_str("Gamma"),
            'δ' => out.push_str("delta"),
            'Δ' => out.push_str("Delta"),
            'ε' | 'ϵ' => out.push_str("eps"),
            'θ' | 'ϑ' => out.push_str("theta"),
            'Θ' => out.push_str("Theta"),
            'λ' => out.push_str("lambda"),
            'Λ' => out.push_str("Lambda"),
            'μ' => out.push('µ'), // 0xB5 in WinAnsi!
            'π' => out.push_str("pi"),
            'Π' => out.push_str("Pi"),
            'ρ' => out.push_str("rho"),
            'σ' => out.push_str("sigma"),
            'Σ' => out.push_str("Sigma"),
            'τ' => out.push_str("tau"),
            'φ' | 'ϕ' => out.push_str("phi"),
            'Φ' => out.push_str("Phi"),
            'ψ' => out.push_str("psi"),
            'ω' => out.push_str("omega"),
            'Ω' => out.push_str("Omega"),
            // Standard typography
            '“' | '”' => out.push('"'),
            '‘' | '’' => out.push('\''),
            '—' | '–' => out.push('-'),
            '…' => out.push_str("..."),
            c if (c as u32) <= 0xFF => out.push(c),
            // Strip private-use Nerd Font icons cleanly in PDF export
            c if ('\u{e000}'..='\u{f8ff}').contains(&c)
                || ('\u{f0000}'..='\u{fffff}').contains(&c) => {}
            _ => {}
        }
    }
    out
}

fn encode_winansi_lossy(s: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(s.len());
    for ch in s.chars() {
        let code = ch as u32;
        if (0x20..=0x7E).contains(&code) || (0xA0..=0xFF).contains(&code) {
            bytes.push(code as u8);
        } else {
            bytes.push(b' ');
        }
    }
    bytes
}

#[allow(clippy::too_many_arguments)]
fn add_export_page(
    pdf: &mut LoDocument,
    pages_id: ObjectId,
    img_id: ObjectId,
    font_reg: ObjectId,
    font_bold: ObjectId,
    font_italic: ObjectId,
    ops: Vec<Operation>,
) -> Result<ObjectId, Box<dyn std::error::Error>> {
    let content = Content { operations: ops };
    let content_bytes = content.encode()?;

    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&content_bytes)?;
    let compressed = encoder.finish()?;

    let stream = Stream::new(
        dictionary! {
            "Filter" => "FlateDecode",
        },
        compressed,
    );
    let content_id = pdf.add_object(stream);

    let resources = dictionary! {
        "Font" => dictionary! {
            "F1" => font_reg,
            "F2" => font_bold,
            "F3" => font_italic,
        },
        "XObject" => dictionary! {
            "Im0" => img_id,
        },
        "ExtGState" => ext_gstates(),
    };

    let page_id = pdf.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => vec![0.into(), 0.into(), CANVAS_W.into(), CANVAS_H.into()],
        "Resources" => resources,
        "Contents" => content_id,
    });

    Ok(page_id)
}

// ───────────────────── vector export: the original pages are reused ─────────────────────
//
// The source PDF is opened with lopdf and modified in place:
//  * pages without notes stay byte-for-byte the original page (same size, vector content,
//    selectable text, links) with the annotations drawn on top in an extra layer;
//  * pages with notes are rebuilt as "slide on the left (original size, no margins) + notes on
//    the right", where the slide is the original page embedded as a Form XObject (still vector).
// If lopdf cannot handle the source (encrypted/damaged files) the caller falls back to the
// rasterised export below.

struct PageGeo {
    /// Visible box (CropBox ∩ MediaBox) in the page's own user space.
    bbox: [f64; 4],
    /// Maps the user space to the "display space" (origin at the lower-left corner, after /Rotate).
    m: [f64; 6],
    /// Inverse of `m`.
    inv: [f64; 6],
    /// Displayed width / height in points.
    dw: f64,
    dh: f64,
}

fn es(e: lopdf::Error) -> String {
    e.to_string()
}

fn obj_num(o: &Object) -> Option<f64> {
    match o {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(r) => Some(*r as f64),
        _ => None,
    }
}

fn page_rect(doc: &LoDocument, page_id: ObjectId, key: &[u8]) -> Option<[f64; 4]> {
    let o = crate::notes::resolve_obj(doc, crate::notes::page_inherited(doc, page_id, key)?);
    let a = o.as_array().ok()?;
    if a.len() < 4 {
        return None;
    }
    let mut v = [0.0f64; 4];
    for (i, item) in a.iter().take(4).enumerate() {
        v[i] = obj_num(&crate::notes::resolve_obj(doc, item.clone()))?;
    }
    Some([v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])])
}

fn invert_affine(m: [f64; 6]) -> [f64; 6] {
    let det = m[0] * m[3] - m[1] * m[2];
    let det = if det.abs() < 1e-12 { 1.0 } else { det };
    [
        m[3] / det,
        -m[1] / det,
        -m[2] / det,
        m[0] / det,
        (m[2] * m[5] - m[3] * m[4]) / det,
        (m[1] * m[4] - m[0] * m[5]) / det,
    ]
}

fn page_geo(doc: &LoDocument, page_id: ObjectId) -> Option<PageGeo> {
    let media = page_rect(doc, page_id, b"MediaBox")?;
    let mut b = media;
    if let Some(c) = page_rect(doc, page_id, b"CropBox") {
        let x0 = media[0].max(c[0]);
        let y0 = media[1].max(c[1]);
        let x1 = media[2].min(c[2]);
        let y1 = media[3].min(c[3]);
        if x1 > x0 && y1 > y0 {
            b = [x0, y0, x1, y1];
        }
    }
    let rot = crate::notes::page_inherited(doc, page_id, b"Rotate")
        .map(|o| crate::notes::resolve_obj(doc, o))
        .and_then(|o| o.as_i64().ok())
        .unwrap_or(0);
    let rot = (((rot % 360) + 360) % 360) / 90 * 90;
    let (x0, y0, x1, y1) = (b[0], b[1], b[2], b[3]);
    let (w, h) = (x1 - x0, y1 - y0);
    let (m, dw, dh) = match rot {
        0 => ([1.0, 0.0, 0.0, 1.0, -x0, -y0], w, h),
        90 => ([0.0, -1.0, 1.0, 0.0, -y0, x1], h, w),
        180 => ([-1.0, 0.0, 0.0, -1.0, x1, y1], w, h),
        _ => ([0.0, 1.0, -1.0, 0.0, y1, -x0], h, w),
    };
    if dw <= 0.0 || dh <= 0.0 {
        return None;
    }
    Some(PageGeo { bbox: b, m, inv: invert_affine(m), dw, dh })
}

fn real_array(v: &[f64]) -> Object {
    Object::Array(v.iter().map(|x| Object::Real(*x as f32)).collect())
}

/// The original page as a Form XObject whose display space is [0,dw]x[0,dh] (rotation applied).
fn make_slide_form(pdf: &mut LoDocument, page_id: ObjectId, geo: &PageGeo) -> Result<ObjectId, String> {
    let content = pdf.get_page_content(page_id).map_err(es)?;
    let resources = match crate::notes::page_inherited(pdf, page_id, b"Resources")
        .map(|o| crate::notes::resolve_obj(pdf, o))
    {
        Some(Object::Dictionary(d)) => d,
        _ => lopdf::Dictionary::new(),
    };
    let dict = dictionary! {
        "Type" => "XObject",
        "Subtype" => "Form",
        "BBox" => real_array(&geo.bbox),
        "Matrix" => real_array(&geo.m),
        "Resources" => Object::Dictionary(resources),
    };
    Ok(pdf.add_object(Stream::new(dict, content)))
}

/// Annotation layer as a Form XObject of size `w` x `h` (display units); `k` scales stroke widths.
fn make_annotation_form(
    pdf: &mut LoDocument,
    anns: &[PageAnnotation],
    prepared: &crate::notes::PreparedImages,
    fonts: (ObjectId, ObjectId, ObjectId),
    w: f64,
    h: f64,
    k: f64,
) -> Result<ObjectId, Box<dyn std::error::Error>> {
    let mut ops = Vec::new();
    draw_pdf_annotations(&mut ops, anns, &prepared.names, 0.0, 0.0, w, h, k);
    let bytes = Content { operations: ops }.encode()?;
    let mut xobjects = lopdf::Dictionary::new();
    for (name, id) in prepared.xobject_entries() {
        xobjects.set(name.as_str(), *id);
    }
    let resources = dictionary! {
        "Font" => dictionary! { "F1" => fonts.0, "F2" => fonts.1, "F3" => fonts.2 },
        "XObject" => xobjects,
        "ExtGState" => ext_gstates(),
    };
    let dict = dictionary! {
        "Type" => "XObject",
        "Subtype" => "Form",
        "BBox" => real_array(&[0.0, 0.0, w, h]),
        "Resources" => resources,
    };
    Ok(pdf.add_object(Stream::new(dict, bytes)))
}

/// Transparency used by highlights and occlusion tapes (same alphas as the on-screen view).
fn ext_gstates() -> lopdf::Dictionary {
    dictionary! {
        "GsHl" => dictionary! { "Type" => "ExtGState", "ca" => Object::Real(95.0 / 255.0), "CA" => Object::Real(95.0 / 255.0) },
        "GsTape" => dictionary! { "Type" => "ExtGState", "ca" => Object::Real(248.0 / 255.0), "CA" => Object::Real(248.0 / 255.0) },
    }
}

/// Draws the annotation layer on top of an untouched original page.
fn overlay_page(
    pdf: &mut LoDocument,
    page_id: ObjectId,
    geo: &PageGeo,
    ann_form: ObjectId,
) -> Result<(), String> {
    crate::notes::register_xobject_on_page(pdf, page_id, "LpAnn", ann_form)?;
    let pre = pdf.add_object(Stream::new(dictionary! {}, b"q\n".to_vec()));
    let i = geo.inv;
    let post_src = format!(
        "\nQ\nq {:.6} {:.6} {:.6} {:.6} {:.6} {:.6} cm /LpAnn Do Q\n",
        i[0], i[1], i[2], i[3], i[4], i[5]
    );
    let post = pdf.add_object(Stream::new(dictionary! {}, post_src.into_bytes()));

    let existing: Vec<Object> = match pdf.get_dictionary(page_id).map_err(es)?.get(b"Contents") {
        Ok(Object::Reference(id)) => match pdf.get_object(*id) {
            Ok(Object::Array(a)) => a.clone(),
            _ => vec![Object::Reference(*id)],
        },
        Ok(Object::Array(a)) => a.clone(),
        _ => Vec::new(),
    };
    let mut all = vec![Object::Reference(pre)];
    all.extend(existing);
    all.push(Object::Reference(post));
    pdf.get_object_mut(page_id)
        .and_then(|o| o.as_dict_mut())
        .map_err(es)?
        .set("Contents", Object::Array(all));
    Ok(())
}

fn with_kids<R>(
    pdf: &mut LoDocument,
    parent: ObjectId,
    f: impl FnOnce(&mut Vec<Object>) -> R,
) -> Result<R, String> {
    let kids = pdf
        .get_dictionary(parent)
        .map_err(es)?
        .get(b"Kids")
        .map_err(es)?
        .clone();
    match kids {
        Object::Reference(id) => {
            let arr = pdf.get_object_mut(id).map_err(es)?.as_array_mut().map_err(es)?;
            Ok(f(arr))
        }
        Object::Array(_) => {
            let d = pdf.get_object_mut(parent).map_err(es)?.as_dict_mut().map_err(es)?;
            let arr = d.get_mut(b"Kids").map_err(es)?.as_array_mut().map_err(es)?;
            Ok(f(arr))
        }
        _ => Err("page tree without Kids".to_string()),
    }
}

fn bump_count(pdf: &mut LoDocument, mut node: ObjectId, delta: i64) {
    loop {
        let Ok(d) = pdf.get_object_mut(node).and_then(|o| o.as_dict_mut()) else {
            break;
        };
        if let Ok(c) = d.get(b"Count").and_then(Object::as_i64) {
            d.set("Count", c + delta);
        }
        match d.get(b"Parent").and_then(Object::as_reference) {
            Ok(p) => node = p,
            Err(_) => break,
        }
    }
}

fn page_parent(pdf: &LoDocument, page_id: ObjectId) -> Result<ObjectId, String> {
    pdf.get_dictionary(page_id)
        .map_err(es)?
        .get(b"Parent")
        .and_then(Object::as_reference)
        .map_err(es)
}

fn detach_page(pdf: &mut LoDocument, page_id: ObjectId) -> Result<(), String> {
    let parent = page_parent(pdf, page_id)?;
    with_kids(pdf, parent, |kids| {
        kids.retain(|o| o.as_reference().ok() != Some(page_id))
    })?;
    bump_count(pdf, parent, -1);
    pdf.delete_object(page_id);
    Ok(())
}

fn insert_page_after(pdf: &mut LoDocument, after: ObjectId, new_page: ObjectId) -> Result<(), String> {
    let parent = page_parent(pdf, after)?;
    with_kids(pdf, parent, |kids| {
        match kids.iter().position(|o| o.as_reference().ok() == Some(after)) {
            Some(pos) => kids.insert(pos + 1, Object::Reference(new_page)),
            None => kids.push(Object::Reference(new_page)),
        }
    })?;
    bump_count(pdf, parent, 1);
    Ok(())
}

fn op_do(name: &str) -> Operation {
    Operation::new("Do", vec![Object::Name(name.as_bytes().to_vec())])
}

fn op_cm(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) -> Operation {
    Operation::new("cm", vec![a.into(), b.into(), c.into(), d.into(), e.into(), f.into()])
}

/// Rebuilds `page_id` as "slide | notes" (the slide keeps its original size and has no margins);
/// notes that do not fit continue on additional pages that repeat the slide.
#[allow(clippy::too_many_arguments)]
fn build_notes_pages(
    pdf: &mut LoDocument,
    page_id: ObjectId,
    geo: &PageGeo,
    slide_form: ObjectId,
    ann_form: Option<ObjectId>,
    fonts: (ObjectId, ObjectId, ObjectId),
    p_num: u32,
    total_pages: u32,
    raw_note: &str,
    ratio: f64,
    font_scale: f64,
) -> Result<(), Box<dyn std::error::Error>> {
    let pal = LIGHT;
    let h_c = CANVAS_H;
    let base_s_c = h_c / geo.dh;
    let s_c = base_s_c * ratio;
    let sw_c = geo.dw * s_c;
    let sh_c = geo.dh * s_c;
    let w_total = (geo.dw * base_s_c).round();
    let text_x = sw_c + MARGIN;
    let avail_text_w = w_total - text_x - MARGIN;
    let slide_y = (h_c - sh_c) / 2.0;
    let avail_text_w = w_total - text_x - MARGIN;
    let k = geo.dh / h_c; // canvas units -> points: the slide is drawn at exactly its original size
    let parent_id = page_parent(pdf, page_id)?;

    let processed_md = crate::notes::preprocess_markdown_math(raw_note);
    let layout_lines = layout_note_lines(&processed_md, avail_text_w, font_scale);
    let chunks = paginate_note_lines(&layout_lines, h_c - MARGIN * 2.4);
    let total_parts = chunks.len().max(1);

    let mut last_id = page_id;
    for (part_idx, chunk) in chunks.into_iter().enumerate() {
        let mut ops = vec![Operation::new("q", vec![]), op_cm(k, 0.0, 0.0, k, 0.0, 0.0)];
        fill_rect(&mut ops, 0.0, 0.0, w_total, h_c, pal.page_bg);
        ops.push(Operation::new("q", vec![]));
        ops.push(op_cm(s_c, 0.0, 0.0, s_c, 0.0, slide_y));
        ops.push(op_do("LpSlide"));
        ops.push(Operation::new("Q", vec![]));
        if ann_form.is_some() {
            ops.push(Operation::new("q", vec![]));
            ops.push(op_cm(s_c, 0.0, 0.0, s_c, 0.0, slide_y));
            ops.push(op_do("LpAnn"));
            ops.push(Operation::new("Q", vec![]));
        }
        fill_rect(&mut ops, sw_c - 0.3, 0.0, 0.6, h_c, pal.divider);

        let header_y = h_c - MARGIN - 11.0;
        let header_str = if total_parts > 1 {
            format!("Slide {} / {}  (Notes {}/{})", p_num, total_pages, part_idx + 1, total_parts)
        } else {
            format!("Slide {} / {}", p_num, total_pages)
        };
        push_styled_span(
            &mut ops,
            &header_str,
            text_x,
            header_y,
            8.5 * font_scale.clamp(0.85, 1.2),
            pal.muted,
            SpanStyle::default(),
            false,
        );
        let div_y = header_y - 6.0;
        fill_rect(&mut ops, text_x, div_y - 0.25, avail_text_w, 0.5, pal.divider);

        let mut cur_y = div_y - 14.0;
        for line in chunk {
            cur_y -= line.font_size;
            render_layout_line(&mut ops, &line, text_x, cur_y, avail_text_w, &pal);
            cur_y -= line.height - line.font_size;
        }
        ops.push(Operation::new("Q", vec![]));

        let bytes = Content { operations: ops }.encode()?;
        let content_id = pdf.add_object(Stream::new(dictionary! {}, bytes));
        let mut xobjects = dictionary! { "LpSlide" => slide_form };
        if let Some(a) = ann_form {
            xobjects.set("LpAnn", a);
        }
        let resources = dictionary! {
            "Font" => dictionary! { "F1" => fonts.0, "F2" => fonts.1, "F3" => fonts.2 },
            "XObject" => xobjects,
        };
        let page_dict = dictionary! {
            "Type" => "Page",
            "Parent" => parent_id,
            "MediaBox" => real_array(&[0.0, 0.0, w_total * k, h_c * k]),
            "Resources" => resources,
            "Contents" => content_id,
        };
        if part_idx == 0 {
            // Same object id: bookmarks and links that point to this page keep working.
            pdf.objects.insert(page_id, Object::Dictionary(page_dict));
        } else {
            let new_id = pdf.add_object(page_dict);
            insert_page_after(pdf, last_id, new_id)?;
            last_id = new_id;
        }
    }
    Ok(())
}

fn export_vector(
    source_pdf: &Path,
    output_pdf: &Path,
    notes: &HashMap<u32, String>,
    annotations: &HashMap<u32, Vec<PageAnnotation>>,
    settings: &ExportSettings,
    hidden_pages: &std::collections::HashSet<u32>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut pdf = crate::notes::load_clean_pdf(source_pdf)?;
    let pages: Vec<(u32, ObjectId)> = pdf.get_pages().into_iter().collect();
    if pages.is_empty() {
        return Err("the source PDF has no pages".into());
    }
    let include_ann = settings.content.includes_annotations();
    let include_notes = settings.content.includes_notes();

    for (n, id) in &pages {
        if hidden_pages.contains(n) {
            detach_page(&mut pdf, *id)?;
        }
    }

    let fonts = (
        embed_monospace_font(&mut pdf, "JetBrainsMono-Regular")?,
        embed_monospace_font(&mut pdf, "JetBrainsMono-Bold")?,
        embed_monospace_font(&mut pdf, "JetBrainsMono-Italic")?,
    );
    let ratio = (settings.slide_width_pct as f64 / 100.0).clamp(0.40, 0.80);
    let font_scale = (settings.base_font_size as f64 / 20.0).clamp(0.60, 1.65);
    let total_pages = pages.len() as u32;
    let media_dir = crate::audio::AudioDocIndex::media_dir_for_pdf(source_pdf).join("screenshots");
    let mut img_cache = HashMap::new();

    for (p_num, page_id) in pages {
        if hidden_pages.contains(&p_num) {
            continue;
        }
        let raw_note = if include_notes {
            notes.get(&p_num).map(|s| s.trim()).unwrap_or("")
        } else {
            ""
        };
        let page_anns: &[PageAnnotation] = if include_ann {
            annotations.get(&p_num).map(|v| v.as_slice()).unwrap_or(&[])
        } else {
            &[]
        };
        if raw_note.is_empty() && page_anns.is_empty() {
            continue; // untouched: identical to the original page
        }
        let geo = page_geo(&pdf, page_id).ok_or("cannot read the page size")?;
        let prepared = prepare_image_xobjects(&mut pdf, page_anns, &media_dir, &mut img_cache);

        if raw_note.is_empty() {
            let form = make_annotation_form(&mut pdf, page_anns, &prepared, fonts, geo.dw, geo.dh, 1.0)?;
            overlay_page(&mut pdf, page_id, &geo, form)?;
        } else {
            let slide_form = make_slide_form(&mut pdf, page_id, &geo)?;
            let s_c = CANVAS_H / geo.dh;
            let ann_form = if page_anns.is_empty() {
                None
            } else {
                Some(make_annotation_form(
                    &mut pdf,
                    page_anns,
                    &prepared,
                    fonts,
                    geo.dw * s_c,
                    geo.dh * s_c,
                    s_c,
                )?)
            };
            build_notes_pages(
                &mut pdf, page_id, &geo, slide_form, ann_form, fonts, p_num, total_pages, raw_note, ratio,
                font_scale,
            )?;
        }
    }

    pdf.prune_objects();
    pdf.compress();
    pdf.save(output_pdf)?;
    Ok(())
}

pub fn export_slides_with_notes(
    source_pdf: &Path,
    output_pdf: &Path,
    notes: &HashMap<u32, String>,
    annotations: &HashMap<u32, Vec<PageAnnotation>>,
    settings: &ExportSettings,
    hidden_pages: &std::collections::HashSet<u32>,
) -> Result<(), Box<dyn std::error::Error>> {
    match export_vector(source_pdf, output_pdf, notes, annotations, settings, hidden_pages) {
        Ok(()) => Ok(()),
        Err(e) => {
            eprintln!("Vector export failed ({e}); falling back to the rasterised export.");
            export_rasterized(source_pdf, output_pdf, notes, annotations, settings, hidden_pages)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ExportContent;

    fn sample_pdf(path: &Path) {
        let mut doc = LoDocument::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id =
            doc.add_object(dictionary! {"Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica"});
        let mut kids: Vec<Object> = Vec::new();
        // (width, height, /Rotate)
        for (i, (w, h, rot)) in [(612.0f32, 792.0f32, 0i64), (612.0, 792.0, 90), (400.0, 300.0, 0)]
            .into_iter()
            .enumerate()
        {
            let ops = vec![
                Operation::new("rg", vec![0.7.into(), 0.85.into(), 1.0.into()]),
                Operation::new("re", vec![40.into(), 40.into(), 200.into(), 100.into()]),
                Operation::new("f", vec![]),
                Operation::new("BT", vec![]),
                Operation::new("rg", vec![0.into(), 0.into(), 0.into()]),
                Operation::new("Tf", vec![Object::Name(b"F1".to_vec()), 24.into()]),
                Operation::new("Td", vec![50.into(), (h - 80.0).into()]),
                Operation::new("Tj", vec![Object::string_literal(format!("Page {}", i + 1))]),
                Operation::new("ET", vec![]),
            ];
            let content_id =
                doc.add_object(Stream::new(dictionary! {}, Content { operations: ops }.encode().unwrap()));
            let mut page = dictionary! {
                "Type" => "Page", "Parent" => pages_id,
                "MediaBox" => vec![0.into(), 0.into(), w.into(), h.into()],
                "Resources" => dictionary! {"Font" => dictionary! {"F1" => font_id}},
                "Contents" => content_id,
            };
            if rot != 0 {
                page.set("Rotate", rot);
            }
            kids.push(doc.add_object(page).into());
        }
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {"Type" => "Pages", "Kids" => kids, "Count" => 3}),
        );
        let cat = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages_id});
        doc.trailer.set("Root", cat);
        doc.save(path).unwrap();
    }

    fn page_sizes(path: &Path) -> Vec<(f32, f32)> {
        let d = MuDocument::open(path.to_str().unwrap()).unwrap();
        (0..d.page_count().unwrap())
            .map(|i| {
                let b = d.load_page(i).unwrap().bounds().unwrap();
                (b.x1 - b.x0, b.y1 - b.y0)
            })
            .collect()
    }

    /// (width, height, stride, RGB samples) at 1 px = 1 pt.
    fn render(path: &Path, index: i32) -> (usize, usize, usize, Vec<u8>) {
        let d = MuDocument::open(path.to_str().unwrap()).unwrap();
        let p = d.load_page(index).unwrap();
        let pix = p
            .to_pixmap(&Matrix::new_scale(1.0, 1.0), &Colorspace::device_rgb(), false, false)
            .unwrap();
        (pix.width() as usize, pix.height() as usize, pix.stride() as usize, pix.samples().to_vec())
    }

    fn px(img: &(usize, usize, usize, Vec<u8>), x: usize, y: usize) -> (u8, u8, u8) {
        let i = y * img.2 + x * 3;
        (img.3[i], img.3[i + 1], img.3[i + 2])
    }

    fn ann(kind: AnnotationKind, color: [u8; 3], pts: &[(f32, f32)], width: f32) -> PageAnnotation {
        PageAnnotation {
            kind,
            color,
            points: pts.to_vec(),
            text: String::new(),
            font_size: 18.0,
            bold: false,
            italic: false,
            stroke_width: width,
        }
    }

    #[test]
    fn vector_export_keeps_pages_sizes_widths_and_modes() {
        let _l = crate::docid::test_lock();
        let root = std::env::temp_dir().join(format!("lapis_export_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // SAFETY: tests are serialised by test_lock().
        unsafe {
            std::env::set_var("XDG_DATA_HOME", root.join("xdg"));
            std::env::set_var("XDG_CONFIG_HOME", root.join("cfg"));
        }
        let src = root.join("src.pdf");
        sample_pdf(&src);

        let mut notes = HashMap::new();
        notes.insert(2u32, "# Title\n\nSome **bold** note about page two.".to_string());
        let mut anns: HashMap<u32, Vec<PageAnnotation>> = HashMap::new();
        anns.insert(
            1,
            vec![
                ann(AnnotationKind::Freehand, [255, 0, 0], &[(0.1, 0.5), (0.9, 0.5)], 10.0),
                ann(AnnotationKind::Highlight, [255, 255, 0], &[(0.1, 0.2), (0.5, 0.25)], 2.0),
                ann(AnnotationKind::Tape, [0, 0, 255], &[(0.6, 0.6), (0.9, 0.7)], 2.0),
            ],
        );
        anns.insert(2, vec![ann(AnnotationKind::Rect, [0, 200, 0], &[(0.1, 0.1), (0.3, 0.2)], 6.0)]);
        let hidden = std::collections::HashSet::new();
        let run = |content: ExportContent, hidden: &std::collections::HashSet<u32>, notes: &HashMap<u32, String>| {
            let out = root.join(format!("out_{:?}.pdf", content));
            let cfg = ExportSettings { content, ..Default::default() };
            export_slides_with_notes(&src, &out, notes, &anns, &cfg, hidden).unwrap();
            out
        };

        // ── annotations + notes ──
        let out = run(ExportContent::AnnotationsAndNotes, &hidden, &notes);
        let sizes = page_sizes(&out);
        assert_eq!(sizes.len(), 3);
        assert_eq!(sizes[0], (612.0, 792.0), "page without notes keeps the original size");
        assert_eq!(sizes[2], (400.0, 300.0));
        // page 2 is /Rotate 90 -> displayed 792 x 612; the slide keeps its height and takes 63% of the width
        assert!((sizes[1].1 - 612.0).abs() < 0.6, "height = original slide height: {:?}", sizes[1]);
        assert!((792.0 / sizes[1].0 - 0.63).abs() < 0.01, "slide ratio: {:?}", sizes[1]);

        // untouched page 3 is pixel-identical to the original
        assert_eq!(render(&out, 2).3, render(&src, 2).3);

        // page 1: pen width = the width chosen in the app (10 pt), full-opacity colour
        let p1 = render(&out, 0);
        let red_rows = (380..412)
            .filter(|&y| {
                let (r, g, b) = px(&p1, 306, y);
                r > 200 && g < 80 && b < 80
            })
            .count();
        assert!((9..=12).contains(&red_rows), "pen thickness {red_rows}");
        // highlight: translucent (yellow tint over white, not opaque yellow)
        let (_, _, hb) = px(&p1, 183, 178);
        assert!((120..200).contains(&hb), "highlight alpha, blue channel = {hb}");
        // occlusion tape: (almost) opaque
        let (tr, _, tb) = px(&p1, 459, 515);
        assert!(tr < 25 && tb > 230, "tape should cover the page: {tr},{tb}");

        // page 2 composite: the slide is drawn at its original size (left part equals the original)
        let comp = render(&out, 1);
        let orig2 = render(&src, 1);
        let mut diff = 0;
        for y in 0..600 {
            for x in 0..700 {
                // the rect annotation (x 79..238, y 61..122, 6 pt wide) is drawn on the composite only
                if (70..250).contains(&x) && (50..135).contains(&y) {
                    continue;
                }
                if px(&comp, x, y) != px(&orig2, x, y) {
                    diff += 1;
                }
            }
        }
        assert!(diff < 50, "slide region differs from the original in {diff} px");

        // ── annotations only: notes are not exported, page 2 keeps its size and shows the annotation
        //    exactly where it belongs even though the page is rotated ──
        let out = run(ExportContent::Annotations, &hidden, &notes);
        let sizes = page_sizes(&out);
        assert_eq!(sizes[1], (792.0, 612.0));
        let r = render(&out, 1);
        let green = (55..68).any(|y| {
            let (rr, g, b) = px(&r, 158, y);
            rr < 60 && g > 150 && b < 60
        });
        assert!(green, "rect annotation on the rotated page");
        let o = render(&src, 1);
        assert!(!(55..68).any(|y| px(&o, 158, y).1 > 150 && px(&o, 158, y).0 < 60));

        // ── notes only: annotations are not drawn, page 1 stays identical ──
        let out = run(ExportContent::Notes, &hidden, &notes);
        assert_eq!(render(&out, 0).3, render(&src, 0).3);
        assert!((page_sizes(&out)[1].1 - 612.0).abs() < 0.6);

        // ── hidden pages are removed ──
        let mut hide = std::collections::HashSet::new();
        hide.insert(3u32);
        let out = run(ExportContent::AnnotationsAndNotes, &hide, &notes);
        assert_eq!(page_sizes(&out).len(), 2);

        // ── long notes continue on extra pages ──
        let mut long = HashMap::new();
        long.insert(2u32, (0..220).map(|i| format!("line number {i}")).collect::<Vec<_>>().join("\n\n"));
        let out = run(ExportContent::Notes, &hidden, &long);
        assert!(page_sizes(&out).len() > 3);

        // ── fallback (used for files lopdf cannot process) still produces a valid PDF ──
        let fb = root.join("fallback.pdf");
        let cfg = ExportSettings::default();
        export_rasterized(&src, &fb, &notes, &anns, &cfg, &hidden).unwrap();
        let fb_sizes = page_sizes(&fb);
        assert_eq!(fb_sizes.len(), 3);
        assert!(fb_sizes.iter().all(|s| *s == (960.0, 540.0)));

        let _ = std::fs::remove_dir_all(&root);
    }
}

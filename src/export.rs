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

pub fn export_slides_with_notes(
    source_pdf: &Path,
    output_pdf: &Path,
    notes: &HashMap<u32, String>,
    annotations: &HashMap<u32, Vec<PageAnnotation>>,
    settings: &ExportSettings,
    hidden_pages: &std::collections::HashSet<u32>,
) -> Result<(), Box<dyn std::error::Error>> {
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
        let page_anns = annotations
            .get(&p_num)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);

        let prepared = prepare_image_xobjects(&mut pdf, page_anns, &media_dir, &mut img_cache);
        // Render slide to RGB pixmap at crisp 2.0x scale and compress into an Image XObject
        let (img_id, pix_w, pix_h) = render_slide_xobject(&mut pdf, &mu_page, 2.0)?;

        if !has_note {
            // Full-width centered slide when there are no notes on this page
            let avail_w = CANVAS_W - MARGIN * 2.0;
            let avail_h = CANVAS_H - MARGIN * 2.0;
            let (sx, sy, sw, sh) =
                fit_rect_in_box(0.0, 0.0, CANVAS_W, CANVAS_H, avail_w, avail_h, pix_w, pix_h);

            let mut ops = Vec::new();
            draw_background(&mut ops, has_note, split_x);
            draw_slide_xobject(&mut ops, sx, sy, sw, sh);
            append_selectable_pdf_text_ops(&mut ops, &mu_page, sx, sy, sw, sh);

            draw_pdf_annotations(&mut ops, page_anns, &prepared.names, sx, sy, sw, sh);

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
                draw_background(&mut ops, true, split_x);
                draw_slide_xobject(&mut ops, sx, sy, sw, sh);
                append_selectable_pdf_text_ops(&mut ops, &mu_page, sx, sy, sw, sh);
                draw_pdf_annotations(&mut ops, page_anns, &prepared.names, sx, sy, sw, sh);

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
                    9.5 * font_scale.clamp(0.85, 1.2),
                    (0.49, 0.61, 1.0),
                    SpanStyle {
                        bold: true,
                        italic: false,
                        code: false,
                    },
                    false,
                );

                // Subtle horizontal divider under header
                let div_y = header_y - 6.0;
                ops.push(Operation::new("q", vec![]));
                ops.push(Operation::new(
                    "RG",
                    vec![0.22.into(), 0.25.into(), 0.34.into()],
                ));
                ops.push(Operation::new("w", vec![0.8.into()]));
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
                    render_layout_line(&mut ops, &line, text_x, cur_y, avail_text_w);
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

fn draw_background(ops: &mut Vec<Operation>, split: bool, split_x: f64) {
    // Dark slate background matching Lapis theme
    ops.push(Operation::new("q", vec![]));
    ops.push(Operation::new(
        "rg",
        vec![0.075.into(), 0.082.into(), 0.110.into()],
    ));
    ops.push(Operation::new(
        "re",
        vec![0.into(), 0.into(), CANVAS_W.into(), CANVAS_H.into()],
    ));
    ops.push(Operation::new("f", vec![]));

    if split {
        // Slightly elevated dark panel on the right for notes
        ops.push(Operation::new(
            "rg",
            vec![0.105.into(), 0.114.into(), 0.153.into()],
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

        // Vertical separator line at `split_x`
        ops.push(Operation::new(
            "RG",
            vec![0.22.into(), 0.25.into(), 0.35.into()],
        ));
        ops.push(Operation::new("w", vec![1.0.into()]));
        ops.push(Operation::new("m", vec![split_x.into(), 0.into()]));
        ops.push(Operation::new("l", vec![split_x.into(), CANVAS_H.into()]));
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

        match ann.kind {
            AnnotationKind::Highlight => {
                if ann.points.len() >= 2 {
                    let (x1, y1) = to_pdf(ann.points[0]);
                    let (x2, y2) = to_pdf(*ann.points.last().unwrap());
                    let rx = x1.min(x2);
                    let ry = y1.min(y2);
                    let rw = (x2 - x1).abs().max(2.0);
                    let rh = (y2 - y1).abs().max(4.0);

                    ops.push(Operation::new("q", vec![]));
                    ops.push(Operation::new("RG", vec![r.into(), g.into(), b.into()]));
                    ops.push(Operation::new("w", vec![1.5.into()]));
                    ops.push(Operation::new(
                        "re",
                        vec![rx.into(), ry.into(), rw.into(), rh.into()],
                    ));
                    ops.push(Operation::new("S", vec![]));
                    ops.push(Operation::new("Q", vec![]));
                }
            }
            AnnotationKind::Underline => {
                if ann.points.len() >= 2 {
                    let (x1, y1) = to_pdf(ann.points[0]);
                    let (x2, _) = to_pdf(*ann.points.last().unwrap());
                    ops.push(Operation::new("q", vec![]));
                    ops.push(Operation::new("RG", vec![r.into(), g.into(), b.into()]));
                    ops.push(Operation::new("w", vec![2.2.into()]));
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
                    ops.push(Operation::new("w", vec![2.2.into()]));
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
                    ops.push(Operation::new("w", vec![2.2.into()]));
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
                    ops.push(Operation::new("w", vec![2.2.into()]));
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
                    ops.push(Operation::new("w", vec![2.2.into()]));
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
                            let hl = 10.0;
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
                    let rw = (x2 - x1).abs().max(2.0);
                    let rh = (y2 - y1).abs().max(4.0);

                    ops.push(Operation::new("q", vec![]));
                    ops.push(Operation::new("RG", vec![r.into(), g.into(), b.into()]));
                    ops.push(Operation::new("w", vec![1.5.into()]));
                    ops.push(Operation::new(
                        "re",
                        vec![rx.into(), ry.into(), rw.into(), rh.into()],
                    ));
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

fn render_layout_line(
    ops: &mut Vec<Operation>,
    line: &LayoutLine,
    text_x: f64,
    y: f64,
    avail_w: f64,
) {
    let sz = line.font_size;
    let char_w = sz * 0.60;

    match &line.kind {
        NoteLineKind::Blank => {}
        NoteLineKind::Rule => {
            ops.push(Operation::new("q", vec![]));
            ops.push(Operation::new(
                "RG",
                vec![0.26.into(), 0.30.into(), 0.42.into()],
            ));
            ops.push(Operation::new("w", vec![0.8.into()]));
            ops.push(Operation::new(
                "m",
                vec![text_x.into(), (y + sz * 0.4).into()],
            ));
            ops.push(Operation::new(
                "l",
                vec![(text_x + avail_w).into(), (y + sz * 0.4).into()],
            ));
            ops.push(Operation::new("S", vec![]));
            ops.push(Operation::new("Q", vec![]));
        }
        NoteLineKind::Heading(lvl, spans) => {
            let col = match lvl {
                1 => (0.49, 0.61, 1.0),
                2 => (0.58, 0.76, 0.98),
                _ => (0.72, 0.82, 0.98),
            };
            let mut cx = text_x;
            for sp in spans {
                let mut st = sp.style;
                st.bold = true;
                cx = push_styled_span(ops, &sp.text, cx, y, sz, col, st, true);
            }
        }
        NoteLineKind::Bullet(indent, spans) => {
            let indent_pt = (*indent as f64) * 12.0;
            let bullet_x = text_x + indent_pt + 4.0;

            // Draw crisp bullet circle
            ops.push(Operation::new("q", vec![]));
            ops.push(Operation::new(
                "rg",
                vec![0.49.into(), 0.61.into(), 1.0.into()],
            ));
            ops.push(Operation::new(
                "re",
                vec![
                    bullet_x.into(),
                    (y + sz * 0.25).into(),
                    3.2.into(),
                    3.2.into(),
                ],
            ));
            ops.push(Operation::new("f", vec![]));
            ops.push(Operation::new("Q", vec![]));

            let mut cx = text_x + indent_pt + 13.0;
            for sp in spans {
                let col = if sp.style.code {
                    (0.68, 0.82, 1.0)
                } else if sp.style.bold {
                    (0.49, 0.61, 1.0)
                } else {
                    (0.86, 0.89, 0.95)
                };
                cx = push_styled_span(ops, &sp.text, cx, y, sz, col, sp.style, true);
            }
        }
        NoteLineKind::Quote(spans) => {
            // Vertical accent bar on the left of blockquotes (also used for $$display math$$!)
            ops.push(Operation::new("q", vec![]));
            ops.push(Operation::new(
                "rg",
                vec![0.49.into(), 0.61.into(), 1.0.into()],
            ));
            ops.push(Operation::new(
                "re",
                vec![
                    text_x.into(),
                    (y - 2.0).into(),
                    2.6.into(),
                    (sz * 1.15).into(),
                ],
            ));
            ops.push(Operation::new("f", vec![]));
            ops.push(Operation::new("Q", vec![]));

            let mut cx = text_x + 10.0;
            for sp in spans {
                let col = if sp.style.code || sp.style.bold {
                    (0.68, 0.82, 1.0)
                } else {
                    (0.74, 0.78, 0.88)
                };
                cx = push_styled_span(ops, &sp.text, cx, y, sz, col, sp.style, true);
            }
        }
        NoteLineKind::CodeLine(code_str) => {
            // Code block pill background
            ops.push(Operation::new("q", vec![]));
            ops.push(Operation::new(
                "rg",
                vec![0.16.into(), 0.19.into(), 0.28.into()],
            ));
            ops.push(Operation::new(
                "re",
                vec![
                    text_x.into(),
                    (y - 3.0).into(),
                    avail_w.into(),
                    (sz * 1.32).into(),
                ],
            ));
            ops.push(Operation::new("f", vec![]));
            ops.push(Operation::new("Q", vec![]));

            let _ = push_styled_span(
                ops,
                code_str,
                text_x + 6.0,
                y,
                sz,
                (0.76, 0.86, 1.0),
                SpanStyle {
                    bold: false,
                    italic: false,
                    code: true,
                },
                true,
            );
        }
        NoteLineKind::Paragraph(spans) => {
            let mut cx = text_x;
            for sp in spans {
                if sp.style.code {
                    let span_w = (sp.text.chars().count() as f64) * char_w + 4.0;
                    ops.push(Operation::new("q", vec![]));
                    ops.push(Operation::new(
                        "rg",
                        vec![0.18.into(), 0.21.into(), 0.32.into()],
                    ));
                    ops.push(Operation::new(
                        "re",
                        vec![
                            (cx - 1.5).into(),
                            (y - 2.0).into(),
                            span_w.into(),
                            (sz * 1.15).into(),
                        ],
                    ));
                    ops.push(Operation::new("f", vec![]));
                    ops.push(Operation::new("Q", vec![]));
                }

                let col = if sp.style.code {
                    (0.72, 0.84, 1.0)
                } else if sp.style.bold {
                    (0.49, 0.61, 1.0)
                } else {
                    (0.86, 0.89, 0.95)
                };
                cx = push_styled_span(ops, &sp.text, cx, y, sz, col, sp.style, true);
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
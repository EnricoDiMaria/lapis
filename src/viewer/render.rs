//! MuPDF rasterization and the document-owned page and thumbnail workers.

use super::text::{PageText, extract_text};
use eframe::egui;
use mupdf::{Colorspace, Document, Matrix};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

const SCALE_STEP: f32 = 0.25;
const MIN_RENDER_SCALE: f32 = 0.5;
const MAX_RENDER_SCALE: f32 = 4.0;
const MAX_PAGE_PIXELS: f32 = 12_000_000.0;
const MAX_TEXTURE_SIDE: f32 = 8192.0;
const MAX_THUMBNAIL_SIDE: f32 = 512.0;

/// MuPDF scale for the on-screen width, quantized upwards and capped by texture limits.
pub(crate) fn render_scale(
    (pw, ph): (f32, f32),
    display_width: f32,
    pixels_per_point: f32,
    presenting: bool,
) -> f32 {
    if pw <= 0.0 || ph <= 0.0 || display_width <= 0.0 {
        return 1.5;
    }
    let ideal = display_width * pixels_per_point / pw;
    let cap = (MAX_TEXTURE_SIDE / pw.max(ph)).min((MAX_PAGE_PIXELS / (pw * ph)).sqrt());
    let cap = if presenting {
        cap
    } else {
        cap.min(MAX_RENDER_SCALE)
    };
    let quantized = (ideal / SCALE_STEP).ceil() * SCALE_STEP;
    quantized.max(MIN_RENDER_SCALE).min(cap)
}

pub(super) fn scale_key(scale: f32) -> u32 {
    (scale * 100.0).round() as u32
}

/// Pages to render (0-based) in priority order. A newer request always REPLACES an older
/// one (latest-wins), so the UI just describes what it needs right now.
pub(super) struct RenderRequest {
    pub(super) scale: f32,
    pub(super) pages: Vec<i32>,
    pub(super) dark_mode: bool,
}

pub(super) struct RenderResult {
    pub(super) page_idx: i32,
    /// `scale * 100`, rounded: tells the UI at which resolution this texture was rendered.
    pub(super) scale_key: u32,
    /// Page size in PDF points (used to pick the render scale for the display size).
    pub(super) page_pt: (f32, f32),
    /// Already converted to Color32 on the worker thread.
    pub(super) image: egui::ColorImage,
    pub(super) is_thumb: bool,
    pub(super) dark_mode: bool,
    pub(super) links: Vec<PdfLinkZone>,
}

/// Messages from the workers to the UI.
pub(super) enum WorkerMsg {
    Page(RenderResult),
    /// Selectable text of a page. Sent once per page, right AFTER its first image, so
    /// text extraction never delays the first paint.
    Text {
        page_idx: i32,
        text: Arc<PageText>,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct PdfLinkZone {
    /// Normalized bounding box `[x0, y0, x1, y1]` in `0.0..=1.0`
    pub(crate) rect: [f32; 4],
    /// Target 1-based page number if internal link, or external URL
    pub(crate) target_page: Option<u32>,
    pub(crate) external_uri: Option<String>,
}

fn extract_page_links(doc: &Document, page: &mupdf::Page, pw: f32, ph: f32) -> Vec<PdfLinkZone> {
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

pub(crate) fn render_to_image(
    doc: &Document,
    idx: i32,
    scale: f32,
    dark_mode: bool,
) -> Option<((f32, f32), egui::ColorImage, Vec<PdfLinkZone>)> {
    render_page(doc, idx, scale, dark_mode, false)
}

fn render_page(
    doc: &Document,
    idx: i32,
    scale: f32,
    dark_mode: bool,
    thumbnail: bool,
) -> Option<((f32, f32), egui::ColorImage, Vec<PdfLinkZone>)> {
    let page = doc.load_page(idx).ok()?;
    let b = page.bounds().ok()?;
    let pw = b.x1 - b.x0;
    let ph = b.y1 - b.y0;
    let scale = if thumbnail {
        scale.min(MAX_THUMBNAIL_SIDE / pw.max(ph))
    } else {
        scale
    };

    let pix = page
        .to_pixmap(
            &Matrix::new_scale(scale, scale),
            &Colorspace::device_rgb(),
            false,
            false, // show_extras=false: do not paint PDF annotation appearances; Lapis draws its own layer.
        )
        .ok()?;
    let size = [pix.width() as usize, pix.height() as usize];
    let samples = pix.samples();
    let image = if dark_mode {
        let mut dark_rgb = Vec::with_capacity(samples.len());
        for chunk in samples.as_chunks::<3>().0 {
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
                if let Some((page_pt, image, links)) =
                    render_page(&doc, idx, req.scale, req.dark_mode, is_thumb)
                {
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
                if !is_thumb
                    && text_done.insert(idx)
                    && let Some(text) = extract_text(&doc, idx)
                {
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
    });

    req_tx
}

/// The request and result channels belong to one open PDF.
pub(super) struct RenderWorkers {
    pub(super) page_requests: Sender<RenderRequest>,
    pub(super) thumbnail_requests: Sender<RenderRequest>,
    pub(super) results: Receiver<WorkerMsg>,
}

impl RenderWorkers {
    pub(super) fn start(path: PathBuf, ctx: egui::Context) -> Self {
        let (results_tx, results) = channel();
        let page_requests = spawn_worker(path.clone(), results_tx.clone(), ctx.clone(), false);
        let thumbnail_requests = spawn_worker(path, results_tx, ctx, true);
        Self {
            page_requests,
            thumbnail_requests,
            results,
        }
    }

    pub(super) fn try_result(&self) -> Option<WorkerMsg> {
        self.results.try_recv().ok()
    }
}

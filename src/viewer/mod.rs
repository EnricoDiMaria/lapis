//! Rendering requests and cached page results for the active PDF.

mod document;
mod render;
mod text;

use eframe::egui;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

pub(crate) use document::{TocItem, inspect_pdf, outline};
pub(crate) use render::{PdfLinkZone, render_scale, render_to_image};
pub(crate) use text::{PageText, extract_text};

#[cfg(test)]
use render::RenderResult;
use render::{RenderRequest, RenderWorkers, WorkerMsg, scale_key};

const THUMB_SCALE: f32 = 0.22;
const PAGE_CACHE_BYTES: usize = 384 * 1024 * 1024;
const IMAGES_PER_FRAME: usize = 2;

#[derive(Clone, Copy)]
pub(crate) struct RenderView {
    pub(crate) current_page: u32,
    pub(crate) total_pages: u32,
    pub(crate) continuous_visible_pages: Option<(u32, u32)>,
    pub(crate) split_page: Option<u32>,
}

impl RenderView {
    /// Pages in priority order, including the adjacent pages to prefetch.
    fn wanted_pages(self) -> Vec<u32> {
        let mut pages = Vec::with_capacity(12);
        let mut push = |page: i64| {
            if page >= 1 && page <= i64::from(self.total_pages) && !pages.contains(&(page as u32)) {
                pages.push(page as u32);
            }
        };
        let current = i64::from(self.current_page);
        if let Some((first, last)) = self.continuous_visible_pages
            && first >= 1
            && last >= first
        {
            push(current);
            for page in first..=last {
                push(i64::from(page));
            }
            push(i64::from(last) + 1);
            push(i64::from(first) - 1);
            push(i64::from(last) + 2);
        } else {
            for offset in [0, 1, -1, 2] {
                push(current + offset);
            }
        }
        if let Some(page) = self.split_page {
            push(i64::from(page));
            push(i64::from(page) + 1);
        }
        pages
    }
}

type PageTexture = (egui::TextureHandle, (u32, u32));

struct RenderedPage {
    texture: PageTexture,
    key: u32,
}

pub(crate) struct CurrentPageRender {
    pub(crate) texture: PageTexture,
    pub(crate) page_pt: (f32, f32),
}

/// Owns the active document's workers, image and text results, and request history.
#[derive(Default)]
pub(crate) struct Viewer {
    workers: Option<RenderWorkers>,
    pages: HashMap<u32, RenderedPage>,
    thumbnails: HashMap<u32, PageTexture>,
    cache_order: VecDeque<u32>,
    text: HashMap<u32, Arc<PageText>>,
    links: HashMap<u32, Vec<PdfLinkZone>>,
    last_page_request: Option<(u32, Vec<u32>)>,
    last_thumbnail_range: Option<(usize, usize)>,
}

impl Viewer {
    pub(crate) fn start(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.close();
        self.workers = Some(RenderWorkers::start(path, ctx.clone()));
    }

    pub(crate) fn stop_workers(&mut self) {
        // Disconnect requests and discard any pending results from this document.
        self.workers = None;
    }

    pub(crate) fn close(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn page(&self, page: u32) -> Option<&PageTexture> {
        self.pages.get(&page).map(|entry| &entry.texture)
    }

    pub(crate) fn thumbnail(&self, page: u32) -> Option<&PageTexture> {
        self.thumbnails.get(&page)
    }

    pub(crate) fn page_or_thumbnail(&self, page: u32) -> Option<&PageTexture> {
        self.page(page).or_else(|| self.thumbnail(page))
    }

    pub(crate) fn text(&self, page: u32) -> Option<&PageText> {
        self.text.get(&page).map(Arc::as_ref)
    }

    pub(crate) fn links(&self, page: u32) -> Option<&[PdfLinkZone]> {
        self.links.get(&page).map(Vec::as_slice)
    }

    pub(crate) fn clear_textures(&mut self) {
        self.pages.clear();
        self.thumbnails.clear();
    }

    pub(crate) fn invalidate_images(&mut self) {
        self.clear_textures();
        self.last_page_request = None;
        self.last_thumbnail_range = None;
    }

    pub(crate) fn touch_page(&mut self, page: u32) {
        self.cache_order.retain(|&cached| cached != page);
        self.cache_order.push_back(page);
    }

    fn evict_pages(&mut self, view: RenderView, budget: usize) {
        let protected = view.wanted_pages();
        let mut bytes: usize = self
            .pages
            .values()
            .map(|entry| {
                let (_, (w, h)) = &entry.texture;
                *w as usize * *h as usize * 4
            })
            .sum();
        let mut i = 0;
        while bytes > budget && i < self.cache_order.len() {
            let page = self.cache_order[i];
            if protected.contains(&page) {
                i += 1;
                continue;
            }
            self.cache_order.remove(i);
            if let Some(entry) = self.pages.remove(&page) {
                let (_, (w, h)) = entry.texture;
                bytes = bytes.saturating_sub(w as usize * h as usize * 4);
            }
        }
    }

    /// Match the color mode and keep at most 2x the requested scale for linear filtering.
    fn page_is_adequate(&self, page: u32, key: u32) -> bool {
        self.pages.get(&page).is_some_and(|entry| {
            let cached = entry.key;
            cached % 2 == key % 2 && cached >= key && cached <= key * 2
        })
    }

    pub(crate) fn request_pages(&mut self, view: RenderView, scale: f32, dark_mode: bool) {
        let key = scale_key(scale) * 2 + u32::from(dark_mode);
        let wanted = view.wanted_pages();
        if self
            .last_page_request
            .as_ref()
            .is_some_and(|(last_key, last_pages)| *last_key == key && *last_pages == wanted)
        {
            return;
        }
        let pages: Vec<i32> = wanted
            .iter()
            .copied()
            .filter(|&page| !self.page_is_adequate(page, key))
            .map(|page| (page - 1) as i32)
            .collect();
        self.last_page_request = Some((key, wanted));
        if !pages.is_empty()
            && let Some(workers) = &self.workers
        {
            let _ = workers.page_requests.send(RenderRequest {
                scale,
                pages,
                dark_mode,
            });
        }
    }

    /// Request the visible sidebar rows plus a margin, starting near the center.
    pub(crate) fn request_thumbnails(
        &mut self,
        total_pages: u32,
        first_row: usize,
        end_row: usize,
        dark_mode: bool,
    ) {
        let lo = first_row.saturating_sub(3);
        let hi = (end_row + 3).min(total_pages as usize);
        if self.last_thumbnail_range == Some((lo, hi)) {
            return;
        }
        self.last_thumbnail_range = Some((lo, hi));
        let mid = ((first_row + end_row) / 2) as i32;
        let mut pages: Vec<i32> = (lo..hi)
            .filter(|i| !self.thumbnails.contains_key(&((*i + 1) as u32)))
            .map(|i| i as i32)
            .collect();
        if !pages.is_empty()
            && let Some(workers) = &self.workers
        {
            pages.sort_by_key(|&i| (i - mid).abs());
            let _ = workers.thumbnail_requests.send(RenderRequest {
                scale: THUMB_SCALE,
                pages,
                dark_mode,
            });
        }
    }

    /// Process results on the UI thread and hand back the latest image for the active page.
    pub(crate) fn upload_ready(
        &mut self,
        ctx: &egui::Context,
        view: RenderView,
    ) -> Option<CurrentPageRender> {
        let mut current = None;
        let mut images = 0;
        while images < IMAGES_PER_FRAME {
            let Some(msg) = self.workers.as_ref().and_then(RenderWorkers::try_result) else {
                break;
            };
            let result = match msg {
                WorkerMsg::Text { page_idx, text } => {
                    self.text.insert((page_idx + 1) as u32, text);
                    continue;
                }
                WorkerMsg::Page(result) => result,
            };
            images += 1;
            let page = (result.page_idx + 1) as u32;
            let [w, h] = result.image.size;
            let dims = (w as u32, h as u32);
            self.links.insert(page, result.links);

            if result.is_thumb {
                let handle = ctx.load_texture(
                    format!("pdf_thumb_{page}"),
                    result.image,
                    egui::TextureOptions::LINEAR,
                );
                self.thumbnails.insert(page, (handle, dims));
                continue;
            }

            let result_key = result.scale_key * 2 + u32::from(result.dark_mode);
            // A superseded request may finish after the viewport changes.
            if let Some((key, wanted)) = &self.last_page_request
                && wanted.contains(&page)
                && result_key != *key
                && self.page_is_adequate(page, *key)
            {
                continue;
            }
            let handle = ctx.load_texture(
                format!("pdf_page_{page}"),
                result.image,
                egui::TextureOptions::LINEAR,
            );
            if page == view.current_page {
                current = Some(CurrentPageRender {
                    texture: (handle.clone(), dims),
                    page_pt: result.page_pt,
                });
            }
            self.pages.insert(
                page,
                RenderedPage {
                    texture: (handle, dims),
                    key: result_key,
                },
            );
            self.touch_page(page);
            self.evict_pages(view, PAGE_CACHE_BYTES);
        }
        if images == IMAGES_PER_FRAME {
            ctx.request_repaint();
        }
        current
    }
}

#[cfg(test)]
mod tests;

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
const THUMBNAIL_CACHE_BYTES: usize = 32 * 1024 * 1024;
const IMAGES_PER_FRAME: usize = 2;

#[derive(Clone, Copy, PartialEq, Eq)]
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

struct CachedTexture {
    texture: PageTexture,
    key: u32,
}

fn texture_bytes((w, h): (u32, u32)) -> usize {
    w as usize * h as usize * 4
}

#[derive(Default)]
struct TextureCache {
    entries: HashMap<u32, CachedTexture>,
    order: VecDeque<u32>,
    bytes: usize,
}

impl TextureCache {
    fn touch(&mut self, page: u32) {
        self.order.retain(|&cached| cached != page);
        self.order.push_back(page);
    }

    fn insert(&mut self, page: u32, texture: PageTexture, key: u32) {
        self.bytes += texture_bytes(texture.1);
        if let Some(previous) = self.entries.insert(page, CachedTexture { texture, key }) {
            self.bytes -= texture_bytes(previous.texture.1);
        }
        self.touch(page);
    }

    fn evict(&mut self, budget: usize, protected: impl Fn(u32) -> bool) {
        let mut i = 0;
        while self.bytes > budget && i < self.order.len() {
            let page = self.order[i];
            if protected(page) {
                i += 1;
                continue;
            }
            self.order.remove(i);
            if let Some(entry) = self.entries.remove(&page) {
                self.bytes -= texture_bytes(entry.texture.1);
            }
        }
    }
}

struct PageRequest {
    view: RenderView,
    key: u32,
    wanted: Vec<u32>,
}

pub(crate) struct CurrentPageRender {
    pub(crate) texture: PageTexture,
    pub(crate) page_pt: (f32, f32),
}

/// Owns the active document's workers, image and text results, and request history.
#[derive(Default)]
pub(crate) struct Viewer {
    workers: Option<RenderWorkers>,
    pages: TextureCache,
    thumbnails: TextureCache,
    text: HashMap<u32, Arc<PageText>>,
    links: HashMap<u32, Vec<PdfLinkZone>>,
    last_page_request: Option<PageRequest>,
    last_thumbnail_request: Option<(usize, usize, bool)>,
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
        self.pages.entries.get(&page).map(|entry| &entry.texture)
    }

    pub(crate) fn thumbnail(&self, page: u32) -> Option<&PageTexture> {
        self.thumbnails
            .entries
            .get(&page)
            .map(|entry| &entry.texture)
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

    pub(crate) fn invalidate_images(&mut self) {
        self.pages = TextureCache::default();
        self.thumbnails = TextureCache::default();
        self.last_page_request = None;
        self.last_thumbnail_request = None;
    }

    pub(crate) fn touch_page(&mut self, page: u32) {
        self.pages.touch(page);
    }

    fn evict_pages(&mut self, view: RenderView, budget: usize) {
        if self.pages.bytes > budget {
            if let Some(last) = &self.last_page_request
                && last.view == view
            {
                self.pages.evict(budget, |page| last.wanted.contains(&page));
            } else {
                let protected = view.wanted_pages();
                self.pages.evict(budget, |page| protected.contains(&page));
                // A page from the previous request may have been evicted.
                self.last_page_request = None;
            }
        }
    }

    fn evict_thumbnails(&mut self, view: RenderView, budget: usize) {
        if self.thumbnails.bytes <= budget {
            return;
        }
        let sidebar = self.last_thumbnail_request;
        self.thumbnails.evict(budget, |page| {
            page == view.current_page
                || Some(page) == view.split_page
                || sidebar.is_some_and(|(lo, hi, _)| (lo..hi).contains(&((page - 1) as usize)))
        });
        // A large sidebar range can itself exceed the budget. Keep the active page's fallback.
        self.thumbnails
            .evict(budget, |page| page == view.current_page);
    }

    /// Match the color mode and keep at most 2x the requested scale for linear filtering.
    fn page_is_adequate(&self, page: u32, key: u32) -> bool {
        self.pages.entries.get(&page).is_some_and(|entry| {
            let cached = entry.key;
            cached % 2 == key % 2 && cached >= key && cached <= key * 2
        })
    }

    pub(crate) fn request_pages(&mut self, view: RenderView, scale: f32, dark_mode: bool) {
        let key = scale_key(scale) * 2 + u32::from(dark_mode);
        if self
            .last_page_request
            .as_ref()
            .is_some_and(|last| last.key == key && last.view == view)
        {
            return;
        }
        let wanted = view.wanted_pages();
        if self.pages.bytes > PAGE_CACHE_BYTES {
            self.pages
                .evict(PAGE_CACHE_BYTES, |page| wanted.contains(&page));
        }
        if let Some(last) = &mut self.last_page_request
            && last.key == key
            && last.wanted == wanted
        {
            last.view = view;
            return;
        }
        let pages: Vec<i32> = wanted
            .iter()
            .copied()
            .filter(|&page| !self.page_is_adequate(page, key))
            .map(|page| (page - 1) as i32)
            .collect();
        self.last_page_request = Some(PageRequest { view, key, wanted });
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
        if self.last_thumbnail_request == Some((lo, hi, dark_mode)) {
            return;
        }
        self.last_thumbnail_request = Some((lo, hi, dark_mode));
        let mid = ((first_row + end_row) / 2) as i32;
        let mut pages = Vec::new();
        for index in lo..hi {
            let page = (index + 1) as u32;
            if self
                .thumbnails
                .entries
                .get(&page)
                .is_some_and(|entry| entry.key == u32::from(dark_mode))
            {
                self.thumbnails.touch(page);
            } else {
                pages.push(index as i32);
            }
        }
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
        dark_mode: bool,
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
                if result.dark_mode != dark_mode || texture_bytes(dims) > THUMBNAIL_CACHE_BYTES {
                    continue;
                }
                let handle = ctx.load_texture(
                    format!("pdf_thumb_{page}"),
                    result.image,
                    egui::TextureOptions::LINEAR,
                );
                self.thumbnails
                    .insert(page, (handle, dims), u32::from(result.dark_mode));
                self.evict_thumbnails(view, THUMBNAIL_CACHE_BYTES);
                continue;
            }

            let result_key = result.scale_key * 2 + u32::from(result.dark_mode);
            // A superseded request may finish after the viewport changes.
            if let Some(last) = &self.last_page_request
                && last.wanted.contains(&page)
                && result_key != last.key
                && self.page_is_adequate(page, last.key)
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
            self.pages.insert(page, (handle, dims), result_key);
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

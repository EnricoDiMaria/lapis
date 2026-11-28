//! Page, zoom, and scroll navigation across viewing modes.

use super::*;

/// Keep the content point under `anchor` fixed during zoom.

pub(super) fn zoom_scroll_offset(
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
pub(super) fn clamp_scroll_offset(
    off: egui::Vec2,
    content: egui::Vec2,
    view: egui::Vec2,
) -> egui::Vec2 {
    let max = (content - view).max(egui::Vec2::ZERO);
    egui::vec2(off.x.clamp(0.0, max.x), off.y.clamp(0.0, max.y))
}

#[derive(Clone, Copy, Default)]
pub(super) struct SinglePageWheel {
    accumulated: f32,
    last_event: Option<f64>,
    pub(super) reset_offset: bool,
}

impl SinglePageWheel {
    pub(super) const POINTS_PER_PAGE: f32 = 48.0;

    pub(super) fn update(&mut self, time: f64, delta: f32, offset: f32, max_offset: f32) -> i32 {
        if !delta.is_finite() {
            self.accumulated = 0.0;
            return 0;
        }
        if delta == 0.0 {
            return 0;
        }
        let at_edge = (delta > 0.0 && offset <= 1.0) || (delta < 0.0 && offset >= max_offset - 1.0);
        if !at_edge {
            self.accumulated = 0.0;
            self.last_event = None;
            return 0;
        }
        if self.last_event.is_none_or(|t| time - t > 0.3)
            || self.accumulated.signum() != delta.signum()
        {
            self.accumulated = 0.0;
        }
        self.last_event = Some(time);
        self.accumulated += delta;
        let pages = (self.accumulated / Self::POINTS_PER_PAGE).trunc();
        self.accumulated %= Self::POINTS_PER_PAGE;
        (-pages) as i32
    }
}

pub(super) fn wheel_page_target(
    current: u32,
    total: u32,
    hidden: &HashSet<u32>,
    steps: i32,
) -> u32 {
    let mut target = current;
    let mut page = current;
    let mut remaining = steps.unsigned_abs();
    while remaining > 0 {
        let Some(next) = page.checked_add_signed(steps.signum()) else {
            break;
        };
        if next == 0 || next > total {
            break;
        }
        page = next;
        if !hidden.contains(&page) {
            target = page;
            remaining -= 1;
        }
    }
    target
}

impl Lapis {
    /// Mode to remember: slideshows are transient, so they store the mode they came from.
    pub(super) fn reading_view_mode(&self) -> ViewMode {
        match self.view_mode {
            ViewMode::Slideshow | ViewMode::SlideshowWithNotes => self.prev_view_mode,
            mode => mode,
        }
    }

    /// Wheel / trackpad scroll multiplier shared by every page view.
    pub(super) fn wheel_speed(&self) -> f32 {
        self.app_state.settings.wheel_scroll_speed.clamp(0.2, 8.0)
    }

    /// Middle-button drag multiplier shared by every page view.
    pub(super) fn pan_speed(&self) -> f32 {
        self.app_state.settings.pan_scroll_speed.clamp(0.2, 12.0)
    }

    /// Scales this frame's wheel scrolling over `rect` (Ctrl + wheel stays the zoom).
    pub(super) fn apply_wheel_speed(&self, ui: &egui::Ui, rect: egui::Rect) {
        if !ui.rect_contains_pointer(rect) {
            return;
        }
        let speed = self.wheel_speed();
        ui.input_mut(|i| {
            if !i.modifiers.ctrl && !i.modifiers.command {
                i.smooth_scroll_delta *= speed;
            }
        });
    }

    /// Middle-button drag panning for a scroll area outside the main canvas: the scroll delta
    /// for this frame. The drag starts inside `rect` and continues wherever the pointer goes.
    pub(super) fn middle_drag_pan(&self, ui: &egui::Ui, key: &str, rect: egui::Rect) -> egui::Vec2 {
        let id = ui.make_persistent_id(("middle_drag_pan", key));
        let (pressed, down, delta, pos) = ui.input(|i| {
            (
                i.pointer.button_pressed(egui::PointerButton::Middle),
                i.pointer.button_down(egui::PointerButton::Middle),
                i.pointer.delta(),
                i.pointer.hover_pos().or(i.pointer.latest_pos()),
            )
        });
        let mut active = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
        if pressed && pos.is_some_and(|p| rect.contains(p)) {
            active = true;
        }
        if !down {
            active = false;
        }
        ui.data_mut(|d| d.insert_temp(id, active));
        if !active {
            return egui::Vec2::ZERO;
        }
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        ui.ctx().request_repaint();
        delta * self.pan_speed()
    }

    /// Switches view mode. Single <-> Continuous keeps the vertical reading position.
    pub(super) fn set_view_mode(&mut self, mode: ViewMode) {
        let off = self.pdf_scroll_offset;
        self.mode_switch_offset = None;
        self.single_page_wheel = SinglePageWheel::default();
        match (self.view_mode, mode) {
            (ViewMode::SinglePage, ViewMode::Continuous) => {
                self.continuous_scroll_to_page = Some(self.current_page);
                self.mode_switch_offset = Some(egui::vec2(off.x, off.y.max(0.0)));
            }
            (ViewMode::Continuous, ViewMode::SinglePage) => {
                let row = (1..self.current_page)
                    .filter(|p| !self.hidden_pages.contains(p))
                    .count() as f32;
                let pitch = self.last_layout.1 + CONTINUOUS_GAP;
                self.mode_switch_offset = Some(egui::vec2(off.x, (off.y - row * pitch).max(0.0)));
            }
            (_, ViewMode::Continuous) => self.continuous_scroll_to_page = Some(self.current_page),
            _ => {}
        }
        self.view_mode = mode;
    }

    pub(super) fn enter_custom_zoom(&mut self) {
        if self.zoom_mode != ZoomMode::Custom {
            self.zoom_scale = self.last_effective_zoom.clamp(0.3, 3.0);
            self.zoom_mode = ZoomMode::Custom;
        }
    }

    pub(super) fn go_to_page(&mut self, page: u32, _ctx: &egui::Context) {
        if page < 1 || page > self.total_pages || page == self.current_page {
            self.target_page = self.current_page;
            return;
        }
        self.single_page_wheel = SinglePageWheel::default();
        self.sync_and_save();

        self.record_lecture_page(page);

        self.current_page = page;
        // Reading position belongs to state.json, not to the PDF's annotation payload.
        // Queue it on navigation, including when no notes were edited.
        self.save_session_state();
        if !self.audio_engine.is_recording()
            && !self.audio_engine.is_playing()
            && !self.is_transcribing
            && let Some(i) = self.audio_index.other_covering(page)
        {
            self.audio_engine.stop_and_release();
            self.audio_index.switch_to_other(i);
            self.transcript_last_seg = None;
            self.audio_index.save(&self.pdf_path);
        }
        self.target_page = page;
        if !self.lock_note_page {
            self.note_page = page;
            self.current_note = self.notes.get(&page).cloned().unwrap_or_default();
        }

        self.selected_textbox = None;
        if matches!(
            self.view_mode,
            ViewMode::SinglePage | ViewMode::Slideshow | ViewMode::SlideshowWithNotes
        ) {
            self.pdf_selection = None;
            self.selecting = false;
            self.active_stroke = None;
            self.radial_menu_pos = None;
            self.slide_rects.clear();
            self.last_slide_rects.clear();
        }
        self.continuous_scroll_to_page = Some(page);
        // Keep the last texture visible until the destination is ready.

        // Rendering requests are issued once per frame by `pump_render_requests`.
        if let Some((handle, dims)) = self.viewer.page(page) {
            self.texture_handle = Some(handle.clone());
            self.page_dims = *dims;
            self.is_rendering = false;
            self.viewer.touch_page(page);
        } else {
            self.is_rendering = true;
            // Prefer a destination thumbnail; otherwise retain the previous render.
            if let Some((h, dims)) = self.viewer.thumbnail(page) {
                self.texture_handle = Some(h.clone());
                self.page_dims = *dims;
            }
        }
    }

    pub(super) fn render_view(&self) -> RenderView {
        RenderView {
            current_page: self.current_page,
            total_pages: self.total_pages,
            continuous_visible_pages: (self.view_mode == ViewMode::Continuous)
                .then_some(self.visible_pages),
            split_page: self.split_view_enabled.then_some(self.split_secondary_page),
        }
    }

    pub(super) fn pump_render_requests(&mut self, ctx: &egui::Context) {
        // Keep scaling cached textures until the panel drag ends.
        if !self.has_pdf() || self.notes_panel_resizing {
            return;
        }
        let scale = render_scale(
            self.page_pt,
            self.display_w_pt,
            ctx.pixels_per_point(),
            matches!(
                self.view_mode,
                ViewMode::Slideshow | ViewMode::SlideshowWithNotes
            ),
        );
        self.viewer
            .request_pages(self.render_view(), scale, self.pdf_dark_mode);
    }

    pub(super) fn current_book_spread(&self) -> (Option<u32>, Option<u32>) {
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

    pub(super) fn prev_page(&mut self, ctx: &egui::Context) {
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

    pub(super) fn next_page(&mut self, ctx: &egui::Context) {
        if self.view_mode == ViewMode::TwoPage {
            let (_, right_opt) = self.current_book_spread();
            let base = right_opt.unwrap_or(self.current_page);
            if let Some(next_p) =
                ((base + 1)..=self.total_pages).find(|p| !self.hidden_pages.contains(p))
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

    pub(super) fn toggle_hide_page(&mut self, page: u32, ctx: &egui::Context) {
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
}

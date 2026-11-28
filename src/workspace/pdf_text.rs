//! PDF text selection and the workspace search controls.

use super::*;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::TryRecvError;

/// Text and character positions used to search the PDF and highlight hits on the page.
#[derive(Default)]
struct PdfSearchResults {
    pages: HashMap<u32, search::SearchPage>,
    texts: HashMap<u32, Arc<PageText>>,
}

struct SearchIndexJob {
    results: Receiver<PdfSearchResults>,
    cancel: Arc<AtomicBool>,
}

impl Drop for SearchIndexJob {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub(super) struct PdfSearchIndex {
    results: Option<PdfSearchResults>,
    pending: Option<SearchIndexJob>,
}

impl PdfSearchIndex {
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }

    fn start(&mut self, path: PathBuf, total: u32, ctx: &egui::Context) {
        if self.results.is_some() || self.pending.is_some() {
            return;
        }
        let (sender, results) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let ctx = ctx.clone();
        match thread::Builder::new()
            .name("lapis-pdf-search".into())
            .spawn(move || {
                let index = build_pdf_search_index(&path, total, &worker_cancel);
                if !worker_cancel.load(Ordering::Relaxed) {
                    let _ = sender.send(index);
                    ctx.request_repaint();
                }
            }) {
            Ok(_) => self.pending = Some(SearchIndexJob { results, cancel }),
            Err(err) => {
                eprintln!("Lapis: could not start PDF search: {err}");
                self.results = Some(PdfSearchResults::default());
            }
        }
    }

    fn poll(&mut self) -> bool {
        let Some(job) = &self.pending else {
            return false;
        };
        match job.results.try_recv() {
            Ok(results) => self.results = Some(results),
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => self.results = Some(PdfSearchResults::default()),
        }
        self.pending = None;
        true
    }

    #[cfg(test)]
    pub(super) fn wait_for_result(&mut self) {
        let job = self.pending.take().expect("PDF search worker was started");
        self.results = Some(
            job.results
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap(),
        );
    }
}

/// Appends `s[a..b]` to `job`; the parts covered by `hl` (byte ranges, sorted) get a background.
pub(super) fn append_highlighted(
    job: &mut egui::text::LayoutJob,
    s: &str,
    a: usize,
    b: usize,
    fmt: egui::TextFormat,
    hl: &[(usize, usize, bool)],
    (bg_cur, bg_other): (egui::Color32, egui::Color32),
) {
    let mut pos = a;
    for &(hs, he, cur) in hl {
        if he <= pos || hs >= b {
            continue;
        }
        let s0 = hs.max(pos);
        let e0 = he.min(b);
        if s0 > pos {
            job.append(&s[pos..s0], 0.0, fmt.clone());
        }
        let mut f = fmt.clone();
        f.background = if cur { bg_cur } else { bg_other };
        job.append(&s[s0..e0], 0.0, f);
        pos = e0;
    }
    if pos < b {
        job.append(&s[pos..b], 0.0, fmt);
    }
}

/// A caret sits BETWEEN characters: `idx` in `0..=chars.len()` of the given page.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) struct TextCaret {
    pub(super) page: u32,
    pub(super) idx: usize,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct TextSelection {
    pub(super) anchor: TextCaret,
    pub(super) focus: TextCaret,
}

impl TextSelection {
    /// (start, end) in reading order, whichever way the user dragged.
    pub(super) fn ordered(&self) -> (TextCaret, TextCaret) {
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

/// Retire the previous worker's channel and cancellation token as a pair.
impl Lapis {
    /// Highlights the selected text of one page: one rectangle per line, like a real viewer.
    pub(super) fn paint_text_selection(
        &self,
        painter: &egui::Painter,
        page: u32,
        slide_rect: egui::Rect,
    ) {
        let Some(sel) = self.pdf_selection else {
            return;
        };
        let Some(text) = self.viewer.text(page) else {
            return;
        };
        let Some((lo, hi)) = sel.range_on_page(page, text.chars.len()) else {
            return;
        };
        let [r, g, b] = self.theme.accent;
        let fill = egui::Color32::from_rgba_unmultiplied(r, g, b, 85);
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

    /// Highlights the search hits of one page: the current one stands out, the others are faint.
    pub(super) fn paint_search_highlight(
        &self,
        painter: &egui::Painter,
        page: u32,
        slide_rect: egui::Rect,
    ) {
        if !self.show_search || self.search_hits.is_empty() {
            return;
        }
        let Some(text) = self
            .pdf_search_index
            .results
            .as_ref()
            .and_then(|index| index.texts.get(&page))
        else {
            return;
        };
        let warn = rgb(self.theme.accent);
        let (w, h) = (slide_rect.width(), slide_rect.height());
        for (i, hit) in self.search_hits.iter().enumerate() {
            if hit.page != page || hit.source != search::Source::Pdf {
                continue;
            }
            let cur = i == self.search_sel;
            let alpha = if cur { 140 } else { 60 };
            let fill = egui::Color32::from_rgba_unmultiplied(warn.r(), warn.g(), warn.b(), alpha);
            for l in &text.lines {
                let a = l.start.max(hit.start);
                let b = l.end.min(hit.end);
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
                )
                .expand2(egui::vec2(1.0, 0.5));
                painter.rect_filled(r, egui::CornerRadius::same(1), fill);
                if cur {
                    painter.rect_stroke(
                        r,
                        egui::CornerRadius::same(1),
                        egui::Stroke::new(1.5, warn),
                        egui::StrokeKind::Outside,
                    );
                }
            }
        }
    }

    /// Text of the current selection (can span several pages). Pages whose text has not
    /// been loaded yet (never rendered) are extracted synchronously: rare, copy-time only.
    pub(super) fn selection_text(&self) -> Option<String> {
        let sel = self.pdf_selection?;
        let (s, e) = sel.ordered();
        if s == e {
            return None;
        }
        let mut extra: HashMap<u32, PageText> = HashMap::new();
        if (s.page..=e.page).any(|p| self.viewer.text(p).is_none())
            && let Ok(doc) = Document::open(self.pdf_path.to_str().unwrap_or_default())
        {
            for p in s.page..=e.page {
                if self.viewer.text(p).is_none()
                    && let Some(t) = extract_text(&doc, p as i32 - 1)
                {
                    extra.insert(p, t);
                }
            }
        }
        let mut parts: Vec<String> = Vec::new();
        for p in s.page..=e.page {
            let text = self.viewer.text(p).or_else(|| extra.get(&p));
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
    pub(super) fn update_text_drag(&mut self, ctx: &egui::Context) {
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
        let Some(text) = self.viewer.text(page) else {
            return;
        };
        let nx = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        let ny = ((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
        let idx = text.caret_at(nx, ny);
        if let Some(sel) = &mut self.pdf_selection {
            sel.focus = TextCaret { page, idx };
        }
    }

    pub(super) fn open_search(&mut self) {
        self.show_search = true;
        self.focus_search_input = true;
    }

    /// Closing the search also resets it: query, results and highlights are dropped.
    pub(super) fn close_search(&mut self) {
        self.show_search = false;
        self.focus_search_input = false;
        self.search_query.clear();
        self.search_last_query.clear();
        self.search_hits.clear();
        self.search_sel = 0;
        self.search_key = 0;
        self.search_scroll_to_sel = false;
        self.search_note_scroll = false;
    }

    /// Everything the search results depend on, hashed: when it changes they are recomputed.
    fn search_state_key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.search_query.hash(&mut h);
        self.current_note.hash(&mut h);
        self.note_page.hash(&mut h);
        self.notes.len().hash(&mut h);
        self.total_pages.hash(&mut h);
        self.audio_index.segments.len().hash(&mut h);
        self.audio_index.page_memos.len().hash(&mut h);
        self.study_revision.hash(&mut h);
        self.pdf_search_index.results.is_some().hash(&mut h);
        h.finish()
    }

    fn recompute_search_hits(&mut self, ctx: &egui::Context) {
        let needle = search::fold_query(&self.search_query);
        let mut hits: Vec<search::Hit> = Vec::new();
        if needle.is_empty() {
            self.search_hits = hits;
            return;
        }
        self.ensure_pdf_search_index(ctx);
        let pdf = self.pdf_search_index.results.as_ref();

        'pages: for p in 1..=self.total_pages {
            // The note being edited lives in `current_note`, not in `notes`, until it is saved.
            let note: &str = if p == self.note_page {
                &self.current_note
            } else {
                self.notes.get(&p).map(|s| s.as_str()).unwrap_or("")
            };
            if !note.trim().is_empty() {
                for m in search::SearchPage::from_text(note).find(&needle, search::MAX_HITS) {
                    hits.push(search::Hit {
                        page: p,
                        source: search::Source::Note,
                        start: m.start,
                        end: m.end,
                        snippet: m.snippet,
                        snip_match: m.snip_match,
                        ts_ms: None,
                        tag: String::new(),
                    });
                    if hits.len() >= search::MAX_HITS {
                        break 'pages;
                    }
                }
            }
            if let Some(pg) = pdf.and_then(|index| index.pages.get(&p)) {
                for m in pg.find(&needle, search::MAX_HITS) {
                    hits.push(search::Hit {
                        page: p,
                        source: search::Source::Pdf,
                        start: m.start,
                        end: m.end,
                        snippet: m.snippet,
                        snip_match: m.snip_match,
                        ts_ms: None,
                        tag: String::new(),
                    });
                    if hits.len() >= search::MAX_HITS {
                        break 'pages;
                    }
                }
            }
        }

        if hits.len() < search::MAX_HITS {
            for seg in &self.audio_index.segments {
                if let Some(m) = search::SearchPage::from_text(&seg.text)
                    .find(&needle, 1)
                    .into_iter()
                    .next()
                {
                    hits.push(search::Hit {
                        page: seg.page,
                        source: search::Source::Audio,
                        start: 0,
                        end: 0,
                        snippet: m.snippet,
                        snip_match: m.snip_match,
                        ts_ms: Some(seg.start_ms),
                        tag: audio::format_ms(seg.start_ms),
                    });
                }
            }
            for (&p, memos) in &self.audio_index.page_memos {
                for memo in memos {
                    let Some(tr) = memo.transcript.as_deref() else {
                        continue;
                    };
                    if let Some(m) = search::SearchPage::from_text(tr)
                        .find(&needle, 1)
                        .into_iter()
                        .next()
                    {
                        hits.push(search::Hit {
                            page: p,
                            source: search::Source::Memo,
                            start: 0,
                            end: 0,
                            snippet: m.snippet,
                            snip_match: m.snip_match,
                            ts_ms: None,
                            tag: memo.id.to_string(),
                        });
                    }
                }
            }
        }

        // Stable: within a page the order stays note, slide, audio, memo.
        hits.sort_by_key(|h| h.page);
        hits.truncate(search::MAX_HITS);
        self.search_hits = hits;
    }

    fn search_step(&mut self, delta: i64, ctx: &egui::Context) {
        let n = self.search_hits.len() as i64;
        if n == 0 {
            return;
        }
        self.search_sel = (self.search_sel as i64 + delta).rem_euclid(n) as usize;
        self.search_goto_selected(ctx);
    }

    /// Shows the selected hit: page, note (edit mode, scrolled to the match) or audio position.
    fn search_goto_selected(&mut self, ctx: &egui::Context) {
        let Some(hit) = self.search_hits.get(self.search_sel).cloned() else {
            return;
        };
        self.search_scroll_to_sel = true;

        if hit.page != self.current_page && (1..=self.total_pages).contains(&hit.page) {
            self.go_to_page(hit.page, ctx);
        }

        if hit.source == search::Source::Note {
            if self.note_page != hit.page {
                self.switch_note_page(hit.page);
            }
            self.show_notes = true;
            // Only the editor can highlight the match.
            self.note_mode = NoteMode::Edit;
            self.search_note_scroll = true;
        }

        if let Some(ts_ms) = hit.ts_ms {
            if self.audio_engine.total_duration_ms == 0 {
                if let Some(ref wav_name) = self.audio_index.lecture_wav {
                    let wav_path =
                        audio::AudioDocIndex::media_dir_for_pdf(&self.pdf_path).join(wav_name);
                    let _ =
                        self.audio_engine
                            .load_and_play(&wav_path, "Lecture".to_string(), ts_ms);
                }
            } else {
                self.audio_engine.seek_to_ms(ts_ms);
            }
        }
        ctx.request_repaint();
    }

    pub(super) fn render_search_popup(&mut self, ui: &mut egui::Ui) {
        if !self.show_search {
            return;
        }
        let ctx = ui.ctx().clone();
        let index_ready = self.pdf_search_index.poll();
        let input_id = ui.make_persistent_id("global_search_input");
        let input_had_focus = ctx.memory(|m| m.has_focus(input_id));

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.close_search();
            return;
        }

        // Enter / Shift+Enter (and the arrows) cycle through the results while the search box has
        // the focus. They are consumed so the box keeps its focus.
        let mut step: i64 = 0;
        if input_had_focus {
            ctx.input_mut(|i| {
                if i.consume_key(egui::Modifiers::SHIFT, egui::Key::Enter) {
                    step -= 1;
                } else if i.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {
                    step += 1;
                }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                    step += 1;
                }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                    step -= 1;
                }
            });
        }

        if step != 0 {
            self.search_step(step.signum(), &ctx);
        }

        let accent = rgb(self.theme.accent);
        let weak = rgb(self.theme.fg_muted);
        let colors = search::RowColors {
            accent,
            text: rgb(self.theme.fg_primary),
            weak,
            hover: rgb(self.theme.bg_hover),
        };
        let (mut close, mut refocus) = (false, false);
        let mut clicked_row: Option<usize> = None;
        let mut nav_step: i64 = 0;
        // Rows visible before the list scrolls. The half row shows that there is more below.
        const LIST_ROWS: f32 = 3.5;

        let (search_align, search_offset) =
            if self.app_state.settings.topbar_position == state::TopbarPosition::Bottom {
                (egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -46.0))
            } else {
                (egui::Align2::RIGHT_TOP, egui::vec2(-16.0, 46.0))
            };
        egui::Area::new(ui.make_persistent_id("global_search_popover"))
            .anchor(search_align, search_offset)
            .order(egui::Order::Foreground)
            .constrain(true)
            .show(&ctx, |ui| {
                self.modal_glass_frame(MENU_GLASS_ALPHA)
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_width(340.0);
                        ui.spacing_mut().item_spacing.y = 2.0;
                        let n = self.search_hits.len();

                        // One row: icon, text box, counter and result buttons. Every piece has a
                        // fixed width and the row a fixed height, so nothing overlaps or stretches.
                        egui::Frame::NONE
                            .fill(rgb(self.theme.bg_widget))
                            .corner_radius(egui::CornerRadius::same(7))
                            .stroke(egui::Stroke::new(
                                1.0,
                                if input_had_focus {
                                    accent
                                } else {
                                    rgb(self.theme.border)
                                },
                            ))
                            .inner_margin(egui::Margin::symmetric(8, 3))
                            .show(ui, |ui| {
                                const ROW_H: f32 = 24.0;
                                const GAP: f32 = 4.0;
                                const ICON_W: f32 = 18.0;
                                const BTN: f32 = 22.0;
                                const COUNTER_W: f32 = 56.0;
                                let row_w = ui.available_width();
                                let status = if self.search_query.trim().is_empty() {
                                    String::new()
                                } else if n == 0 {
                                    "No match".to_string()
                                } else if n >= search::MAX_HITS {
                                    format!("{}/{}+", self.search_sel + 1, n)
                                } else {
                                    format!("{}/{}", self.search_sel + 1, n)
                                };
                                ui.allocate_ui_with_layout(
                                    egui::vec2(row_w, ROW_H),
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| {
                                        ui.spacing_mut().item_spacing.x = GAP;
                                        ui.add_sized(
                                            [ICON_W, ROW_H],
                                            egui::Label::new(
                                                egui::RichText::new("\u{f0349}")
                                                    .size(14.0)
                                                    .color(accent),
                                            ),
                                        );
                                        let text_w =
                                            (row_w - ICON_W - COUNTER_W - 3.0 * BTN - 5.0 * GAP)
                                                .max(40.0);
                                        let te = ui.add(
                                            egui::TextEdit::singleline(&mut self.search_query)
                                                .id(input_id)
                                                .hint_text("Search notes, slides, audio…")
                                                .font(egui::FontId::proportional(14.0))
                                                .frame(egui::Frame::NONE)
                                                .desired_width(text_w),
                                        );
                                        // A new popup spends its first frame(s) measuring itself and
                                        // drops focus requests, so keep asking until the box has it.
                                        if self.focus_search_input {
                                            if te.has_focus() {
                                                self.focus_search_input = false;
                                            } else {
                                                te.request_focus();
                                            }
                                        }
                                        ui.allocate_ui_with_layout(
                                            egui::vec2(COUNTER_W, ROW_H),
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.label(
                                                    egui::RichText::new(status)
                                                        .size(11.0)
                                                        .color(weak),
                                                );
                                            },
                                        );
                                        let icon_btn = |glyph: &str| {
                                            egui::Button::new(egui::RichText::new(glyph).size(12.0))
                                                .frame(false)
                                                .min_size(egui::vec2(BTN, BTN))
                                        };
                                        if ui
                                            .add_enabled(n > 0, icon_btn("\u{f0143}"))
                                            .on_hover_text("Previous result [Shift+Enter]")
                                            .clicked()
                                        {
                                            nav_step = -1;
                                            refocus = true;
                                        }
                                        if ui
                                            .add_enabled(n > 0, icon_btn("\u{f0140}"))
                                            .on_hover_text("Next result [Enter]")
                                            .clicked()
                                        {
                                            nav_step = 1;
                                            refocus = true;
                                        }
                                        if ui
                                            .add(icon_btn("\u{f0156}"))
                                            .on_hover_text("Close and reset the search [Esc]")
                                            .clicked()
                                        {
                                            close = true;
                                        }
                                    },
                                );
                            });

                        if self.search_query.trim().is_empty() {
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(
                                    "Enter next  ·  Shift+Enter previous  ·  Esc close",
                                )
                                .size(11.0)
                                .color(weak),
                            );
                        }

                        if self.pdf_search_index.pending.is_some()
                            && !self.search_query.trim().is_empty()
                        {
                            ui.weak("Indexing PDF…");
                        }
                        if n > 0 {
                            ui.add_space(4.0);
                            let list_max_h =
                                (search::ROW_H + ui.spacing().item_spacing.y) * LIST_ROWS;
                            let hits = &self.search_hits;
                            let sel = self.search_sel;
                            let follow = self.search_scroll_to_sel;
                            ui.scope(|ui| {
                                // A solid scroll bar, so it is clear when there is more to scroll.
                                ui.style_mut().spacing.scroll = egui::style::ScrollStyle::solid();
                                ui.visuals_mut().widgets.inactive.bg_fill = accent;
                                ui.visuals_mut().widgets.hovered.bg_fill =
                                    accent.gamma_multiply(1.2); // brighter on hover
                                ui.visuals_mut().widgets.active.bg_fill =
                                    accent.gamma_multiply(0.8); // darker while dragging
                                egui::ScrollArea::vertical()
                                    .id_salt("global_search_results_scroll")
                                    .max_height(list_max_h)
                                    // An Area only offers the size it had last frame, which for a popup
                                    // that opened empty is tiny: ask for the full height explicitly.
                                    .min_scrolled_height(list_max_h)
                                    .auto_shrink([false, true])
                                    .show(ui, |ui| {
                                        for (i, hit) in hits.iter().enumerate() {
                                            let resp =
                                                search::result_row(ui, hit, i == sel, &colors);
                                            if resp.clicked() {
                                                clicked_row = Some(i);
                                            }
                                            // Keep the selected result in view while cycling.
                                            if follow && i == sel {
                                                resp.scroll_to_me(None);
                                            }
                                        }
                                    });
                            });
                            self.search_scroll_to_sel = false;
                        }
                    });
            });

        if close {
            self.close_search();
            return;
        }

        // Results follow the query (and the notes) live.
        let key = self.search_state_key();
        if key != self.search_key || index_ready {
            self.search_key = key;
            let query_changed = self.search_query != self.search_last_query;
            let previous = self
                .search_hits
                .get(self.search_sel)
                .map(|h| (h.page, h.source, h.start));
            self.recompute_search_hits(&ctx);
            ctx.request_repaint();
            if query_changed {
                self.search_last_query = self.search_query.clone();
                self.search_sel = 0;
                self.search_scroll_to_sel = true;
                // Like a browser's find: show the first hit while typing.
                self.search_goto_selected(&ctx);
            } else {
                self.search_sel = previous
                    .and_then(|pv| {
                        self.search_hits
                            .iter()
                            .position(|h| (h.page, h.source, h.start) == pv)
                    })
                    .unwrap_or(self.search_sel)
                    .min(self.search_hits.len().saturating_sub(1));
                if index_ready && previous.is_none() {
                    self.search_goto_selected(&ctx);
                }
            }
        }

        if let Some(i) = clicked_row {
            self.search_sel = i;
            self.search_goto_selected(&ctx);
            refocus = true;
        }
        if nav_step != 0 {
            self.search_step(nav_step, &ctx);
        }
        if refocus {
            ctx.memory_mut(|m| m.request_focus(input_id));
        }
    }

    pub(super) fn ensure_pdf_search_index(&mut self, ctx: &egui::Context) {
        if self.has_pdf() {
            self.pdf_search_index
                .start(self.pdf_path.clone(), self.total_pages, ctx);
        }
    }
}

fn build_pdf_search_index(path: &Path, total: u32, cancel: &AtomicBool) -> PdfSearchResults {
    let mut index = PdfSearchResults::default();
    let Ok(doc) = Document::open(path.to_str().unwrap_or_default()) else {
        return index;
    };
    for idx in 0..(total as i32) {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Some(text) = extract_text(&doc, idx) else {
            continue;
        };
        if text.chars.is_empty() {
            continue;
        }
        // Lines are joined with a space so a phrase can match across a line break.
        let mut items: Vec<(char, usize)> = Vec::with_capacity(text.chars.len() + text.lines.len());
        for line in &text.lines {
            for i in line.start..line.end {
                items.push((text.chars[i].ch, i));
            }
            items.push((' ', search::SYNTHETIC));
        }
        let page = (idx + 1) as u32;
        index.pages.insert(page, search::SearchPage::new(items));
        index.texts.insert(page, Arc::new(text));
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::isolated_dir;

    #[test]
    fn retired_pdf_search_results_cannot_reach_the_next_index() {
        let root = isolated_dir!(retired_pdf_search_results_cannot_reach_the_next_index);
        let second = root.join("new.pdf");
        notes::create_blank_study_notebook(&second).unwrap();

        let (sender, results) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut index = PdfSearchIndex {
            results: None,
            pending: Some(SearchIndexJob {
                results,
                cancel: Arc::clone(&cancel),
            }),
        };
        let mut old = PdfSearchResults::default();
        old.pages
            .insert(10, search::SearchPage::from_text("Old document"));
        assert!(sender.send(old).is_ok());
        index.clear();
        assert!(cancel.load(Ordering::Relaxed));
        assert!(sender.send(PdfSearchResults::default()).is_err());

        index.start(second, 1, &egui::Context::default());
        index.wait_for_result();
        assert!(!index.results.as_ref().unwrap().pages.contains_key(&10));
    }
}

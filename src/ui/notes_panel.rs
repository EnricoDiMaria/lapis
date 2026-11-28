//! Notes editor, preview, and slideshow notes panels.

use super::super::*;
use crate::workspace::pdf_text::append_highlighted;

impl Lapis {
    pub(in crate::workspace) fn render_notes_panel_contents(
        &mut self,
        ui: &mut egui::Ui,
        text_edit_id: egui::Id,
        use_bottom: bool,
    ) {
        // A resizable side panel must keep using its full assigned width; otherwise its
        // contents can keep the previous frame's narrower min-rect while the splitter moves.
        ui.take_available_width();
        let panel_bg = ui.visuals().panel_fill;
        let page = self.current_page;
        let media_dir = AudioDocIndex::media_dir_for_pdf(&self.pdf_path);

        // Full panel rectangle, including the user-selected height.
        let panel_rect = ui.max_rect();

        let memo_count = if self.app_state.settings.audio_notes {
            self.audio_index
                .page_memos
                .get(&page)
                .map(|v| v.len())
                .unwrap_or(0)
        } else {
            0
        };
        let has_page_memos = memo_count > 0;

        // 1. Determine which bottom controls are visible.
        let playing = self.audio_engine.is_playing();
        let pos_ms = self.audio_engine.current_pos_ms();
        let has_wav = self.audio_index.lecture_wav.is_some();
        let has_transcript = !self.audio_index.segments.is_empty();

        let has_relevant_segment = if playing || pos_ms > 0 {
            self.audio_index
                .segment_index_at(pos_ms)
                .and_then(|i| self.audio_index.segments.get(i))
                .is_some()
                || self.audio_index.segments.iter().any(|s| s.page == page)
        } else {
            self.audio_index.segments.iter().any(|s| s.page == page)
        };

        let show_ticker = self.app_state.settings.audio_dock
            && !self.audio_dock_open
            && (has_relevant_segment || self.is_transcribing);

        let memo_h = if has_page_memos {
            let visible_rows = memo_count.min(3);
            32.0 + visible_rows as f32 * 24.0
        } else {
            0.0
        };

        // Use panel_rect.height() for stable sizing.
        let max_h = (panel_rect.height() * 0.55).max(140.0);

        let default_h: f32 = if has_transcript {
            220.0
        } else if has_wav {
            150.0
        } else {
            125.0
        };

        let dock_h = if !self.app_state.settings.audio_dock {
            0.0
        } else if self.audio_dock_open {
            if self.audio_dock_height <= 0.0 {
                self.audio_dock_height = default_h;
            }
            // Do not overwrite the saved dock height when the panel is constrained.
            // Clamp only the height used for this frame.
            self.audio_dock_height.clamp(100.0, max_h)
        } else if show_ticker {
            32.0
        } else {
            0.0
        };

        let top_handle_offset = if use_bottom {
            let handle_rect = egui::Rect::from_center_size(
                egui::pos2(panel_rect.center().x, panel_rect.top() + 6.0),
                egui::vec2(42.0, 4.0),
            );

            let pointer_in_handle = ui.rect_contains_pointer(handle_rect.expand(4.0));
            let color = if pointer_in_handle {
                rgb(self.theme.accent)
            } else {
                rgb(self.theme.fg_muted)
            };

            ui.painter()
                .rect_filled(handle_rect, egui::CornerRadius::same(2), color);
            12.0
        } else {
            0.0
        };

        let full_panel_rect = egui::Rect::from_min_max(
            egui::pos2(panel_rect.left(), panel_rect.top() + top_handle_offset),
            panel_rect.max,
        );

        let total_bottom_h = memo_h + dock_h;
        let min_notes_h = 65.0;
        let notes_max_h = (full_panel_rect.height() - total_bottom_h).max(min_notes_h);

        // Split rectangles for notes and audio.
        let notes_rect = egui::Rect::from_min_size(
            full_panel_rect.min,
            egui::vec2(full_panel_rect.width(), notes_max_h),
        );
        // Keep the audio area anchored to the same panel edges in every notes mode.
        // The audio controls apply their own inset, so Sketch must not shrink this rect.
        let side_pad = 0.0;
        let bottom_rect = egui::Rect::from_min_max(
            egui::pos2(full_panel_rect.left() + side_pad, notes_rect.bottom()),
            egui::pos2(full_panel_rect.right() - side_pad, full_panel_rect.bottom()),
        );

        // ---------------------------------------------------------
        // A. Notes area, constrained to `notes_rect`.
        // ---------------------------------------------------------
        let mut notes_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(notes_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        notes_ui.set_clip_rect(notes_rect);

        {
            let ui = &mut notes_ui;
            ui.take_available_width();

            // A compact, frameless single row: page navigation on the left, mode controls fill the rest.
            let sk_on = self.sketch_enabled();
            let button_radius =
                egui::CornerRadius::same(self.theme.corner_radius.saturating_sub(1));

            let (nav_h, mode_h) = (self.sz(23.0), self.sz(22.0));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(2.0, 0.0);
                let full_width = ui.available_width();
                let compact = full_width < 320.0;

                // Page navigation stays compact and left aligned.
                let lock_icon = if self.lock_note_page { "󰌾" } else { "󰌿" };
                if ui
                    .add_sized(
                        [18.0, nav_h],
                        crate::theme::selected_button(
                            egui::RichText::new(lock_icon).size(11.0),
                            self.lock_note_page,
                            ui.visuals(),
                        )
                        .corner_radius(button_radius),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text("Lock or unlock the note page while scrolling the PDF")
                    .clicked()
                {
                    self.lock_note_page = !self.lock_note_page;
                    if !self.lock_note_page && self.note_page != self.current_page {
                        self.switch_note_page(self.current_page);
                    }
                }

                if ui
                    .add_enabled(
                        self.note_page > 1,
                        egui::Button::new(egui::RichText::new("\u{F438}").size(11.0))
                            .min_size(egui::vec2(17.0, nav_h))
                            .corner_radius(button_radius),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text("Previous notes page")
                    .clicked()
                {
                    self.lock_note_page = true;
                    self.switch_note_page(self.note_page - 1);
                }

                let page_label = if compact {
                    self.note_page.to_string()
                } else {
                    format!("p. {}", self.note_page)
                };
                if ui
                    .add_sized(
                        [if compact { 23.0 } else { 35.0 }, nav_h],
                        egui::Button::new(egui::RichText::new(page_label).size(10.0).strong())
                            .corner_radius(button_radius),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text("Return notes to the current PDF page")
                    .clicked()
                {
                    self.note_page = self.current_page;
                    self.lock_note_page = false;
                }

                if ui
                    .add_enabled(
                        self.note_page < self.total_pages,
                        egui::Button::new(egui::RichText::new("\u{F44A}").size(11.0))
                            .min_size(egui::vec2(17.0, nav_h))
                            .corner_radius(button_radius),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text("Next notes page")
                    .clicked()
                {
                    self.lock_note_page = true;
                    self.switch_note_page(self.note_page + 1);
                }

                ui.add_space(3.0);
                ui.separator();
                ui.add_space(4.0);

                // Sketch stays a small icon tab. Edit and Preview divide the remaining row.
                // Switch to icon-only controls while there is still room, rather than letting
                // the Preview label become squeezed or clipped at narrow panel widths.
                let mode_available = ui.available_width().max(0.0);
                let sketch_width = if sk_on { 23.0 } else { 0.0 };
                let mode_gap_total = if sk_on { 4.0 } else { 2.0 };
                let primary_width =
                    ((mode_available - sketch_width - mode_gap_total) / 2.0).max(18.0);
                let show_labels = primary_width >= 84.0;
                let edit_label = if show_labels { "󰏫  Edit" } else { "󰏫" };
                let preview_label = if show_labels { "󰈈  Preview" } else { "󰈈" };
                let mode_font_size = if show_labels { 10.0 } else { 11.0 };

                if sk_on {
                    let resp = ui.add_sized(
                        [sketch_width, mode_h],
                        crate::theme::selected_button(
                            egui::RichText::new(" ").size(11.0).strong(),
                            self.note_mode == NoteMode::Sketch,
                            ui.visuals(),
                        )
                        .corner_radius(button_radius),
                    );
                    let has_sketch = self
                        .sketches
                        .get(&self.note_page)
                        .is_some_and(|s| !s.strokes.is_empty());
                    if has_sketch {
                        // Corner dot like Notes and Flashcards, slightly smaller for the narrow
                        // tab. The clip is widened so the panel edge cannot crop it.
                        let dot_center = egui::pos2(
                            resp.rect.right_top().x - 3.0,
                            resp.rect.right_top().y + 3.5,
                        );
                        let painter = ui
                            .painter()
                            .with_clip_rect(ui.painter().clip_rect().expand(6.0));
                        painter.circle(
                            dot_center,
                            2.5,
                            theme::rgb(self.theme.accent),
                            egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 255, 255)),
                        );
                    }
                    let mut hover = self
                        .app_state
                        .shortcuts
                        .toggle_sketch
                        .hint("Sketch: a separate scratchpad for this page");
                    if has_sketch {
                        hover.push_str("\n(This page contains a sketch)");
                    }
                    if resp
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text(hover)
                        .clicked()
                    {
                        self.note_mode = NoteMode::Sketch;
                    }
                }

                if ui
                    .add_sized(
                        [primary_width, mode_h],
                        crate::theme::selected_button(
                            egui::RichText::new(edit_label)
                                .size(mode_font_size)
                                .strong(),
                            self.note_mode == NoteMode::Edit,
                            ui.visuals(),
                        )
                        .corner_radius(button_radius),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text(
                        self.app_state
                            .shortcuts
                            .toggle_edit_preview
                            .hint("Edit your Markdown and LaTeX notes"),
                    )
                    .clicked()
                {
                    self.note_mode = NoteMode::Edit;
                    self.focus_note_editor = true;
                }

                if ui
                    .add_sized(
                        [primary_width, mode_h],
                        crate::theme::selected_button(
                            egui::RichText::new(preview_label)
                                .size(mode_font_size)
                                .strong(),
                            self.note_mode == NoteMode::Preview,
                            ui.visuals(),
                        )
                        .corner_radius(button_radius),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text(
                        self.app_state
                            .shortcuts
                            .toggle_edit_preview
                            .hint("Preview the formatted notes"),
                    )
                    .clicked()
                {
                    self.note_mode = NoteMode::Preview;
                }
            });

            let sketch_open = self.note_mode == NoteMode::Sketch && sk_on;
            ui.add_space(self.sz(8.0));

            let is_edit = matches!(self.note_mode, NoteMode::Edit);

            // Keep the text origin and usable width identical in Edit and Preview.
            // The Edit field has zero internal margin, so this shared frame inset is
            // also the exact origin used by the Markdown viewer.
            let text_content_margin = egui::Margin::symmetric(14, self.sz(8.0) as i8);
            let (notes_bg, notes_stroke, notes_margin) = if is_edit {
                (
                    rgb(self.theme.bg_widget),
                    egui::Stroke::new(1.0, rgb(self.theme.border)),
                    text_content_margin,
                )
            } else if sketch_open {
                // The canvas fills the whole panel below the toolbar.
                (
                    egui::Color32::TRANSPARENT,
                    egui::Stroke::NONE,
                    egui::Margin::ZERO,
                )
            } else {
                // Preview sits directly on the notes panel background, without a card surface.
                (
                    egui::Color32::TRANSPARENT,
                    egui::Stroke::NONE,
                    text_content_margin,
                )
            };
            // Main Notes Area
            egui::Frame::NONE
                .fill(notes_bg)
                .stroke(notes_stroke)
                .corner_radius(egui::CornerRadius::same(self.theme.corner_radius))
                .inner_margin(notes_margin)
                .show(ui, |ui| {
                    // Reclaim the panel's current width after a live resize; do not keep a
                    // narrower min-rect from the previous frame.
                    ui.take_available_width();
                    if self.note_mode == NoteMode::Sketch {
                        let sketch_page = self.note_page;
                        if sketch::show(
                            ui,
                            &mut self.sketch,
                            sketch_page,
                            &mut self.sketches,
                            &self.app_state.shortcuts.trackpad_draw_hold,
                        ) {
                            self.save_sketch_data();
                        }
                        return;
                    }

                    let scroll_id = ui.make_persistent_id("notes_scroll");
                    let anim_scroll_id = ui.make_persistent_id("notes_anim_scroll_target");

                    let current_offset_y = ui.data(|d| {
                        d.get_temp::<f32>(scroll_id.with("last_offset"))
                            .unwrap_or(0.0)
                    });

                    let preview_view_width = ui.available_width().max(1.0);
                    ui.take_available_width();
                    let mut scroll_area = egui::ScrollArea::vertical()
                        .id_salt("notes_scroll")
                        .auto_shrink([false, false])
                        .max_width(preview_view_width);

                    if let Some(target) = ui.data(|d| d.get_temp::<f32>(anim_scroll_id)) {
                        let diff = target - current_offset_y;
                        if diff.abs() > 1.0 {
                            let next_offset = current_offset_y + diff * 0.2;
                            scroll_area = scroll_area.vertical_scroll_offset(next_offset);
                            ui.ctx().request_repaint();
                        } else {
                            ui.data_mut(|d| d.remove::<f32>(anim_scroll_id));
                        }
                    }

                    let note_font_sz = self.app_state.settings.note_font_size.clamp(10.0, 28.0);
                    let edit_bottom_pad = note_font_sz * 3.5;
                    let mut pending_scroll_px = 0.0_f32;

                    let scroll_out = scroll_area.show(ui, |ui| match self.note_mode {
                        NoteMode::Edit => {
                            if self.focus_note_editor {
                                ui.ctx().memory_mut(|m| m.request_focus(text_edit_id));
                                self.focus_note_editor = false;
                            }
                            // Do not claim or rewrite the clipboard when the editor gains focus.
                            // Ctrl+V must read the current system clipboard.

                            let mut copied_or_cut_text: Option<String> = None;
                            if ui.ctx().memory(|m| m.has_focus(text_edit_id)) {
                                let mut has_paste_event = false;
                                let mut fallback_v_release = false;
                                let mut has_copy_or_cut = false;

                                let ctrl_recent = screenshots::ctrl_recently_held(ui.ctx());
                                ui.input(|i| {
                                    let ctrl =
                                        i.modifiers.ctrl || i.modifiers.command || ctrl_recent;
                                    for ev in &i.events {
                                        match ev {
                                            egui::Event::Paste(_) => {
                                                has_paste_event = true;
                                                self.paste_handled_on_press = true;
                                            }
                                            egui::Event::Copy | egui::Event::Cut => {
                                                has_copy_or_cut = true;
                                            }
                                            egui::Event::Key {
                                                key: egui::Key::V,
                                                pressed: false,
                                                modifiers,
                                                ..
                                            } if (modifiers.ctrl || modifiers.command || ctrl) => {
                                                if self.paste_handled_on_press {
                                                    self.paste_handled_on_press = false;
                                                } else {
                                                    fallback_v_release = true;
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                });

                                let selected_slice: Option<String> = if has_copy_or_cut {
                                    egui::text_edit::TextEditState::load(ui.ctx(), text_edit_id)
                                        .and_then(|state| state.cursor.char_range())
                                        .and_then(|range| {
                                            let min =
                                                range.primary.index.min(range.secondary.index);
                                            let max =
                                                range.primary.index.max(range.secondary.index);
                                            if min != max {
                                                Some(
                                                    self.current_note
                                                        .chars()
                                                        .skip(min.into())
                                                        .take((max - min).into())
                                                        .collect::<String>(),
                                                )
                                            } else {
                                                None
                                            }
                                        })
                                } else {
                                    None
                                };

                                if has_paste_event || fallback_v_release {
                                    // 1. Read text immediately through arboard when available.
                                    let sys_text = self.clipboard.get_text();
                                    let is_klipper_img = sys_text
                                        .as_deref()
                                        .is_some_and(screenshots::is_image_placeholder);

                                    if let Some(sys_text) = sys_text.filter(|_| !is_klipper_img) {
                                        // Plain text: use the original fast path.
                                        ui.input_mut(|i| {
                                            if has_paste_event {
                                                for ev in &mut i.events {
                                                    if let egui::Event::Paste(t) = ev {
                                                        *t = sys_text.clone();
                                                    }
                                                }
                                            } else {
                                                i.events.push(egui::Event::Paste(sys_text));
                                            }
                                        });
                                    } else {
                                        // If no text is available, try to paste an image in the background.
                                        ui.input_mut(|i| {
                                            i.events.retain(|e| !matches!(e, egui::Event::Paste(_)))
                                        });
                                        let cursor = egui::text_edit::TextEditState::load(
                                            ui.ctx(),
                                            text_edit_id,
                                        )
                                        .and_then(|s| s.cursor.char_range())
                                        .map(|r| r.primary.index)
                                        .unwrap_or_else(|| {
                                            eframe::egui::text::CharIndex(
                                                self.current_note.chars().count(),
                                            )
                                        });
                                        self.shots.request_clipboard(
                                            ui.ctx(),
                                            ShotTarget::Note {
                                                page: self.note_page,
                                                cursor: cursor.into(),
                                            },
                                        );
                                    }
                                }

                                if has_copy_or_cut {
                                    copied_or_cut_text = selected_slice;
                                }
                            }

                            let text_col = rgb(self.theme.fg_primary);
                            let err_stroke =
                                egui::Stroke::new(1.3, egui::Color32::from_rgb(240, 85, 85));
                            let checker_ref = &self.spellchecker;

                            // Search hits of this note (char ranges) painted by the layouter.
                            let note_hl: Vec<(usize, usize, bool)> = if self.show_search {
                                self.search_hits
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, h)| {
                                        h.source == search::Source::Note && h.page == self.note_page
                                    })
                                    .map(|(i, h)| (h.start, h.end, i == self.search_sel))
                                    .collect()
                            } else {
                                Vec::new()
                            };
                            let hl_warn = rgb(self.theme.accent);
                            let hl_bg = (
                                egui::Color32::from_rgba_unmultiplied(
                                    hl_warn.r(),
                                    hl_warn.g(),
                                    hl_warn.b(),
                                    150,
                                ),
                                egui::Color32::from_rgba_unmultiplied(
                                    hl_warn.r(),
                                    hl_warn.g(),
                                    hl_warn.b(),
                                    60,
                                ),
                            );

                            // A finite desired width is essential here: INFINITY disables
                            // automatic wrapping and causes the editor to be clipped when the
                            // resizable panel becomes narrower.
                            let editor_width = ui.available_width().max(1.0);

                            let mut layouter =
                                |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap_width: f32| {
                                    let s = text.as_str();
                                    // Code and LaTeX are never spell-checked.
                                    let skip = markdown::code_and_math_ranges(s);
                                    let mut job = egui::text::LayoutJob::default();
                                    job.wrap.max_width = wrap_width.max(1.0);
                                    // Match Preview's proportional body font so ordinary
                                    // paragraphs wrap at nearly the same points in both modes.
                                    let font_id = egui::FontId::proportional(note_font_sz);

                                    // Char ranges of the search hits -> byte ranges of `s`.
                                    let hl_bytes: Vec<(usize, usize, bool)> = if note_hl.is_empty()
                                    {
                                        Vec::new()
                                    } else {
                                        let idx: Vec<usize> = s
                                            .char_indices()
                                            .map(|(i, _)| i)
                                            .chain(std::iter::once(s.len()))
                                            .collect();
                                        note_hl
                                            .iter()
                                            .filter(|(a, b, _)| a < b && *b < idx.len())
                                            .map(|&(a, b, cur)| (idx[a], idx[b], cur))
                                            .collect()
                                    };

                                    let mut last_end = 0;
                                    for (idx, ch) in s.char_indices() {
                                        if !ch.is_alphabetic() && ch != '\'' {
                                            if idx > last_end {
                                                let word = &s[last_end..idx];
                                                let ok = markdown::in_ranges(&skip, last_end)
                                                    || checker_ref.check_word(word);
                                                append_highlighted(
                                                    &mut job,
                                                    s,
                                                    last_end,
                                                    idx,
                                                    egui::TextFormat {
                                                        font_id: font_id.clone(),
                                                        color: text_col,
                                                        underline: if ok {
                                                            egui::Stroke::NONE
                                                        } else {
                                                            err_stroke
                                                        },
                                                        ..Default::default()
                                                    },
                                                    &hl_bytes,
                                                    hl_bg,
                                                );
                                            }
                                            let ch_end = idx + ch.len_utf8();
                                            append_highlighted(
                                                &mut job,
                                                s,
                                                idx,
                                                ch_end,
                                                egui::TextFormat {
                                                    font_id: font_id.clone(),
                                                    color: text_col,
                                                    ..Default::default()
                                                },
                                                &hl_bytes,
                                                hl_bg,
                                            );
                                            last_end = ch_end;
                                        }
                                    }
                                    if last_end < s.len() {
                                        let word = &s[last_end..];
                                        let ok = markdown::in_ranges(&skip, last_end)
                                            || checker_ref.check_word(word);
                                        append_highlighted(
                                            &mut job,
                                            s,
                                            last_end,
                                            s.len(),
                                            egui::TextFormat {
                                                font_id,
                                                color: text_col,
                                                underline: if ok {
                                                    egui::Stroke::NONE
                                                } else {
                                                    err_stroke
                                                },
                                                ..Default::default()
                                            },
                                            &hl_bytes,
                                            hl_bg,
                                        );
                                    }
                                    ui.fonts_mut(|f| f.layout_job(job))
                                };

                            // Right-click can collapse TextEdit's range during `show`.
                            // Snapshot the persisted range BEFORE the widget processes the click.
                            let selection_id = ui.make_persistent_id("note_context_selection");
                            let right_press = ui.input(|i| {
                                i.pointer.button_pressed(egui::PointerButton::Secondary)
                            });
                            let selection_before_click = if right_press {
                                egui::text_edit::TextEditState::load(ui.ctx(), text_edit_id)
                                    .and_then(|st| st.cursor.char_range())
                                    .map(|range| {
                                        (
                                            usize::from(
                                                range.primary.index.min(range.secondary.index),
                                            ),
                                            usize::from(
                                                range.primary.index.max(range.secondary.index),
                                            ),
                                        )
                                    })
                            } else {
                                None
                            };

                            let text_edit = egui::TextEdit::multiline(&mut self.current_note)
                                .id(text_edit_id)
                                .desired_width(editor_width)
                                .desired_rows(4)
                                .frame(egui::Frame::NONE)
                                .margin(egui::Margin::ZERO)
                                .layouter(&mut layouter)
                                .font(egui::FontId::proportional(note_font_sz))
                                .lock_focus(true)
                                .hint_text("Write your Markdown & $LaTeX$ notes here...");

                            let output = text_edit.show(ui);
                            if right_press && output.response.hovered() {
                                // Store on mouse *press*: `secondary_clicked` fires on release,
                                // after TextEdit may already have collapsed its selection.
                                let range = selection_before_click.unwrap_or((0, 0));
                                ui.data_mut(|d| d.insert_temp(selection_id, range));
                            }

                            // Bring the current search hit into view.
                            if self.search_note_scroll {
                                self.search_note_scroll = false;
                                if let Some(h) = self.search_hits.get(self.search_sel)
                                    && h.source == search::Source::Note
                                    && h.page == self.note_page
                                {
                                    let local = output
                                        .galley
                                        .pos_from_cursor(egui::text::CCursor::new(h.start));
                                    let r = local.translate(output.galley_pos.to_vec2());
                                    ui.scroll_to_rect(
                                        r.expand2(egui::vec2(0.0, r.height() * 3.0)),
                                        Some(egui::Align::Center),
                                    );
                                }
                            }

                            // Spazio extra sotto il TextEdit per mantenere lo scrolloff anche sull'ultima riga
                            ui.add_space(edit_bottom_pad);

                            if output.response.changed() {
                                self.note_is_dirty = true;
                                self.sync_and_save();
                            }

                            // Scrolloff: mantiene ~2.5 righe visibili sopra e sotto il cursore
                            let nav_or_edit = output.response.changed()
                                || ui.input(|i| {
                                    i.key_pressed(egui::Key::ArrowDown)
                                        || i.key_pressed(egui::Key::ArrowUp)
                                        || i.key_pressed(egui::Key::ArrowLeft)
                                        || i.key_pressed(egui::Key::ArrowRight)
                                        || i.key_pressed(egui::Key::Enter)
                                        || i.key_pressed(egui::Key::PageDown)
                                        || i.key_pressed(egui::Key::PageUp)
                                });

                            if output.response.has_focus()
                                && nav_or_edit
                                && let Some(range) = output.cursor_range
                            {
                                let local_caret = output.galley.pos_from_cursor(range.primary);
                                let row_h = local_caret.height().max(note_font_sz * 1.35);
                                let caret_screen =
                                    local_caret.translate(output.galley_pos.to_vec2());

                                let margin_y = row_h * 2.5;
                                let padded_rect = egui::Rect::from_min_max(
                                    egui::pos2(caret_screen.min.x, caret_screen.min.y - margin_y),
                                    egui::pos2(caret_screen.max.x, caret_screen.max.y + margin_y),
                                );
                                let clip_bot = ui.clip_rect().bottom();
                                if padded_rect.max.y > clip_bot {
                                    pending_scroll_px = padded_rect.max.y - clip_bot;
                                }
                                ui.scroll_to_rect(padded_rect, None);
                            }

                            let ctx_id = ui.make_persistent_id("spell_ctx_word");
                            // The response's cursor range may already be collapsed by the
                            // secondary click, so prefer the pre-widget snapshot. A collapsed
                            // range also records the insertion point for Paste.
                            if output.response.secondary_clicked() {
                                // Retain the range captured on mouse-down. Do not substitute
                                // `output.cursor_range`, which reflects the collapsed selection.
                                if ui
                                    .data(|d| d.get_temp::<(usize, usize)>(selection_id))
                                    .is_none()
                                {
                                    let range = selection_before_click.unwrap_or((0, 0));
                                    ui.data_mut(|d| d.insert_temp(selection_id, range));
                                }
                            }

                            if output.response.secondary_clicked() {
                                let skip = markdown::code_and_math_ranges(&self.current_note);
                                let picked: Option<(usize, usize, String)> = ui
                                    .input(|i| i.pointer.interact_pos())
                                    .and_then(|p| {
                                        let rel = p - output.galley_pos;
                                        let idx = output.galley.cursor_from_pos(rel).index;
                                        word_at_char_index(&self.current_note, idx.into())
                                    })
                                    .filter(|(start, _, w)| {
                                        // Char index -> byte offset, to look the word up in `skip`.
                                        let byte = self
                                            .current_note
                                            .char_indices()
                                            .nth(*start)
                                            .map_or(self.current_note.len(), |(b, _)| b);
                                        !markdown::in_ranges(&skip, byte)
                                            && !self.spellchecker.check_word(w)
                                    });

                                ui.data_mut(|d| match &picked {
                                    Some(v) => {
                                        d.insert_temp(ctx_id, v.clone());
                                    }
                                    None => {
                                        d.remove::<(usize, usize, String)>(ctx_id);
                                    }
                                });

                                if let Some((s, e, _)) = &picked {
                                    let mut st = output.state.clone();
                                    st.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                                        egui::text::CCursor::new(*s),
                                        egui::text::CCursor::new(*e),
                                    )));
                                    st.store(ui.ctx(), text_edit_id);
                                }
                            }

                            let ctx_word: Option<(usize, usize, String)> =
                                ui.data(|d| d.get_temp(ctx_id));
                            let mut spell_action: Option<SpellAction> = None;
                            let mut context_clipboard: Option<&'static str> = None;
                            let (selection_start, selection_end) = ui.data(|d| {
                                d.get_temp::<(usize, usize)>(selection_id).unwrap_or((0, 0))
                            });
                            let has_selection = selection_end > selection_start;
                            let clipboard_menu =
                                |ui: &mut egui::Ui, action: &mut Option<&'static str>| {
                                    if has_selection {
                                        if ui.button("󰆏  Copy").clicked() {
                                            *action = Some("copy");
                                            ui.close();
                                        }
                                        if ui.button("󰆴  Cut").clicked() {
                                            *action = Some("cut");
                                            ui.close();
                                        }
                                    }

                                    if ui.button("󰆒  Paste").clicked() {
                                        *action = Some("paste");
                                        ui.close();
                                    }
                                };

                            if let Some((_, _, word)) = &ctx_word {
                                let glass_style = self.menu_glass().modifier();
                                egui::Popup::context_menu(&output.response)
                                    .style(glass_style)
                                    .show(|ui| {
                                        // Keep the popup as compact as the other context menus.
                                        // A right-to-left layout here expands to the popup's available
                                        // width (often a large fraction of the notes pane).
                                        ui.set_width(180.0);
                                        clipboard_menu(ui, &mut context_clipboard);
                                        ui.separator();
                                        ui.label(
                                            egui::RichText::new(format!("“{}”", word))
                                                .weak()
                                                .size(12.0),
                                        );
                                        ui.separator();

                                        let cache_id = ui.make_persistent_id("spell_sugg_cache");
                                        let active_languages =
                                            self.spellchecker.loaded_languages.join("|");
                                        let cached: Option<(
                                            String,
                                            String,
                                            Vec<(String, String)>,
                                        )> = ui.data(|d| d.get_temp(cache_id));
                                        let sugg = match cached {
                                            Some((w, langs, v))
                                                if w == *word && langs == active_languages =>
                                            {
                                                v
                                            }
                                            _ => {
                                                let v = self
                                                    .spellchecker
                                                    .suggest_with_languages(word, 5);
                                                ui.data_mut(|d| {
                                                    d.insert_temp(
                                                        cache_id,
                                                        (word.clone(), active_languages, v.clone()),
                                                    )
                                                });
                                                v
                                            }
                                        };

                                        if sugg.is_empty() {
                                            ui.weak("No suggestions");
                                        }
                                        for (suggestion, language) in &sugg {
                                            ui.horizontal(|ui| {
                                                let (rect, response) = ui.allocate_exact_size(
                                                    egui::vec2(164.0, 22.0),
                                                    egui::Sense::click(),
                                                );
                                                let visuals = ui.style().interact(&response);
                                                if response.hovered() || response.has_focus() {
                                                    ui.painter().rect_filled(
                                                        rect,
                                                        3.0,
                                                        visuals.bg_fill,
                                                    );
                                                }
                                                ui.painter().text(
                                                    egui::pos2(rect.left() + 8.0, rect.center().y),
                                                    egui::Align2::LEFT_CENTER,
                                                    suggestion,
                                                    egui::FontId::proportional(14.0),
                                                    visuals.text_color(),
                                                );
                                                if response.clicked() {
                                                    spell_action = Some(SpellAction::Replace(
                                                        suggestion.clone(),
                                                    ));
                                                    ui.close();
                                                }
                                                ui.label(
                                                    egui::RichText::new(language).weak().size(10.0),
                                                );
                                            });
                                        }

                                        ui.separator();
                                        if ui.button("󰐕  Add to dictionary").clicked() {
                                            spell_action = Some(SpellAction::AddToDict);
                                            ui.close();
                                        }
                                        if ui.button("󰈉  Ignore (this session)").clicked() {
                                            spell_action = Some(SpellAction::Ignore);
                                            ui.close();
                                        }
                                    });
                            }

                            if ctx_word.is_none() {
                                let glass_style = self.menu_glass().modifier();
                                egui::Popup::context_menu(&output.response)
                                    .style(glass_style)
                                    .show(|ui| {
                                        ui.set_min_width(150.0);
                                        clipboard_menu(ui, &mut context_clipboard);
                                    });
                            }

                            if let Some(action) = context_clipboard {
                                let mut state =
                                    egui::text_edit::TextEditState::load(ui.ctx(), text_edit_id)
                                        .unwrap_or_default();
                                let len = self.current_note.chars().count();
                                let start = selection_start.min(len);
                                let end = selection_end.min(len).max(start);
                                {
                                    let selected: String = self
                                        .current_note
                                        .chars()
                                        .skip(start)
                                        .take(end - start)
                                        .collect();
                                    if (action == "copy" || action == "cut") && !selected.is_empty()
                                    {
                                        self.clipboard.set_text(&selected);
                                    }
                                    let replacement = match action {
                                        "paste" => self.clipboard.get_text().filter(|text| {
                                            !screenshots::is_image_placeholder(text)
                                        }),
                                        "cut" if start < end => Some(String::new()),
                                        _ => None,
                                    };
                                    if let Some(replacement) = replacement {
                                        replace_char_range(
                                            &mut self.current_note,
                                            start,
                                            end,
                                            &replacement,
                                        );
                                        state.cursor.set_char_range(Some(
                                            egui::text::CCursorRange::one(
                                                egui::text::CCursor::new(
                                                    start + replacement.chars().count(),
                                                ),
                                            ),
                                        ));
                                        state.store(ui.ctx(), text_edit_id);
                                        self.note_is_dirty = true;
                                        self.sync_and_save();
                                    }
                                }
                                ui.data_mut(|d| d.remove::<(usize, usize)>(selection_id));
                                ui.ctx().memory_mut(|m| m.request_focus(text_edit_id));
                                ui.ctx().request_repaint();
                            }

                            if let (Some(act), Some((s, e, word))) = (spell_action, ctx_word) {
                                match act {
                                    SpellAction::Replace(new_word) => {
                                        replace_char_range(&mut self.current_note, s, e, &new_word);
                                        self.note_is_dirty = true;
                                        self.sync_and_save();

                                        let mut st = egui::text_edit::TextEditState::load(
                                            ui.ctx(),
                                            text_edit_id,
                                        )
                                        .unwrap_or_default();
                                        st.cursor.set_char_range(Some(
                                            egui::text::CCursorRange::one(
                                                egui::text::CCursor::new(
                                                    s + new_word.chars().count(),
                                                ),
                                            ),
                                        ));
                                        st.store(ui.ctx(), text_edit_id);
                                    }
                                    SpellAction::AddToDict => {
                                        self.spellchecker.add_user_word(&word)
                                    }
                                    SpellAction::Ignore => self.spellchecker.ignore_word(&word),
                                }
                                ui.data_mut(|d| d.remove::<(usize, usize, String)>(ctx_id));
                                ui.ctx().memory_mut(|m| m.request_focus(text_edit_id));
                                ui.ctx().request_repaint();
                            }

                            if let Some(text) = copied_or_cut_text {
                                self.clipboard.set_text(&text);
                            }
                        }

                        // Handled before the scroll area (the sheet has its own scrolling).
                        NoteMode::Sketch => {}

                        NoteMode::Preview => {
                            if self.current_note.trim().is_empty() {
                                ui.vertical_centered(|ui| {
                                    ui.add_space(40.0);
                                    ui.label(
                                        egui::RichText::new("󰦨")
                                            .size(28.0)
                                            .color(rgb(self.theme.fg_muted)),
                                    );
                                    ui.add_space(6.0);
                                    ui.weak(
                                        "No notes for this page. Switch to Edit to start writing.",
                                    );
                                });
                            } else {
                                // Reflow Markdown and scale embedded images to the live panel
                                // width so they do not retain a wider layout while resizing.
                                let preview_width = ui.available_width().max(1.0);
                                ui.take_available_width();
                                let preview_context_press = ui.input(|i| {
                                    i.pointer.button_pressed(egui::PointerButton::Secondary)
                                        && i.pointer
                                            .hover_pos()
                                            .is_some_and(|p| ui.clip_rect().contains(p))
                                });
                                ui.scope(|ui| {
                                    let style = ui.style_mut();
                                    style.visuals.override_text_color = None;
                                    style.visuals.widgets.noninteractive.fg_stroke =
                                        egui::Stroke::new(1.0, rgb(self.theme.fg_primary));
                                    style.visuals.widgets.active.fg_stroke =
                                        egui::Stroke::new(2.0, rgb(self.theme.accent));
                                    style.visuals.hyperlink_color = rgb(self.theme.accent);
                                    style.visuals.code_bg_color = rgb(self.theme.bg_widget);
                                    style.visuals.extreme_bg_color = rgb(self.theme.bg_canvas);
                                    style.spacing.item_spacing.y = 7.0;
                                    style.interaction.selectable_labels = true;

                                    style.text_styles.insert(
                                        egui::TextStyle::Body,
                                        egui::FontId::proportional(note_font_sz),
                                    );
                                    style.text_styles.insert(
                                        egui::TextStyle::Monospace,
                                        egui::FontId::monospace(note_font_sz * 0.92),
                                    );
                                    style.text_styles.insert(
                                        egui::TextStyle::Heading,
                                        egui::FontId::proportional(note_font_sz * 1.35),
                                    );

                                    let rendered_md =
                                        flashcards::preprocess_notes_preview(&self.current_note);

                                    // Capture the label selection on secondary press, before
                                    // clicking the popup (a primary click clears label selection).
                                    // The result is cached until the user chooses Copy.
                                    if preview_context_press {
                                        ui.input_mut(|i| i.events.push(egui::Event::Copy));
                                    }

                                    egui_commonmark::CommonMarkViewer::new()
                                        .max_image_width(Some(preview_width.ceil() as usize))
                                        .show(ui, &mut self.commonmark_cache, &rendered_md);

                                    ui.add_space(Self::PREVIEW_BOTTOM_PAD);
                                });
                                // CommonMark uses selectable labels; egui owns their selection.
                                // The menu's Copy command delegates to egui's native text-selection action.
                                let preview_response = ui.interact(
                                    ui.min_rect(),
                                    ui.scope_id().with("notes_preview_context"),
                                    egui::Sense::click(),
                                );
                                let glass_style = self.menu_glass().modifier();
                                egui::Popup::context_menu(&preview_response)
                                    .style(glass_style)
                                    .show(|ui| {
                                        if ui.button("󰆏  Copy").clicked() {
                                            let selection_id = ui.make_persistent_id(
                                                "lapis_preview_context_copied_text",
                                            );
                                            if let Some(selected) = ui
                                                .data(|d| d.get_temp::<String>(selection_id))
                                                .or_else(|| self.clipboard.get_text())
                                            {
                                                self.clipboard.set_text(&selected);
                                            }
                                            ui.close();
                                        }
                                    });

                                let copied_texts = ui.ctx().output_mut(|output| {
                                    let mut copied = Vec::new();
                                    output.commands.retain(|command| {
                                        if let egui::OutputCommand::CopyText(text) = command {
                                            copied.push(text.clone());
                                            false
                                        } else {
                                            true
                                        }
                                    });
                                    copied
                                });

                                if preview_context_press {
                                    let selected = copied_texts.join("\n");
                                    let selection_id =
                                        ui.make_persistent_id("lapis_preview_context_copied_text");
                                    ui.data_mut(|d| {
                                        if selected.is_empty() {
                                            d.remove::<String>(selection_id);
                                        } else {
                                            d.insert_temp(selection_id, selected);
                                        }
                                    });
                                } else {
                                    for text in copied_texts {
                                        self.clipboard.set_text(&text);
                                    }
                                }
                            }
                        }
                    });

                    ui.data_mut(|d| {
                        d.insert_temp(
                            scroll_id.with("last_offset"),
                            scroll_out.state.clamped_offset().y,
                        );
                    });

                    let viewport_rect = scroll_out.inner_rect;
                    let content_h = scroll_out.content_size.y;
                    let view_h = viewport_rect.height();
                    let offset_y = scroll_out.state.clamped_offset().y;

                    // Trailing empty space, so the arrow also clears with the audio-button allowance.
                    let extra_pad = if is_edit {
                        edit_bottom_pad
                    } else {
                        Self::PREVIEW_BOTTOM_PAD
                    };
                    let extra_pad_audio = if self.app_state.settings.audio_dock
                        || self.app_state.settings.audio_dock
                    {
                        Self::AUDIO_BTN_SIZE
                    } else {
                        0.0
                    }; //add floating buttons size
                    let remaining_px = content_h
                        - (offset_y + pending_scroll_px + view_h + extra_pad)
                        + extra_pad_audio;
                    if remaining_px > note_font_sz / 2.0 {
                        let alpha = (remaining_px + note_font_sz / 35.0).clamp(0.25, 0.95);
                        let elevated = rgb(self.theme.bg_elevated);
                        let accent = rgb(self.theme.accent);
                        let foreground = rgb(self.theme.fg_primary);

                        let bg_col = egui::Color32::from_rgba_unmultiplied(
                            elevated.r(),
                            elevated.g(),
                            elevated.b(),
                            (alpha * 235.0) as u8,
                        );

                        let border_col = egui::Color32::from_rgba_unmultiplied(
                            accent.r(),
                            accent.g(),
                            accent.b(),
                            (alpha * 180.0) as u8,
                        );

                        let text_col = egui::Color32::from_rgba_unmultiplied(
                            foreground.r(),
                            foreground.g(),
                            foreground.b(),
                            (alpha * 255.0) as u8,
                        );

                        let fade_h = 24.0_f32.min(view_h * 0.25);
                        let fade_rect = egui::Rect::from_min_max(
                            egui::pos2(viewport_rect.left(), viewport_rect.bottom() - fade_h),
                            viewport_rect.right_bottom(),
                        );
                        let mut mesh = egui::Mesh::default();
                        let top_col = egui::Color32::TRANSPARENT;
                        let fade_base = if is_edit {
                            rgb(self.theme.bg_widget)
                        } else {
                            rgb(self.theme.bg_elevated)
                        };

                        let bot_col = egui::Color32::from_rgba_unmultiplied(
                            fade_base.r(),
                            fade_base.g(),
                            fade_base.b(),
                            (alpha * 200.0) as u8,
                        );
                        mesh.colored_vertex(fade_rect.left_top(), top_col);
                        mesh.colored_vertex(fade_rect.right_top(), top_col);
                        mesh.colored_vertex(fade_rect.right_bottom(), bot_col);
                        mesh.colored_vertex(fade_rect.left_bottom(), bot_col);
                        mesh.add_triangle(0, 1, 2);
                        mesh.add_triangle(0, 2, 3);
                        ui.painter()
                            .add(egui::Shape::Mesh(std::sync::Arc::new(mesh)));

                        let pill_size = egui::vec2(34.0, 22.0);
                        let pill_center =
                            egui::pos2(viewport_rect.center().x, viewport_rect.bottom() - 15.0);
                        let pill_rect = egui::Rect::from_center_size(pill_center, pill_size);

                        let resp = ui.interact(
                            pill_rect,
                            scroll_id.with("more_below_btn"),
                            egui::Sense::click(),
                        );

                        let pill_bg = if resp.hovered() {
                            let active = rgb(self.theme.bg_active);
                            egui::Color32::from_rgba_unmultiplied(
                                active.r(),
                                active.g(),
                                active.b(),
                                245,
                            )
                        } else {
                            bg_col
                        };

                        ui.painter().rect(
                            pill_rect,
                            egui::CornerRadius::same(11),
                            pill_bg,
                            egui::Stroke::new(1.0, border_col),
                            egui::epaint::StrokeKind::Inside,
                        );

                        ui.painter().text(
                            pill_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "󰁅",
                            egui::FontId::proportional(14.0),
                            text_col,
                        );

                        if resp.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }

                        if resp.clicked() {
                            let target = (offset_y + view_h * 0.7).min(content_h - view_h);
                            ui.data_mut(|d| d.insert_temp(anim_scroll_id, target));
                            ui.ctx().request_repaint();
                        }
                    }
                });
        }

        // ---------------------------------------------------------
        // B. Bottom area: voice memos and lesson audio.
        // ---------------------------------------------------------
        if total_bottom_h > 0.0 {
            let mut bottom_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(bottom_rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            bottom_ui.set_clip_rect(bottom_rect);
            bottom_ui.spacing_mut().item_spacing.y = 0.0;

            // 1. Voice memos, when available.
            if has_page_memos {
                let memo_rect = egui::Rect::from_min_size(
                    bottom_rect.min,
                    egui::vec2(bottom_rect.width(), memo_h),
                );
                let mut memo_ui = bottom_ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(memo_rect)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                egui::Frame::NONE
                    .fill(panel_bg)
                    .stroke(egui::Stroke::NONE)
                    .inner_margin(egui::Margin::symmetric(8, 2))
                    .show(&mut memo_ui, |u| {
                        self.render_page_voice_memos_strip(u);
                    });
            }

            // 2. Audio Dock o Ticker
            if dock_h > 0.0 {
                let dock_top = bottom_rect.bottom() - dock_h;

                let dock_rect = egui::Rect::from_min_max(
                    egui::pos2(bottom_rect.left(), dock_top),
                    bottom_rect.max,
                );
                let mut dock_ui = bottom_ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(dock_rect)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                self.render_audio_dock_in_rect(&mut dock_ui, show_ticker);
            }
        }

        // ---------------------------------------------------------
        // C. Floating buttons when no voice memos are shown.
        // ---------------------------------------------------------
        if !has_page_memos {
            self.render_floating_audio_fabs(ui.ctx(), bottom_rect, &media_dir);
        }

        // ---------------------------------------------------------
        // D. Final panel UI allocation.
        // ---------------------------------------------------------
        // Panel::bottom saves `ui.min_rect()` to PanelState at frame end.
        // `new_child` does not move the parent cursor
        // or add item spacing, so expand min_rect to panel_rect.
        // This preserves the height selected by dragging
        // without automatic growth or reset to min_content_h.
        ui.allocate_rect(panel_rect, egui::Sense::hover());
    }

    pub(in crate::workspace) fn render_slideshow_notes_contents(
        &mut self,
        ui: &mut egui::Ui,
        active_slide_note: String,
    ) {
        ui.take_available_width();
        ui.add_space(self.sz(6.0));
        // A frameless header keeps the note pane aligned with the rest of the topbar.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(
                egui::RichText::new("󰈙  Notes")
                    .size(13.0)
                    .strong()
                    .color(rgb(self.theme.fg_primary)),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let icon = if self.lock_note_page { "󰌾" } else { "󰌿" };
                if ui
                    .add_sized(
                        [self.sz(26.0), self.sz(26.0)],
                        crate::theme::selected_button(
                            egui::RichText::new(icon).size(13.0),
                            self.lock_note_page,
                            ui.visuals(),
                        )
                        .corner_radius(egui::CornerRadius::same(6)),
                    )
                    .on_hover_text("Pin or unpin the current note while browsing slides")
                    .clicked()
                {
                    self.lock_note_page = !self.lock_note_page;
                }
                ui.label(
                    egui::RichText::new(format!("{} / {}", self.note_page, self.total_pages))
                        .size(11.5)
                        .color(rgb(self.theme.fg_muted)),
                );
            });
        });
        ui.add_space(8.0);

        // Case 1: pinned note.
        if self.lock_note_page && !self.current_note.trim().is_empty() {
            let note_clone = self.current_note.clone();
            let pinned_p = self.note_page;
            let accent_col = rgb(self.theme.accent);
            self.render_markdown_preview_with_arrow(
                ui,
                "slideshow_notes_scroll",
                &note_clone,
                |ui| {
                    ui.label(
                        egui::RichText::new(format!("󰌾 Pinned Note from Slide {}", pinned_p))
                            .size(12.0)
                            .color(accent_col),
                    );
                    ui.add_space(6.0);
                },
            );
            return;
        }

        // Case 2: the current slide has a Markdown note.
        if !active_slide_note.is_empty() {
            self.render_markdown_preview_with_arrow(
                ui,
                "slideshow_notes_scroll",
                &active_slide_note,
                |_| {},
            );
            return;
        }

        // Case 3: handle an empty slide using `SlideshowEmptyMode`.
        match self.app_state.settings.slideshow_empty_mode {
            SlideshowEmptyMode::AutoHold => {
                if let Some((prev_p, prev_note)) = self.find_previous_note(self.current_page) {
                    let bg_elev = rgb(self.theme.bg_elevated);
                    let accent_col = rgb(self.theme.accent);
                    self.render_markdown_preview_with_arrow(ui, "slideshow_notes_scroll", &prev_note, |ui| {
                                        egui::Frame::NONE
                                            .fill(bg_elev)
                                            .corner_radius(egui::CornerRadius::same(6))
                                            .inner_margin(egui::Margin::symmetric(10, 6))
                                            .show(ui, |ui| {
                                                ui.label(
                                                    egui::RichText::new(format!("󰌾 Auto-holding notes from Slide {} (current slide has no notes)", prev_p))
                                                        .size(11.5)
                                                        .color(accent_col),
                                                );
                                            });
                                        ui.add_space(8.0);
                                    });
                } else {
                    ui.weak("No previous notes to hold yet.");
                }
            }

            SlideshowEmptyMode::ContextDashboard => {
                // 3B. Show audio transcript for this slide + Up Next preview
                let slide_segs: Vec<_> = self
                    .audio_index
                    .segments
                    .iter()
                    .filter(|s| s.page == self.current_page)
                    .cloned()
                    .collect();

                if !slide_segs.is_empty() {
                    ui.label(
                        egui::RichText::new("󰗊 Lecture Audio Transcript")
                            .strong()
                            .size(13.5)
                            .color(rgb(self.theme.accent)),
                    );
                    ui.add_space(4.0);
                    for seg in &slide_segs {
                        let lbl = format!("[{}]  {}", audio::format_ms(seg.start_ms), seg.text);
                        if ui
                            .add(
                                egui::Button::new(egui::RichText::new(lbl).size(12.0)).frame(false),
                            )
                            .clicked()
                        {
                            self.audio_engine.seek_to_ms(seg.start_ms);
                        }
                    }
                    ui.add_space(12.0);
                    ui.separator();
                }

                if let Some((next_p, next_note)) = self.find_next_note(self.current_page) {
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(format!("󰒭 Up Next: Notes on Slide {}", next_p))
                            .strong()
                            .size(13.0)
                            .color(rgb(self.theme.fg_muted)),
                    );
                    ui.add_space(4.0);
                    let preview_lines: String =
                        next_note.lines().take(6).collect::<Vec<_>>().join("\n");
                    let rendered = flashcards::preprocess_notes_preview(&preview_lines);
                    egui_commonmark::CommonMarkViewer::new().show(
                        ui,
                        &mut self.commonmark_cache,
                        &rendered,
                    );
                } else if slide_segs.is_empty() {
                    ui.weak("No notes or audio transcript for this slide.");
                }
            }

            SlideshowEmptyMode::SkipEmpty
            | SlideshowEmptyMode::StaticEmpty
            | SlideshowEmptyMode::ExpandFull => {
                ui.weak("No notes for this slide.");
            }
        }
        ui.allocate_space(ui.available_size());
    }
}

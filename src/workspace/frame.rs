use super::*;
use egui::scroll_area::{DragScroll, ScrollSource};

impl eframe::App for Lapis {
    fn on_exit(&mut self) {
        if !self.allow_window_close && !self.finish_audio_before_transition() {
            eprintln!("Lapis: could not finalize audio on forced exit; recording files retained");
        }
        self.audio_engine.shutdown_output();
        self.memo_audio_engine.shutdown_output();
        if self.allow_window_close {
            return;
        }
        // The normal close path joins before issuing Close. Handle forced shutdown too.
        if let Some(job) = self.pdf_save_job.take() {
            let _ = job.worker.join();
        }
        self.finish_sketch_edit();
        self.sync_and_save();
        if self.has_pdf() {
            let result = AudioDocIndex::recover_page_change(&self.pdf_path).and_then(|()| {
                notes::commit_doc_data(
                    &self.pdf_path,
                    &self.notes,
                    &self.annotations,
                    self.blank_pages.clone(),
                    &self.sketches,
                )
            });
            if let Err(err) = result {
                eprintln!("Lapis: failed to commit PDF annotations on exit: {err}");
            }
        }
        self.save_session_state();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let win_rect = ui.ctx().content_rect();
        ui.ctx().data_mut(|d| {
            d.remove::<(egui::LayerId, egui::Rect)>(egui::Id::unique("annotation_cursor_target"))
        });

        // Keep the egui style in step with the interface density (a theme change resets it).
        let density = self.app_state.settings.ui_density;
        if !self.theme.density_applied(ui.ctx(), density) {
            self.theme.apply(ui.ctx());
            self.theme.apply_density(ui.ctx(), density);
            ui.ctx().request_repaint();
        }

        if self.service_pdf_save(ui) {
            return;
        }
        self.show_signed_page_modal(ui);

        // Persist view mode and panel sizes once the pointer is released.
        let pointer_down = ui.input(|i| i.pointer.any_down());
        if !pointer_down && self.notes_panel_resizing {
            self.notes_panel_resizing = false;
            ui.ctx().request_repaint();
        }
        if self.has_pdf()
            && !pointer_down
            && (self.panel_sizes_dirty || self.reading_view_mode() != self.saved_view_mode)
        {
            self.panel_sizes_dirty = false;
            self.save_session_state();
        }
        let use_bottom_notes =
            self.app_state.settings.bottom_notes && win_rect.height() > win_rect.width();
        if let Some((kind, result)) = self.dialogs.poll() {
            let dialog_ctx = ui.ctx().clone();
            self.handle_dialog_result(kind, result, &dialog_ctx);
        }
        if self.pending_delete_page.is_some() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.pending_delete_page = None;
        }
        let lesson_poll_pending = self.audio_engine.poll();
        let memo_poll_pending = self.memo_audio_engine.poll();
        let audio_active = self.audio_engine.is_playing()
            || self.audio_engine.is_recording()
            || lesson_poll_pending
            || self.memo_audio_engine.is_playing()
            || memo_poll_pending;
        if audio_active {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));

            if self.audio_engine.is_playing() && self.audio_engine.follow_pdf {
                let pos = self.audio_engine.current_pos_ms();
                if let Some(synced_page) = self.audio_index.page_at_time(pos)
                    && synced_page != self.current_page
                    && (1..=self.total_pages).contains(&synced_page)
                {
                    self.go_to_page(synced_page, ui.ctx());
                }
            }
        }
        if let Ok(result) = self.export_rx.try_recv() {
            self.is_exporting = false;
            match result {
                Ok(path) => {
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    self.toast_message =
                        Some((format!("󰄬 Exported: {}", name), std::time::Instant::now()));
                }
                Err(err) => {
                    self.toast_message = Some((
                        format!("󰅚 Export error: {}", err),
                        std::time::Instant::now(),
                    ));
                }
            }
        }
        while let Ok(msg) = self.asr_rx.try_recv() {
            match msg {
                audio::AsrJobResult::Lecture {
                    segments,
                    done_ms,
                    total_ms,
                    finished,
                } => {
                    let n = segments.len();
                    self.audio_index.segments = segments;
                    self.audio_index_pending = true;
                    if self.audio_index.timeline_is_live {
                        self.audio_index.assign_pages_from_timeline();
                    }
                    if finished {
                        self.is_transcribing = false;
                        self.asr_progress = None;
                        if !self.audio_index.timeline_is_live {
                            self.transcript_all = true;
                        }
                        self.report_audio_index_result(format!(
                            "󰄬 Transcribed {n} segments. Use Align to map them to slides"
                        ));
                    } else {
                        self.asr_progress = Some((done_ms, total_ms));
                    }
                }
                audio::AsrJobResult::Aligned {
                    segments,
                    timeline,
                    range,
                } => {
                    self.is_transcribing = false;
                    let n = segments.len();
                    self.audio_index.segments = segments;
                    self.audio_index.timeline = timeline;
                    self.audio_index.timeline_is_live = false;
                    self.audio_index.page_range = Some(range);
                    self.transcript_all = false;
                    self.report_audio_index_result(format!(
                        "󰄬 Aligned {n} segments to pages {}–{}",
                        range.0, range.1
                    ));
                }
                audio::AsrJobResult::Memo { filename, text } => {
                    self.is_transcribing = false;
                    self.asr_progress = None;
                    if self.audio_index.set_memo_transcript(&filename, text) {
                        self.report_audio_index_result("󰄬 Voice memo transcribed!");
                    }
                }
                audio::AsrJobResult::Cancelled => {
                    self.is_transcribing = false;
                    self.asr_progress = None;
                    self.report_audio_index_result(
                        "󰅖 Transcription cancelled (partial transcript kept)",
                    );
                }
                audio::AsrJobResult::Error(err) => {
                    self.is_transcribing = false;
                    self.asr_progress = None;
                    if self.audio_index_pending {
                        self.report_audio_index_result(format!(
                            "󰅚 Transcription: {err} (partial transcript kept)"
                        ));
                    } else {
                        self.audio_toast(format!("󰅚 Transcription: {err}"));
                    }
                }
            }
        }

        if let Some(render) = self.viewer.upload_ready(ui.ctx(), self.render_view()) {
            let (handle, dims) = render.texture;
            self.texture_handle = Some(handle);
            self.page_dims = dims;
            self.page_pt = render.page_pt;
            self.is_rendering = false;
        }

        let sel_ctx = ui.ctx().clone();
        self.update_text_drag(&sel_ctx);

        // Global Hotkeys & Zoom handling (processed before widgets so they work reliably)
        let text_edit_id = ui.make_persistent_id("note_text_edit");
        let any_focused = ui.ctx().memory(|m| m.focused().is_some());
        let note_editor_focused = ui.ctx().memory(|m| m.has_focus(text_edit_id));

        // Leaving the Sketch tab (or hiding the notes) finishes an open text box.
        if !self.sketch_enabled() && self.note_mode == NoteMode::Sketch {
            self.note_mode = NoteMode::Edit;
        }
        let sketch_visible = self.show_notes && self.note_mode == NoteMode::Sketch;
        if !sketch_visible {
            self.sketch.hovered = false;
            self.finish_sketch_edit();
        }
        // Pinch / Ctrl + wheel over the sheet zooms the sheet, not the PDF.
        let sketch_hover = sketch_visible && self.sketch.hovered;

        let mut act_prev_page = false;
        let mut act_next_page = false;
        let mut act_save = false;
        let mut act_toggle_notes = false;
        let mut act_toggle_mode = false;
        let mut act_view_mode: Option<ViewMode> = None;
        let mut act_toggle_sketch = false;
        let mut act_toggle_topbar = false;
        let mut act_fit_page = false;
        let mut act_fit_pdf_page = false;
        let mut act_tool = None;
        let mut act_shape = None;
        let mut act_fit_width = false;
        let mut act_toggle_night = false;
        let mut act_toggle_tools = false;
        let mut act_toggle_toc = false;
        let mut act_toggle_fullscreen = false;
        let mut copy_triggered = false;

        if !self.show_settings && self.rebinding_action.is_none() {
            ui.input_mut(|i| {
                shortcuts::track_key_presses(i);
                // 1. Trackpad pinch or Ctrl + wheel zoom
                let z_delta = i.zoom_delta();
                if sketch_hover {
                    // Handled by the sketch sheet.
                } else if (z_delta - 1.0).abs() > 1e-4 {
                    self.enter_custom_zoom();
                    let old = self.zoom_scale;
                    self.zoom_scale = (self.zoom_scale * z_delta).clamp(0.3, 3.0);
                    if (self.zoom_scale - old).abs() > 1e-4 {
                        self.zoom_anchor = i.pointer.hover_pos().or_else(|| i.pointer.latest_pos());
                    }
                    i.smooth_scroll_delta = egui::Vec2::ZERO;
                } else if (i.modifiers.ctrl || i.modifiers.command)
                    && i.smooth_scroll_delta.y.abs() > 0.0
                {
                    self.enter_custom_zoom();
                    let old = self.zoom_scale;
                    let step = (i.smooth_scroll_delta.y * 0.002).clamp(-0.15, 0.15);
                    self.zoom_scale = (self.zoom_scale + step).clamp(0.3, 3.0);
                    if (self.zoom_scale - old).abs() > 1e-4 {
                        self.zoom_anchor = i.pointer.hover_pos().or_else(|| i.pointer.latest_pos());
                    }
                    i.smooth_scroll_delta = egui::Vec2::ZERO;
                }

                // 2. Ctrl + Plus / Minus / 0 for PDF Zoom
                if i.consume_shortcut(&egui::KeyboardShortcut::new(
                    egui::Modifiers::COMMAND,
                    egui::Key::Plus,
                )) || i.consume_shortcut(&egui::KeyboardShortcut::new(
                    egui::Modifiers::COMMAND,
                    egui::Key::Equals,
                )) {
                    self.enter_custom_zoom();
                    self.zoom_scale = (self.zoom_scale + 0.1).min(3.0);
                }
                if i.consume_shortcut(&egui::KeyboardShortcut::new(
                    egui::Modifiers::COMMAND,
                    egui::Key::Minus,
                )) {
                    self.enter_custom_zoom();
                    self.zoom_scale = (self.zoom_scale - 0.1).max(0.3);
                }
                if i.consume_shortcut(&egui::KeyboardShortcut::new(
                    egui::Modifiers::COMMAND,
                    egui::Key::Num0,
                )) {
                    self.zoom_scale = 1.0;
                    self.zoom_mode = ZoomMode::Custom;
                }

                // Configurable workspace and annotation shortcuts.
                let sc = &self.app_state.shortcuts;
                let text_context = any_focused
                    || self.show_search
                    || self.show_command_palette
                    || self.show_flashcards_modal;
                let allowed =
                    |binding: &ShortcutBinding| !text_context || binding.ctrl || binding.alt;
                if allowed(&sc.save_notes) && sc.save_notes.consume_once(i) {
                    act_save = true;
                }
                if allowed(&sc.toggle_edit_preview) && sc.toggle_edit_preview.consume_once(i) {
                    act_toggle_mode = true;
                }
                if allowed(&sc.toggle_notes) && sc.toggle_notes.consume_once(i) {
                    act_toggle_notes = true;
                }
                if allowed(&sc.toggle_sketch) && sc.toggle_sketch.consume_once(i) {
                    act_toggle_sketch = true;
                }
                if allowed(&sc.fit_page) && sc.fit_page.consume_once(i) {
                    act_fit_page = true;
                }
                if !text_context && self.has_pdf() {
                    let mut consume = |binding: &ShortcutBinding| binding.consume_once(i);
                    act_fit_pdf_page = consume(&sc.fit_pdf_page);
                    act_fit_width = consume(&sc.fit_width);
                    act_toggle_night = consume(&sc.toggle_night);
                    act_toggle_tools = consume(&sc.toggle_tools);
                    for (binding, tool) in [
                        (&sc.select_cursor, AnnotationTool::Cursor),
                        (&sc.select_pen, AnnotationTool::Draw),
                        (&sc.select_highlighter, AnnotationTool::Highlight),
                        (&sc.select_textbox, AnnotationTool::TextBox),
                        (&sc.select_underline, AnnotationTool::Underline),
                        (&sc.select_eraser, AnnotationTool::Eraser),
                        (&sc.select_snip, AnnotationTool::Snip),
                        (&sc.select_tape, AnnotationTool::Tape),
                    ] {
                        if consume(binding) {
                            act_tool = Some(tool);
                        }
                    }
                    for (binding, shape) in [
                        (&sc.select_rectangle, ShapeKind::Rect),
                        (&sc.select_circle, ShapeKind::Circle),
                        (&sc.select_line, ShapeKind::Line),
                        (&sc.select_arrow, ShapeKind::Arrow),
                    ] {
                        if consume(binding) {
                            act_shape = Some(shape);
                        }
                    }
                }
                if allowed(&sc.toggle_topbar) && sc.toggle_topbar.consume_once(i) {
                    act_toggle_topbar = true;
                }
                if allowed(&sc.toggle_outline) && sc.toggle_outline.consume_once(i) {
                    act_toggle_toc = true;
                }
                // Configured page navigation and fixed PageUp/PageDown controls.
                if allowed(&sc.prev_slide_global) && sc.prev_slide_global.consume(i)
                    || i.consume_shortcut(&egui::KeyboardShortcut::new(
                        egui::Modifiers::NONE,
                        egui::Key::PageUp,
                    ))
                {
                    act_prev_page = true;
                }
                if allowed(&sc.next_slide_global) && sc.next_slide_global.consume(i)
                    || i.consume_shortcut(&egui::KeyboardShortcut::new(
                        egui::Modifiers::NONE,
                        egui::Key::PageDown,
                    ))
                {
                    act_next_page = true;
                }

                // 5. Standard Left/Right or K/J when not typing in a text box and annotation tools
                if !text_context {
                    if !i.modifiers.any() {
                        if i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::K) {
                            act_prev_page = true;
                        }
                        if i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::J) {
                            act_next_page = true;
                        }
                    }
                    if self.app_state.settings.radial_menu_enabled
                        && self.app_state.shortcuts.radial_tools_menu.consume_once(i)
                    {
                        self.radial_menu_pos = if self.radial_menu_pos.is_some() {
                            None
                        } else {
                            i.pointer
                                .hover_pos()
                                .or_else(|| Some(i.content_rect().center()))
                        };
                    }
                }
                // F11 works while typing only when it is the configured fullscreen binding.
                {
                    let b = &self.app_state.shortcuts.toggle_fullscreen;
                    let by_binding = (allowed(b) || b.key == "F11") && b.consume_once(i);
                    if by_binding {
                        act_toggle_fullscreen = true;
                    }
                }
                // 6. View Mode Shortcuts: Alt+1 (Single), Alt+2 (Continuous), Alt+3 (TwoPage), Alt+4 (Slideshow), Alt+4 (Slideshow+Notes), Esc (Exit Slideshow)
                if allowed(&sc.view_single) && sc.view_single.consume_once(i) {
                    act_view_mode = Some(ViewMode::SinglePage);
                }
                if allowed(&sc.view_continuous) && sc.view_continuous.consume_once(i) {
                    act_view_mode = Some(ViewMode::Continuous);
                }
                if allowed(&sc.view_twopage) && sc.view_twopage.consume_once(i) {
                    self.view_mode = ViewMode::TwoPage;
                }
                if allowed(&sc.view_slideshow) && sc.view_slideshow.consume_once(i) {
                    if self.view_mode != ViewMode::Slideshow
                        && self.view_mode != ViewMode::SlideshowWithNotes
                    {
                        self.prev_view_mode = self.view_mode;
                    }
                    self.view_mode = ViewMode::Slideshow;
                }
                if allowed(&sc.view_slideshow_notes) && sc.view_slideshow_notes.consume_once(i) {
                    if self.view_mode != ViewMode::Slideshow
                        && self.view_mode != ViewMode::SlideshowWithNotes
                    {
                        self.prev_view_mode = self.view_mode;
                    }
                    self.view_mode = ViewMode::SlideshowWithNotes;
                }

                if (self.view_mode == ViewMode::Slideshow
                    || self.view_mode == ViewMode::SlideshowWithNotes)
                    && i.key_pressed(egui::Key::Escape)
                {
                    self.view_mode = self.prev_view_mode;
                }
                // Toggle Continuous <-> Single page (default: C). Plain key: only when not typing,
                // and never while a Ctrl/Alt chord (e.g. Ctrl+C copy) is held.
                let in_show = matches!(
                    self.view_mode,
                    ViewMode::Slideshow | ViewMode::SlideshowWithNotes
                );
                if !text_context && !in_show && self.has_pdf() {
                    let b = &self.app_state.shortcuts.toggle_continuous_single;
                    if b.consume_once(i) {
                        act_view_mode = Some(if self.view_mode == ViewMode::Continuous {
                            ViewMode::SinglePage
                        } else {
                            ViewMode::Continuous
                        });
                    }
                }
                // 7. Continuous Viewmode arrows shortcuts
                if !any_focused && self.view_mode == ViewMode::Continuous {
                    if i.key_pressed(egui::Key::ArrowDown) {
                        self.continuous_scroll_delta -= 120.0;
                    }
                    if i.key_pressed(egui::Key::ArrowUp) {
                        self.continuous_scroll_delta += 120.0;
                    }
                }
                // Copy selected PDF text when the note editor is not focused.
                if !note_editor_focused {
                    let has_copy_ev = i.events.iter().any(|e| matches!(e, egui::Event::Copy));
                    if has_copy_ev
                        || i.consume_shortcut(&egui::KeyboardShortcut::new(
                            egui::Modifiers::COMMAND,
                            egui::Key::C,
                        ))
                    {
                        copy_triggered = true;
                    }
                }
                // 9. CTRL+Z for tools annotations
                let mut do_undo = false;
                let mut do_redo = false;
                if !any_focused && self.rebinding_action.is_none() {
                    let is_cmd = i.modifiers.command;
                    if is_cmd && i.key_pressed(egui::Key::Z) {
                        if i.modifiers.shift {
                            do_redo = true;
                        } else {
                            do_undo = true;
                        }
                    } else if is_cmd && i.key_pressed(egui::Key::Y) {
                        do_redo = true;
                    }
                }

                if do_undo {
                    self.undo_annotation(self.current_page);
                } else if do_redo {
                    self.redo_annotation(self.current_page);
                }
                //10. Search
                if allowed(&self.app_state.shortcuts.search_notes)
                    && self.app_state.shortcuts.search_notes.consume_once(i)
                {
                    if self.show_search {
                        self.close_search();
                    } else {
                        self.open_search();
                    }
                }

                //11. Command palette
                //11. Command palette
                let toggle_cmd_palette = i.modifiers.command
                    && (i.key_pressed(egui::Key::P) || i.key_pressed(egui::Key::K));
                if toggle_cmd_palette {
                    self.show_command_palette = !self.show_command_palette;
                    if self.show_command_palette {
                        self.command_palette_query.clear();
                        self.command_palette_selected = 0;
                    }
                }
            });
        }
        if let Some(mode) = act_view_mode {
            self.set_view_mode(mode);
        }
        if copy_triggered && let Some(text) = self.selection_text() {
            self.clipboard.set_text(&text);
        }

        if act_save {
            self.request_pdf_save(AfterPdfSave::Stay, ui.ctx());
        }
        if act_toggle_toc {
            self.show_toc = !self.show_toc;
        }
        if act_toggle_notes {
            self.show_notes = !self.show_notes;
            if self.show_notes {
                self.note_mode = NoteMode::Edit;
                self.focus_note_editor = !self.app_state.shortcuts.toggle_notes.is_plain();
            }
        }
        if act_toggle_sketch {
            self.toggle_sketch_tab();
        }
        if let Some(tool) = act_tool {
            self.annotation_tool = tool;
        }
        if let Some(shape) = act_shape {
            self.shape_kind = shape;
            self.annotation_tool = AnnotationTool::Shape;
        }
        if act_fit_width {
            self.zoom_mode = ZoomMode::FitWidth;
            self.zoom_anchor = None;
        }
        if act_toggle_night {
            self.pdf_dark_mode = !self.pdf_dark_mode;
            self.viewer.invalidate_images();
            self.save_session_state();
        }
        if act_toggle_tools {
            self.show_tools_rail = !self.show_tools_rail;
            if !self.show_tools_rail && self.app_state.settings.reset_tool_on_bar_close {
                self.annotation_tool = AnnotationTool::Cursor;
            }
        }
        if act_fit_pdf_page {
            self.zoom_mode = ZoomMode::FitPage;
            self.zoom_anchor = None;
        }
        if act_fit_page && !use_bottom_notes {
            self.request_fit_notes_layout(ui.ctx());
        }
        if act_toggle_topbar && self.has_pdf() {
            self.toggle_topbar();
        }
        if act_toggle_mode && self.show_notes {
            self.note_mode = match self.note_mode {
                NoteMode::Edit => NoteMode::Preview,
                NoteMode::Preview | NoteMode::Sketch => NoteMode::Edit,
            };
            if self.note_mode == NoteMode::Edit {
                self.focus_note_editor = true;
            }
        }
        if act_prev_page {
            self.prev_page(ui.ctx());
            if note_editor_focused {
                self.focus_note_editor = true;
            }
        }
        if act_next_page {
            self.next_page(ui.ctx());
            if note_editor_focused {
                self.focus_note_editor = true;
            }
        }

        // Top Panel
        let in_slideshow = matches!(
            self.view_mode,
            ViewMode::Slideshow | ViewMode::SlideshowWithNotes
        );
        let shot_ctx = ui.ctx().clone();
        self.process_screenshots(&shot_ctx, any_focused, in_slideshow);
        // Fullscreen is wanted in slideshow modes and whenever the user asked for it.
        // F11 does nothing in slideshow (Esc exits it); the user choice is kept for afterwards.
        if act_toggle_fullscreen && !in_slideshow {
            self.user_fullscreen = !self.user_fullscreen;
        }
        let wanted_fullscreen = in_slideshow || self.user_fullscreen;
        let actual_fullscreen = ui.input(|i| i.viewport().fullscreen);
        if self.fullscreen_sent != Some(wanted_fullscreen) {
            // Edge-triggered: send the command only when the wanted state changes.
            // Re-sending it every frame fights tiling compositors (e.g. Hyprland), which
            // report the new state asynchronously or let the user leave fullscreen themselves.
            if self.fullscreen_sent.is_some()
                || actual_fullscreen.unwrap_or(false) != wanted_fullscreen
            {
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Fullscreen(wanted_fullscreen));
                self.fullscreen_cmd_at = Some(std::time::Instant::now());
                ui.ctx().request_repaint();
            }
            self.fullscreen_sent = Some(wanted_fullscreen);
        } else if !in_slideshow {
            // Fullscreen changed from outside (compositor keybinding, window manager, ...):
            // follow the real state instead of forcing ours back.
            let settled = self
                .fullscreen_cmd_at
                .is_none_or(|t| t.elapsed() > std::time::Duration::from_millis(800));
            if let Some(actual) = actual_fullscreen
                && settled
                && actual != self.user_fullscreen
            {
                self.user_fullscreen = actual;
                self.fullscreen_sent = Some(actual);
            }
        }

        if in_slideshow
            && self
                .fullscreen_cmd_at
                .is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(800))
        {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(16));
        }

        if !in_slideshow && self.has_pdf() {
            let topbar_slot = self.topbar_slot(ui);
            // The topbar itself, drawn into whichever ui the slot provides.
            let overlay_at_bottom =
                self.app_state.settings.topbar_position == state::TopbarPosition::Bottom;
            let bar = self.app_state.settings.ui_density.bar();
            let overlay_height = bar.row_h + 2.0 * f32::from(bar.margin_y);
            let mut draw_topbar = |ui: &mut egui::Ui| -> f32 {
                // Popups opened from the topbar set this again while they are open.
                self.topbar_popup_open = false;
                // Density sets the row height, margins and button padding.
                let density = self.app_state.settings.ui_density;
                let bar = density.bar();
                let (bar_h, margin_y, pad_y) = (bar.row_h, bar.margin_y, bar.pad_y);
                let at_bottom =
                    self.app_state.settings.topbar_position == state::TopbarPosition::Bottom;
                let panel = if at_bottom {
                    egui::Panel::bottom("top_panel")
                } else {
                    egui::Panel::top("top_panel")
                };
                let panel_out = panel
            .frame(
                egui::Frame::NONE
                    .fill(rgb(self.theme.bg_panel))
                    .inner_margin(egui::Margin::symmetric(8, margin_y)),
            )
            .show(ui, |ui| {
                if density != state::UiDensity::Standard {
                    ui.spacing_mut().button_padding.y = pad_y;
                }
                let sq_btn = egui::vec2(bar_h, bar_h);


            let bar_rect = ui.max_rect();
                let bar_w = bar_rect.width();
                // Fixed-height toolbar row: left group, right group, and title.
                // are positioned independently at the row edges and center.
                let row_rect = egui::Rect::from_min_size(bar_rect.min, egui::vec2(bar_w, bar_h));

                // Compactness stage (0 = expanded, 4 = minimal), based on the previous frame.
                let stage_id = ui.make_persistent_id("topbar_stage");
                let widths_id = ui.make_persistent_id("topbar_stage_widths");
                let stage = ui.data(|d| d.get_temp::<usize>(stage_id).unwrap_or(0)).min(3);
                let menu_id = ui.make_persistent_id("file_menu_open_state");

                let mut left_end_x = row_rect.left();
                let mut right_start_x = row_rect.right();
                let mut left_more_rect = egui::Rect::NOTHING;
                let mut bm_anchor = egui::Rect::NOTHING;

                // Left control group.
                ui.horizontal(|ui| {
                    ui.set_height(bar_h);

                    // Outline toggle
                    let toc_arrow = if self.show_toc { "" } else { "" };
                    let toc_btn = crate::theme::selected_button(format!(" {}", toc_arrow), self.show_toc, ui.visuals())
                        .min_size(egui::vec2(44.0, bar_h));
                    if ui
                        .add(toc_btn)
                        .on_hover_text(self.app_state.shortcuts.toggle_outline.hint("Toggle Outline"))
                        .clicked()
                    {
                        self.show_toc = !self.show_toc;
                    }

                    ui.separator();

                    // Zoom controls
                    if stage < 3
                        && ui
                            .add(egui::Button::new("󰍴").min_size(sq_btn))
                            .on_hover_text("Zoom Out [Ctrl+-]")
                            .clicked()
                        {
                            self.enter_custom_zoom();
                            self.zoom_scale = (self.zoom_scale - 0.1).max(0.3);
                        }

                    let zoom_label = match self.zoom_mode {
                        ZoomMode::FitWidth => "󰤼 Fit Width".to_string(),
                        ZoomMode::FitHeight => "󰤻 Fit Height".to_string(),
                        ZoomMode::FitPage => "󰊓 Fit Page".to_string(),
                        ZoomMode::Custom => format!("󰍉 {:.0}%", self.zoom_scale * 100.0),
                    };

                    let zoom_menu = Self::glass_menu_button(ui, self.menu_glass(), zoom_label, |ui| {
                    ui.set_min_width(140.0);
                    if ui.button("Zoom In [Ctrl++]").clicked() {
                        self.enter_custom_zoom();
                        self.zoom_scale = (self.zoom_scale + 0.1).min(3.0);
                    }
                    if ui.button("Zoom Out [Ctrl+-]").clicked() {
                        self.enter_custom_zoom();
                        self.zoom_scale = (self.zoom_scale - 0.1).max(0.3);
                    }
                    ui.separator();
                    if ui
                        .selectable_label(self.zoom_mode == ZoomMode::FitWidth, self.app_state.shortcuts.fit_width.hint("󰤼  Fit Width"))
                        .clicked()
                    {
                        self.zoom_mode = ZoomMode::FitWidth;
                        ui.close();
                    }
                    if ui
                        .selectable_label(
                            self.zoom_mode == ZoomMode::FitHeight,
                            "󰤻  Fit Height",
                        )
                        .clicked()
                    {
                        self.zoom_mode = ZoomMode::FitHeight;
                        ui.close();
                    }
                    if ui
                        .selectable_label(self.zoom_mode == ZoomMode::FitPage, self.app_state.shortcuts.fit_pdf_page.hint("󰊓  Fit Page"))
                        .clicked()
                    {
                        self.zoom_mode = ZoomMode::FitPage;
                        ui.close();
                    }
                    ui.separator();
                    if ui
                        .selectable_label(
                            self.zoom_mode == ZoomMode::Custom
                                && (self.zoom_scale - 1.0).abs() < 0.01,
                            "󰑐  100% [Ctrl+0]",
                        )
                        .clicked()
                    {
                        self.zoom_scale = 1.0;
                        self.zoom_mode = ZoomMode::Custom;
                        ui.close();
                    }
                });

                let resp = zoom_menu.response.on_hover_text(format!("Zoom Mode [Ctrl+Scroll] | Right Click: {} | {}", self.app_state.shortcuts.fit_pdf_page.hint("Fit Page"), self.app_state.shortcuts.fit_width.hint("Fit Width")));

                if resp.hovered() && ui.input(|i| i.pointer.button_clicked(egui::PointerButton::Secondary)) {
                    self.zoom_mode = ZoomMode::FitPage;
                }

                    if stage < 3
                        && ui
                            .add(egui::Button::new("󰐕").min_size(sq_btn))
                            .on_hover_text("Zoom In [Ctrl++]")
                            .clicked()
                        {
                            self.enter_custom_zoom();
                            self.zoom_scale = (self.zoom_scale + 0.1).min(3.0);
                        }

                    ui.separator();

                    if stage < 2 {
                        self.topbar_view_mode_menu(ui);
                        self.topbar_split_button(ui, sq_btn, false);
                        self.topbar_dark_button(ui, sq_btn, false);
                    } else {
                        let more = Self::glass_menu_button(ui, self.menu_glass(), "⋯", |ui| {
                            ui.set_min_width(200.0);
                            if stage >= 4 {
                                // At minimum width, the File menu moves here.
                                if ui
                                    .button("󰈦  File…")
                                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                                    .clicked()
                                {
                                    ui.data_mut(|d| {
                                        d.insert_temp(menu_id, true);
                                        d.insert_temp(ui.make_persistent_id("file_menu_skip_outside"), true);
                                    });
                                    ui.close();
                                }
                                ui.separator();
                            }
                            self.topbar_view_mode_menu(ui);
                            self.topbar_split_button(ui, sq_btn, true);
                            self.topbar_dark_button(ui, sq_btn, true);
                        });
                        left_more_rect = more.response.rect;
                        more.response.on_hover_text("More");
                    }
                    left_end_x = ui.min_rect().right();
                });

                // Right group: notes, tools, flashcards, pages, search, bookmarks, and new page.
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(row_rect)
                        .layout(egui::Layout::right_to_left(egui::Align::Center)),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        ui.spacing_mut().button_padding = egui::vec2(6.0, pad_y);

                        // Notes
                        let page_has_note = page_has_notes_or_sketch(
                            self.current_page,
                            self.note_page,
                            &self.current_note,
                            &self.notes,
                            &self.sketches,
                        );

                        let notes_arrow = if self.show_notes { "" } else { "" };
                        let notes_btn = crate::theme::selected_button(format!("{} 󰠮 ", notes_arrow), self.show_notes, ui.visuals())
                            .min_size(egui::vec2(44.0, bar_h));

                        let resp = ui.add(notes_btn);

                        if page_has_note {
                            self.paint_topbar_dot(ui, resp.rect);
                        }

                        let has_notes_hover = if page_has_note {"\n(This page contains notes or a sketch)"} else {""};
                        if resp.on_hover_text(format!("{}{}", self.app_state.shortcuts.toggle_notes.hint("Toggle Notes Panel"), has_notes_hover)).clicked() {
                            self.show_notes = !self.show_notes;
                            if self.show_notes && self.note_mode == NoteMode::Edit {
                                self.focus_note_editor = true;
                            }
                        }
                        ui.separator();
                        let tools_on = self.app_state.settings.tools_button;
                        let flash_on = self.app_state.settings.flash_button;
                        if stage < 4 {
                            self.topbar_tools_button(ui, sq_btn, false);
                        }
                        if stage < 3 {
                            self.topbar_flash_button(ui, sq_btn, false);
                        }
                        if (stage < 4 && tools_on) || (stage < 3 && flash_on) {
                            ui.separator();
                        }

                        if ui
                            .add(egui::Button::new("󰁔").min_size(sq_btn))
                            .on_hover_text(self.app_state.shortcuts.next_slide_global.hint("Next Page [→ / J / PageDown]"))
                            .clicked()
                        {
                            self.next_page(ui.ctx());
                        }

                        ui.label(
                            egui::RichText::new(format!("| {}", self.total_pages))
                                .color(ui.visuals().weak_text_color()),
                        );

                        let response = ui
                            .add_sized(
                                [42.0, bar_h - 2.0],
                                egui::DragValue::new(&mut self.target_page)
                                    .range(1..=self.total_pages)
                                    .speed(0.1),
                            )
                            .on_hover_text("Current Page (Drag or click to type)");

                        if (response.lost_focus() || (response.changed() && !response.has_focus()))
                            && self.target_page != self.current_page {
                                self.go_to_page(self.target_page, ui.ctx());
                            }

                        if ui
                            .add(egui::Button::new("󰁍").min_size(sq_btn))
                            .on_hover_text(self.app_state.shortcuts.prev_slide_global.hint("Previous Page [← / K / PageUp]"))
                            .clicked()
                        {
                            self.prev_page(ui.ctx());
                        }
                        ui.separator();

                        if stage < 1 {
                            self.topbar_search_button(ui, sq_btn, false);
                            bm_anchor = self.topbar_bookmark_button(ui, sq_btn, false);
                            self.topbar_newpage_button(ui, sq_btn, false);
                        } else {
                            let more = Self::glass_menu_button(ui, self.menu_glass(), "⋯", |ui| {
                                ui.set_min_width(200.0);
                                self.topbar_search_button(ui, sq_btn, true);
                                self.topbar_bookmark_button(ui, sq_btn, true);
                                self.topbar_newpage_button(ui, sq_btn, true);
                                if stage >= 3 {
                                    self.topbar_flash_button(ui, sq_btn, true);
                                }
                                if stage >= 4 {
                                    self.topbar_tools_button(ui, sq_btn, true);
                                }
                            });
                            bm_anchor = more.response.rect;
                            more.response.on_hover_text("More tools");
                        }
                        self.render_bookmark_popup(ui, bm_anchor);
                        right_start_x = ui.min_rect().left();
                    },
                );

                // Center the title and move File into the left overflow menu when needed.
                const TITLE_GAP: f32 = 8.0;
                const TITLE_COMFORT_CHARS: usize = 12;
                const TITLE_MAX_CHARS: usize = 60;
                const ALLOW_TITLE_SLIDE: bool = true;

                let mut is_open = ui.data_mut(|d| *d.get_temp_mut_or_default::<bool>(menu_id));
                let arrow = if is_open { "▴" } else { "▾" };

                let font_id = egui::FontId::proportional(13.0);
                let make_label = |title: &str, arrow: &str| format!("  {}  {}", title, arrow);
                let label_w = |s: &str| -> f32 {
                    (ui.painter()
                        .layout_no_wrap(s.to_owned(), font_id.clone(), ui.visuals().text_color())
                        .size()
                        .x
                        + ui.spacing().button_padding.x * 2.5)
                        .ceil()
                };
                let title_for = |n: usize| -> String {
                    if self.pdf_name.is_empty() {
                        "[No PDF loaded]".to_string()
                    } else {
                        truncate_filename(&self.pdf_path, n)
                    }
                };

                // Measure both groups at the current compactness stage.
                let left_w = (left_end_x - row_rect.left()).max(0.0);
                let right_w = (row_rect.right() - right_start_x).max(0.0);
                let mut widths: Vec<f32> = ui
                    .data(|d| d.get_temp::<Vec<f32>>(widths_id))
                    .filter(|v| v.len() == 8)
                    .unwrap_or_else(|| vec![0.0; 8]);

                widths[stage] = left_w;
                widths[4 + stage] = right_w;

                // Estimate unmeasured stages from the current layout.
                const L_SAVE: [f32; 3] = [0.0, 160.0, 70.0]; // Left: stage s to s+1.
                const R_SAVE: [f32; 3] = [75.0, 0.0, 100.0]; // Right: stage s to s+1.
                let est = |s: usize| -> (f32, f32) {
                    if widths[s] > 0.0 && widths[4 + s] > 0.0 {
                        return (widths[s], widths[4 + s]);
                    }
                    let (mut l, mut r) = (left_w, right_w);
                    if s < stage {
                        for k in s..stage {
                            l += L_SAVE[k];
                            r += R_SAVE[k];
                        }
                    } else {
                        for k in stage..s {
                            l = (l - L_SAVE[k]).max(0.0);
                            r = (r - R_SAVE[k]).max(0.0);
                        }
                    }
                    (l, r)
                };

                let comfort_title_w = label_w(&make_label(&"n".repeat(TITLE_COMFORT_CHARS), "▾"));
                let need = |s: usize| -> f32 {
                    let (l, r) = est(s);

                    if ALLOW_TITLE_SLIDE {
                        l + r + comfort_title_w + 3.0 * TITLE_GAP
                    } else {
                        2.0 * l.max(r) + comfort_title_w + 2.0 * TITLE_GAP
                    }
                };

                let mut new_stage = stage;
                if stage < 3 && need(stage) > bar_w {
                    new_stage = stage + 1;
                } else if stage > 0 && need(stage - 1) + 24.0 <= bar_w {
                    new_stage = stage - 1; // 24 px hysteresis prevents layout oscillation.
                }
                ui.data_mut(|d| {
                    d.insert_temp(widths_id, widths.clone());
                    d.insert_temp(stage_id, new_stage);
                });
                if new_stage != stage {
                    ui.ctx().request_repaint();
                }

                // Use the longest title that fits; otherwise show only the icon.
                let center_x = row_rect.center().x;
                let sym_w = (2.0 * ((center_x - left_end_x).min(right_start_x - center_x) - TITLE_GAP)).max(0.0);
                let gap_w = (right_start_x - left_end_x - 2.0 * TITLE_GAP).max(0.0);
                let max_title_w = if ALLOW_TITLE_SLIDE { gap_w } else { sym_w };

                let chosen_title = (1..=TITLE_MAX_CHARS)
                    .rev()
                    .map(title_for)
                    .find(|t| label_w(&make_label(t.as_str(), "▾")) <= max_title_w)
                    .unwrap_or_default();

                let menu_label = make_label(&chosen_title, arrow);
                let measure_label = make_label(&chosen_title, "▾");
                let total_size = egui::vec2(label_w(&measure_label), bar_h);
                let half = total_size.x / 2.0;
                let lo = left_end_x + TITLE_GAP + half;
                let hi = right_start_x - TITLE_GAP - half;
                let title_fits = total_size.x <= max_title_w;

                let title_cx = if ALLOW_TITLE_SLIDE && title_fits {
                    center_x.clamp(lo, hi)
                } else {
                    center_x
                };

                let centered_rect = egui::Rect::from_center_size(
                    egui::pos2(title_cx, row_rect.center().y),
                    total_size,
                );

                let mut button_rect = egui::Rect::NOTHING;

                if !title_fits {
                    button_rect = left_more_rect;
                } else {
                    ui.scope_builder(egui::UiBuilder::new().max_rect(centered_rect), |ui| {
                        let btn_resp = ui
                            .add_sized(
                                total_size,
                                crate::theme::selected_button(
                                    egui::RichText::new(menu_label).font(font_id),
                                    is_open,
                                    ui.visuals(),
                                )
                                    .corner_radius(egui::CornerRadius::same(
                                        self.theme.corner_radius,
                                    )),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text(format!("File actions and settings\n{}", self.pdf_path.to_string_lossy()));

                        button_rect = btn_resp.rect;

                        if btn_resp.clicked() {
                            is_open = !is_open;
                        }
                    });
                }

                if is_open {
                    let mut close_requested = false;
                    let (popup_pos, popup_pivot) =
                        self.topbar_popup_placement(button_rect, title_fits);

                    let area_resp = egui::Area::new(ui.make_persistent_id("file_menu_area"))
                        .fixed_pos(popup_pos)
                        .pivot(popup_pivot)
                        .order(egui::Order::Foreground)
                        .show(ui.ctx(), |ui| {
                            self.modal_glass_frame(MENU_GLASS_ALPHA)
                                .inner_margin(egui::Margin::symmetric(6, 6))
                                .show(ui, |ui| {
                                    let menu_width = 232.0;
                                    let menu_text_muted = rgb(self.theme.fg_muted);
                                    let menu_radius = egui::CornerRadius::same(
                                        self.theme.corner_radius.saturating_sub(1),
                                    );
                                    ui.set_width(menu_width);
                                    ui.spacing_mut().item_spacing.y = 4.0;

                                    let menu_heading = |ui: &mut egui::Ui, label: &str| {
                                        ui.add_space(4.0);
                                        ui.label(
                                            egui::RichText::new(label)
                                                .size(10.0)
                                                .strong()
                                                .color(menu_text_muted),
                                        );
                                        ui.add_space(3.0);
                                    };
                                    // Computed outside the closure so it does not keep `self` borrowed.
                                    let menu_item_h = self.sz(26.0);
                                    let menu_item = |ui: &mut egui::Ui,
                                                     label: &str,
                                                     enabled: bool|
                                     -> egui::Response {
                                        let btn = egui::Button::new(
                                            egui::RichText::new(label).size(11.8),
                                        )
                                        .min_size(egui::vec2(menu_width, menu_item_h))
                                        .corner_radius(menu_radius);
                                        ui.add_enabled(enabled, btn)
                                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                                    };

                                    menu_heading(ui, "PREFERENCES");
                                    if menu_item(ui, "󰒓  Settings & customization...", true)
                                        .on_hover_text("Themes, export preferences and keyboard shortcuts")
                                        .clicked()
                                    {
                                        self.show_settings = true;
                                        self.rebinding_action = None;
                                        close_requested = true;
                                    }

                                    ui.separator();
                                    menu_heading(ui, "DOCUMENT");
                                    if menu_item(ui, "󰈦  Open PDF...", true).clicked() {
                                        self.dialogs.open(
                                            ui.ctx(),
                                            DialogKind::OpenPdf,
                                            DialogSpec::open_file().filter("PDF files", &["pdf"]),
                                        );
                                        close_requested = true;
                                    }

                                    if menu_item(ui, "󰅖  Close PDF · Return to Home", self.has_pdf())
                                        .on_hover_text("Close current document and return to Welcome Screen")
                                        .clicked()
                                    {
                                        self.request_pdf_save(AfterPdfSave::Home, ui.ctx());
                                        close_requested = true;
                                    }

                                    if menu_item(ui, "󰆓  Save Notes", true)
                                        .on_hover_text(self.app_state.shortcuts.save_notes.hint("Checkpoint locally and commit the PDF"))
                                        .clicked()
                                    {
                                        self.request_pdf_save(AfterPdfSave::Stay, ui.ctx());
                                        close_requested = true;
                                    }

                                    if menu_item(ui, "󰇚  Save Original PDF...", true)
                                        .on_hover_text("Save a copy of the original, unmodified PDF")
                                        .clicked()
                                    {
                                        self.prompt_download_original_pdf(ui.ctx());
                                        close_requested = true;
                                    }

                                    ui.separator();
                                    menu_heading(ui, "NOTES & STUDY");
                                    if menu_item(ui, "󰈦  Import Notes (.txt / .md)...", true)
                                        .on_hover_text("Import notes mapped by slide headers (e.g. # Slide 1)")
                                        .clicked()
                                    {
                                        self.import_notes_dialog(ui.ctx());
                                        close_requested = true;
                                    }

                                    if menu_item(ui, "󰚥  Export Raw Notes (.md)", true)
                                        .on_hover_text("Export all notes into a single Markdown file")
                                        .clicked()
                                    {
                                        self.export_raw_notes_dialog(ui.ctx());
                                        close_requested = true;
                                    }

                                    ui.separator();
                                    menu_heading(ui, "EXPORT");
                                    let export_label = if self.is_exporting {
                                        "󱑚  Exporting...".to_string()
                                    } else {
                                        format!(
                                            "󰚥  Export PDF ({})",
                                            self.app_state.export_settings.content.short_label()
                                        )
                                    };

                                    if menu_item(ui, &export_label, !self.is_exporting).clicked() {
                                        self.start_export(ui.ctx());
                                        close_requested = true;
                                    }

                                });
                        });

                    let skip_outside = ui
                        .data_mut(|d| d.remove_temp::<bool>(ui.make_persistent_id("file_menu_skip_outside")))
                        .unwrap_or(false);
                    let clicked_outside = !skip_outside
                        && ui.input(|i| i.pointer.any_click())
                        && ui
                            .input(|i| i.pointer.interact_pos())
                            .map(|pos| {
                                !button_rect.contains(pos) && !area_resp.response.rect.contains(pos)
                            })
                            .unwrap_or(false);

                    if close_requested
                        || clicked_outside
                        || ui.input(|i| i.key_pressed(egui::Key::Escape))
                    {
                        is_open = false;
                    }
                }

                ui.data_mut(|d| *d.get_temp_mut_or_default::<bool>(menu_id) = is_open);
                if is_open {
                    self.topbar_popup_open = true;
                }

            });
                panel_out.response.rect.height()
            };

            // Docked, revealed as an overlay (auto-hide), or hidden.
            let mut overlay_h = 0.0;
            let mut dock_h = 0.0;
            match topbar_slot {
                TopbarSlot::Docked => dock_h = draw_topbar(ui),
                TopbarSlot::Overlay => {
                    let ctx = ui.ctx().clone();
                    let screen = ctx.content_rect();
                    let area = egui::Area::new(ui.make_persistent_id("topbar_overlay"))
                        .order(egui::Order::Foreground)
                        .constrain(false)
                        .default_size(egui::vec2(screen.width(), overlay_height));
                    let area = if overlay_at_bottom {
                        area.anchor(egui::Align2::LEFT_BOTTOM, egui::Vec2::ZERO)
                    } else {
                        area.fixed_pos(screen.min)
                    };
                    let area = area.show(&ctx, |area_ui| {
                        area_ui.set_width(screen.width());
                        area_ui.set_max_height(overlay_height);
                        draw_topbar(area_ui)
                    });
                    overlay_h = area.inner;
                }
                TopbarSlot::Hidden => {}
            }
            self.topbar_overlay_h = overlay_h;
            self.topbar_dock_h = dock_h;
        } else {
            self.topbar_overlay_h = 0.0;
            self.topbar_dock_h = 0.0;
        }

        // Set again by the sub-topbar when it is drawn this frame.
        self.subbar_h = 0.0;
        if self.has_pdf() && self.view_mode != ViewMode::Slideshow {
            match self.app_state.settings.tools_layout {
                state::ToolsLayoutMode::SubTopbar => {
                    if self.show_tools_rail {
                        self.render_sub_topbar_tools(ui);
                    }
                }
                state::ToolsLayoutMode::VerticalRail => {
                    if self.show_tools_rail {
                        self.render_vertical_tools_rail(ui);
                    }
                }
            }
        }

        // Left Panel (Dual-Mode Sidebar: Thumbnails Grid & Outline/Bookmarks)
        if self.show_toc && self.has_pdf() && !in_slideshow {
            // Also re-sync when the tab changes: the hidden tab did not scroll while the page moved.
            let scroll_sidebar_to_current = self.last_synced_sidebar_page != self.current_page
                || self.last_synced_sidebar_tab != self.sidebar_tab;

            if scroll_sidebar_to_current {
                self.last_synced_sidebar_page = self.current_page;
                self.last_synced_sidebar_tab = self.sidebar_tab;
            }
            egui::Panel::left("toc_panel")
                .resizable(true)
                .default_size(170.0)
                .show(ui, |ui| {
                    // Top switcher: thumbnails or outline.
                    let tab_h = self.sz(26.0);
                    ui.add_space(self.sz(4.0));
                    ui.columns(2, |cols| {
                        if cols[0]
                            .add_sized(
                                [cols[0].available_width(), tab_h],
                                crate::theme::selected_button(
                                    " 󰕰 ",
                                    self.sidebar_tab == SidebarTab::Thumbnails,
                                    cols[0].visuals(),
                                ),
                            )
                            .on_hover_text("Thumbnails")
                            .clicked()
                        {
                            self.sidebar_tab = SidebarTab::Thumbnails;
                            cols[0].ctx().request_repaint(); // apply the sync next frame
                        }
                        if cols[1]
                            .add_sized(
                                [cols[1].available_width(), tab_h],
                                crate::theme::selected_button(
                                    "  ",
                                    self.sidebar_tab == SidebarTab::Outline,
                                    cols[1].visuals(),
                                ),
                            )
                            .on_hover_text("Outline and Bookmarks")
                            .clicked()
                        {
                            self.sidebar_tab = SidebarTab::Outline;
                            cols[1].ctx().request_repaint(); // apply the sync next frame
                        }
                    });
                    ui.separator();

                    let mut page_to_go: Option<u32> = None;
                    let mut toggle_bm_page: Option<u32> = None;
                    let mut remove_bm_page: Option<u32> = None;
                    let mut delete_page_action: Option<u32> = None;
                    let mut move_page_action: Option<(u32, u32)> = None;

                    match self.sidebar_tab {
                        SidebarTab::Thumbnails => {
                            let row_h = 116.0; // Compact row height
                            let total = self.total_pages as usize;
                            let mut thumb_rows: Option<(usize, usize)> = None;
                            let avail_h = ui.available_height();

                            let last_thumb_page_id =
                                ui.make_persistent_id("last_synced_thumb_page");
                            let prev_thumb_page: u32 = ui
                                .ctx()
                                .data_mut(|d| *d.get_temp_mut_or(last_thumb_page_id, 0u32));
                            let page_just_changed = prev_thumb_page != self.current_page;
                            if page_just_changed {
                                ui.ctx().data_mut(|d| {
                                    d.insert_temp(last_thumb_page_id, self.current_page)
                                });
                            }

                            ui.spacing_mut().item_spacing.y = 0.0;

                            let mut thumb_scroll = egui::ScrollArea::vertical()
                                .id_salt("thumbs_scroll")
                                .auto_shrink([false, false]);

                            if scroll_sidebar_to_current || page_just_changed {
                                let max_offset = (total as f32 * row_h - avail_h).max(0.0);

                                let ideal_y = (self.current_page.saturating_sub(1) as f32 * row_h)
                                    - (avail_h - row_h) * 0.5;

                                thumb_scroll = thumb_scroll
                                    .vertical_scroll_offset(ideal_y.clamp(0.0, max_offset));
                            }

                            // Create IDs on the same `Ui` and reuse them
                            // inside `show_rows` and after the ScrollArea.
                            // IDs created inside row UIs differ from IDs created outside them.
                            let drag_source_id = ui.make_persistent_id("thumb_drag_source_page");
                            let drag_target_id = ui.make_persistent_id("thumb_drag_target_page");

                            let pointer_pos = ui.input(|i| {
                                i.pointer.interact_pos().or_else(|| i.pointer.hover_pos())
                            });

                            thumb_scroll.show_rows(ui, row_h, total, |ui, row_range| {
                                thumb_rows = Some((row_range.start, row_range.end));
                                let avail_w = ui.available_width();

                                for idx in row_range {
                                    let p = (idx + 1) as u32;
                                    let is_current = p == self.current_page;
                                    let is_hidden = self.hidden_pages.contains(&p);
                                    let has_note = self
                                        .notes
                                        .get(&p)
                                        .map(|n| !n.trim().is_empty())
                                        .unwrap_or(false);
                                    let is_bm = self.user_bookmarks.iter().any(|b| b.page == p);

                                    // Allocate exact `row_h` so virtual scrolling never drifts!
                                    let (row_rect, _) = ui.allocate_exact_size(
                                        egui::vec2(avail_w, row_h),
                                        egui::Sense::hover(),
                                    );

                                    let card_w = (avail_w - 14.0).clamp(90.0, 156.0);
                                    let card_h = row_h - 6.0;
                                    let card_rect = egui::Rect::from_center_size(
                                        row_rect.center(),
                                        egui::vec2(card_w, card_h),
                                    );
                                    let card_resp = ui.interact(
                                        card_rect,
                                        ui.make_persistent_id(("thumb_card", p)),
                                        egui::Sense::click_and_drag(),
                                    );

                                    if card_resp.drag_started() {
                                        ui.ctx().data_mut(|d| {
                                            d.insert_temp(drag_source_id, p);
                                            d.insert_temp(drag_target_id, p);
                                        });
                                    }
                                    let drag_source: u32 =
                                        ui.ctx().data(|d| d.get_temp(drag_source_id).unwrap_or(0));
                                    if drag_source != 0
                                        && let Some(pos) =
                                            pointer_pos.filter(|pos| card_rect.contains(*pos))
                                    {
                                        // target = slot d'inserimento (da 1 a total_pages + 1).
                                        let target = if pos.y < card_rect.center().y {
                                            p
                                        } else {
                                            p.saturating_add(1).min(self.total_pages + 1)
                                        };
                                        ui.ctx()
                                            .data_mut(|d| d.insert_temp(drag_target_id, target));
                                    }

                                    let border_col = if is_current {
                                        rgb(self.theme.accent)
                                    } else if card_resp.hovered() {
                                        rgb(self.theme.fg_muted)
                                    } else {
                                        rgb(self.theme.border)
                                    };
                                    let bg_col = if is_current {
                                        rgb(self.theme.bg_active)
                                    } else {
                                        rgb(self.theme.bg_elevated)
                                    };

                                    ui.painter().rect_filled(
                                        card_rect,
                                        egui::CornerRadius::same(5),
                                        bg_col,
                                    );
                                    ui.painter().rect_stroke(
                                        card_rect,
                                        egui::CornerRadius::same(5),
                                        egui::Stroke::new(
                                            if is_current { 2.0 } else { 1.0 },
                                            border_col,
                                        ),
                                        egui::StrokeKind::Inside,
                                    );

                                    // Compact thumbnail image rect inside the card
                                    let img_rect = egui::Rect::from_min_max(
                                        egui::pos2(card_rect.left() + 5.0, card_rect.top() + 4.0),
                                        egui::pos2(
                                            card_rect.right() - 5.0,
                                            card_rect.bottom() - 20.0,
                                        ),
                                    );

                                    let tint = if is_hidden {
                                        egui::Color32::from_white_alpha(75)
                                    } else {
                                        egui::Color32::WHITE
                                    };

                                    if let Some((tex, (tw, th))) = self.viewer.thumbnail(p) {
                                        let ar = (*tw as f32 / (*th as f32).max(1.0)).max(0.1);
                                        let mut w = img_rect.width();
                                        let mut h = w / ar;
                                        if h > img_rect.height() {
                                            h = img_rect.height();
                                            w = h * ar;
                                        }
                                        let fitted = egui::Rect::from_center_size(
                                            img_rect.center(),
                                            egui::vec2(w, h),
                                        );
                                        ui.painter().image(
                                            tex.id(),
                                            fitted,
                                            egui::Rect::from_min_max(
                                                egui::pos2(0.0, 0.0),
                                                egui::pos2(1.0, 1.0),
                                            ),
                                            tint,
                                        );
                                    } else {
                                        ui.painter().rect_filled(
                                            img_rect,
                                            egui::CornerRadius::same(2),
                                            rgb(self.theme.bg_canvas),
                                        );
                                    }

                                    if is_hidden {
                                        ui.painter().text(
                                            img_rect.center(),
                                            egui::Align2::CENTER_CENTER,
                                            "Hidden",
                                            egui::FontId::proportional(11.0),
                                            rgb(self.theme.error),
                                        );
                                    }

                                    let active_drag_source: u32 =
                                        ui.ctx().data(|d| d.get_temp(drag_source_id).unwrap_or(0));
                                    let active_drag_target: u32 =
                                        ui.ctx().data(|d| d.get_temp(drag_target_id).unwrap_or(0));
                                    if active_drag_source != 0 && active_drag_source != p {
                                        let marker_y = if active_drag_target == p {
                                            Some(card_rect.top() + 1.0)
                                        } else if active_drag_target == p + 1 {
                                            Some(card_rect.bottom() - 1.0)
                                        } else {
                                            None
                                        };
                                        if let Some(y) = marker_y {
                                            ui.painter().line_segment(
                                                [
                                                    egui::pos2(card_rect.left(), y),
                                                    egui::pos2(card_rect.right(), y),
                                                ],
                                                egui::Stroke::new(3.0, rgb(self.theme.accent)),
                                            );
                                        }
                                    }

                                    // Footer label inside card
                                    let footer_y = card_rect.bottom() - 10.0;
                                    let lbl = match (has_note, is_bm) {
                                        (true, true) => format!("p.{}  󰃀", p),
                                        (true, false) => format!("p.{} ", p),
                                        (false, true) => format!("p.{} 󰃀", p),
                                        (false, false) => format!("p.{}", p),
                                    };
                                    ui.painter().text(
                                        egui::pos2(card_rect.center().x, footer_y),
                                        egui::Align2::CENTER_CENTER,
                                        lbl,
                                        egui::FontId::proportional(11.0),
                                        if is_current {
                                            egui::Color32::WHITE
                                        } else {
                                            rgb(self.theme.fg_muted)
                                        },
                                    );

                                    // Hover quick-actions: Top-Left = Hide, Top-Right = Bookmark
                                    let pointer_in_card = pointer_pos
                                        .map(|pos| card_rect.contains(pos))
                                        .unwrap_or(false);

                                    let hide_rect = egui::Rect::from_min_size(
                                        egui::pos2(card_rect.left() + 6.0, card_rect.top() + 5.0),
                                        egui::vec2(18.0, 18.0),
                                    );
                                    let bm_rect = egui::Rect::from_min_size(
                                        egui::pos2(card_rect.right() - 24.0, card_rect.top() + 5.0),
                                        egui::vec2(18.0, 18.0),
                                    );
                                    let del_rect = egui::Rect::from_min_size(
                                        egui::pos2(
                                            card_rect.right() - 22.0,
                                            card_rect.bottom() - 20.0,
                                        ),
                                        egui::vec2(17.0, 17.0),
                                    );
                                    let del_hovered = pointer_pos
                                        .map(|pos| del_rect.contains(pos))
                                        .unwrap_or(false);

                                    if pointer_in_card && self.total_pages > 1 {
                                        ui.painter().rect_filled(
                                            del_rect,
                                            egui::CornerRadius::same(3),
                                            egui::Color32::from_black_alpha(175),
                                        );
                                        ui.painter().text(
                                            del_rect.center(),
                                            egui::Align2::CENTER_CENTER,
                                            "󰆴",
                                            egui::FontId::proportional(11.0),
                                            if del_hovered {
                                                rgb(self.theme.error)
                                            } else {
                                                egui::Color32::WHITE
                                            },
                                        );
                                    }

                                    let hide_hovered = pointer_pos
                                        .map(|pos| hide_rect.contains(pos))
                                        .unwrap_or(false);
                                    let bm_hovered = pointer_pos
                                        .map(|pos| bm_rect.contains(pos))
                                        .unwrap_or(false);

                                    // Show reorder arrows on hover.
                                    // Keep these separate from hide, bookmark, and delete actions.
                                    let move_up_rect = egui::Rect::from_min_size(
                                        egui::pos2(
                                            card_rect.left() + 6.0,
                                            card_rect.bottom() - 21.0,
                                        ),
                                        egui::vec2(18.0, 16.0),
                                    );
                                    let move_down_rect = egui::Rect::from_min_size(
                                        egui::pos2(
                                            card_rect.left() + 27.0,
                                            card_rect.bottom() - 21.0,
                                        ),
                                        egui::vec2(18.0, 16.0),
                                    );
                                    let up_resp = ui.interact(
                                        move_up_rect,
                                        ui.make_persistent_id(("thumb_move_up", p)),
                                        egui::Sense::click(),
                                    );
                                    let down_resp = ui.interact(
                                        move_down_rect,
                                        ui.make_persistent_id(("thumb_move_down", p)),
                                        egui::Sense::click(),
                                    );
                                    let up_hovered = up_resp.hovered();
                                    let down_hovered = down_resp.hovered();
                                    if pointer_in_card {
                                        for (rect, glyph, hovered) in [
                                            (move_up_rect, "↑", up_hovered),
                                            (move_down_rect, "↓", down_hovered),
                                        ] {
                                            ui.painter().rect_filled(
                                                rect,
                                                egui::CornerRadius::same(3),
                                                egui::Color32::from_black_alpha(175),
                                            );
                                            ui.painter().text(
                                                rect.center(),
                                                egui::Align2::CENTER_CENTER,
                                                glyph,
                                                egui::FontId::proportional(12.0),
                                                if hovered {
                                                    rgb(self.theme.accent)
                                                } else {
                                                    egui::Color32::WHITE
                                                },
                                            );
                                        }
                                    }
                                    if up_resp.clicked() && p > 1 {
                                        move_page_action = Some((p, p - 1));
                                    } else if down_resp.clicked() && p < self.total_pages {
                                        move_page_action = Some((p, p + 1));
                                    }

                                    if pointer_in_card || is_hidden {
                                        ui.painter().rect_filled(
                                            hide_rect,
                                            egui::CornerRadius::same(3),
                                            egui::Color32::from_black_alpha(175),
                                        );
                                        ui.painter().text(
                                            hide_rect.center(),
                                            egui::Align2::CENTER_CENTER,
                                            if is_hidden { "󰈉 " } else { "󰈈 " },
                                            egui::FontId::proportional(11.5),
                                            if is_hidden || hide_hovered {
                                                rgb(self.theme.error)
                                            } else {
                                                egui::Color32::WHITE
                                            },
                                        );
                                    }

                                    if pointer_in_card || is_bm {
                                        ui.painter().rect_filled(
                                            bm_rect,
                                            egui::CornerRadius::same(3),
                                            egui::Color32::from_black_alpha(175),
                                        );
                                        ui.painter().text(
                                            bm_rect.center(),
                                            egui::Align2::CENTER_CENTER,
                                            if is_bm { "󰃀" } else { "󰃃" },
                                            egui::FontId::proportional(11.5),
                                            if is_bm || bm_hovered {
                                                rgb(self.theme.warning)
                                            } else {
                                                egui::Color32::WHITE
                                            },
                                        );
                                    }

                                    // Conferma di cancellazione inline
                                    let confirming = self.pending_delete_page == Some(p);
                                    let confirm_rect = egui::Rect::from_center_size(
                                        egui::pos2(
                                            img_rect.center().x - 24.0,
                                            img_rect.center().y + 14.0,
                                        ),
                                        egui::vec2(40.0, 22.0),
                                    );
                                    let cancel_rect = egui::Rect::from_center_size(
                                        egui::pos2(
                                            img_rect.center().x + 24.0,
                                            img_rect.center().y + 14.0,
                                        ),
                                        egui::vec2(40.0, 22.0),
                                    );
                                    let confirm_hovered = pointer_pos
                                        .map(|pos| confirm_rect.contains(pos))
                                        .unwrap_or(false);
                                    let cancel_hovered = pointer_pos
                                        .map(|pos| cancel_rect.contains(pos))
                                        .unwrap_or(false);

                                    if confirming {
                                        ui.painter().rect_filled(
                                            card_rect,
                                            egui::CornerRadius::same(4),
                                            egui::Color32::from_black_alpha(215),
                                        );
                                        ui.painter().text(
                                            egui::pos2(
                                                img_rect.center().x,
                                                img_rect.center().y - 12.0,
                                            ),
                                            egui::Align2::CENTER_CENTER,
                                            "Delete page?",
                                            egui::FontId::proportional(11.5),
                                            egui::Color32::WHITE,
                                        );
                                        let confirm_color = if confirm_hovered {
                                            let error = self.theme.error;

                                            let lighten = |c: u8| -> u8 {
                                                (c as f32 + (255.0 - c as f32) * 0.25) as u8
                                            };

                                            egui::Color32::from_rgb(
                                                lighten(error[0]),
                                                lighten(error[1]),
                                                lighten(error[2]),
                                            )
                                        } else {
                                            rgb(self.theme.error)
                                        };

                                        ui.painter().rect_filled(
                                            confirm_rect,
                                            egui::CornerRadius::same(4),
                                            confirm_color,
                                        );
                                        ui.painter().text(
                                            confirm_rect.center(),
                                            egui::Align2::CENTER_CENTER,
                                            "Yes",
                                            egui::FontId::proportional(11.0),
                                            egui::Color32::WHITE,
                                        );
                                        ui.painter().rect_filled(
                                            cancel_rect,
                                            egui::CornerRadius::same(4),
                                            egui::Color32::from_white_alpha(if cancel_hovered {
                                                60
                                            } else {
                                                30
                                            }),
                                        );
                                        ui.painter().text(
                                            cancel_rect.center(),
                                            egui::Align2::CENTER_CENTER,
                                            "No",
                                            egui::FontId::proportional(11.0),
                                            egui::Color32::WHITE,
                                        );
                                    }

                                    if card_resp.clicked()
                                        && !up_resp.clicked()
                                        && !down_resp.clicked()
                                    {
                                        if confirming {
                                            if confirm_hovered {
                                                delete_page_action = Some(p);
                                            }
                                            self.pending_delete_page = None; // Yes deletes; any other click cancels.
                                        } else if hide_hovered {
                                            self.toggle_hide_page(p, ui);
                                        } else if bm_hovered {
                                            toggle_bm_page = Some(p);
                                        } else if del_hovered && self.total_pages > 1 {
                                            if self.is_disposable_blank_page(p) {
                                                delete_page_action = Some(p);
                                            } else {
                                                self.pending_delete_page = Some(p);
                                            }
                                        } else {
                                            page_to_go = Some(p);
                                        }
                                    }
                                }
                            });

                            let drag_released = ui.input(|i| i.pointer.any_released());
                            if drag_released {
                                let (source, target): (u32, u32) = ui.ctx().data(|d| {
                                    (
                                        d.get_temp(drag_source_id).unwrap_or(0),
                                        d.get_temp(drag_target_id).unwrap_or(0),
                                    )
                                });
                                if source != 0 && target != 0 && source != target {
                                    let final_target = if source < target {
                                        target.saturating_sub(1)
                                    } else {
                                        target
                                    };
                                    let final_target = final_target.clamp(1, self.total_pages);
                                    if final_target != source {
                                        move_page_action = Some((source, final_target));
                                    }
                                }
                                ui.ctx().data_mut(|d| {
                                    d.insert_temp(drag_source_id, 0u32);
                                    d.insert_temp(drag_target_id, 0u32);
                                });
                            }

                            if let Some((start, end)) = thumb_rows {
                                self.viewer.request_thumbnails(
                                    self.total_pages,
                                    start,
                                    end,
                                    self.pdf_dark_mode,
                                );
                            }
                        }

                        SidebarTab::Outline => {
                            egui::ScrollArea::vertical()
                                .id_salt("toc_scroll")
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    if self.user_bookmarks.is_empty() && self.outline.is_empty() {
                                        // Remain cleanly empty when document has no outline/bookmarks
                                        return;
                                    }

                                    // 1. User Bookmarks
                                    if !self.user_bookmarks.is_empty() {
                                        ui.label(
                                            egui::RichText::new("󰃀  Bookmarks")
                                                .size(12.0)
                                                .color(rgb(self.theme.fg_muted)),
                                        );
                                        for bm in &self.user_bookmarks {
                                            let is_active = self.current_page == bm.page;
                                            let has_note = self
                                                .notes
                                                .get(&bm.page)
                                                .map(|s| !s.trim().is_empty())
                                                .unwrap_or(false);
                                            ui.horizontal(|ui| {
                                                let prefix = if has_note { "● " } else { "" };
                                                let label = format!(
                                                    "{}{}  (p. {})",
                                                    prefix, bm.title, bm.page
                                                );
                                                let resp = ui.selectable_label(is_active, label);
                                                if is_active && scroll_sidebar_to_current {
                                                    resp.scroll_to_me(Some(egui::Align::Center));
                                                }
                                                if resp.clicked() {
                                                    page_to_go = Some(bm.page);
                                                }
                                                ui.with_layout(
                                                    egui::Layout::right_to_left(
                                                        egui::Align::Center,
                                                    ),
                                                    |ui| {
                                                        if ui
                                                            .small_button("󰅖")
                                                            .on_hover_text("Delete bookmark")
                                                            .clicked()
                                                        {
                                                            remove_bm_page = Some(bm.page);
                                                        }
                                                    },
                                                );
                                            });
                                        }
                                        if !self.outline.is_empty() {
                                            ui.separator();
                                        }
                                    }

                                    // 2. Document Native Outline (if present)
                                    if !self.outline.is_empty() {
                                        ui.label(
                                            egui::RichText::new("󰈙 Document Outline")
                                                .size(12.0)
                                                .color(rgb(self.theme.fg_muted)),
                                        );
                                        for item in &self.outline {
                                            let is_active = self.current_page == item.page;
                                            let has_note = self
                                                .notes
                                                .get(&item.page)
                                                .map(|s| !s.trim().is_empty())
                                                .unwrap_or(false);
                                            let text = if has_note {
                                                format!("● {}", item.title)
                                            } else {
                                                item.title.clone()
                                            };
                                            let resp = ui.selectable_label(is_active, text);
                                            if is_active && scroll_sidebar_to_current {
                                                resp.scroll_to_me(Some(egui::Align::Center));
                                            }
                                            if resp.clicked() {
                                                page_to_go = Some(item.page);
                                            }
                                        }
                                    }
                                });
                        }
                    }

                    if let Some(p) = toggle_bm_page {
                        if self.user_bookmarks.iter().any(|b| b.page == p) {
                            self.remove_bookmark(p);
                        } else {
                            self.add_or_update_bookmark(p, format!("Page {}", p));
                        }
                    }
                    if let Some(p) = remove_bm_page {
                        self.remove_bookmark(p);
                    }
                    if let Some(target) = page_to_go {
                        self.go_to_page(target, ui.ctx());
                    }
                    if let Some(del_p) = delete_page_action {
                        self.delete_pdf_page_and_shift_data(del_p, ui.ctx());
                    }
                    if let Some((from, to)) = move_page_action {
                        self.reorder_pdf_page(from, to, ui.ctx());
                    }
                });
        }

        let fit_notes_layout = std::mem::take(&mut self.fit_notes_layout_pending)
            && self.can_fit_notes_layout()
            && !use_bottom_notes;

        // Right Panel (Notes + Bottom Annotation Tools)
        if self.has_pdf() {
            if self.view_mode == ViewMode::SlideshowWithNotes {
                let active_slide_note = if self.note_page == self.current_page {
                    self.current_note.trim().to_string()
                } else {
                    self.notes
                        .get(&self.current_page)
                        .cloned()
                        .unwrap_or_default()
                        .trim()
                        .to_string()
                };

                // In `ExpandFull` mode, if the current slide has no note (and note page isn't locked),
                // we hide the right panel completely so the slide expands to 100% fullscreen!
                let hide_panel_for_expand = self.view_mode == ViewMode::SlideshowWithNotes
                    && self.app_state.settings.slideshow_empty_mode
                        == SlideshowEmptyMode::ExpandFull
                    && active_slide_note.is_empty()
                    && !self.lock_note_page;

                if self.view_mode == ViewMode::SlideshowWithNotes
                    && self.has_pdf()
                    && !hide_panel_for_expand
                {
                    let show_hud = ui
                        .input(|i| i.pointer.hover_pos())
                        .map(|p| p.y < 60.0)
                        .unwrap_or(false);
                    if show_hud {
                        let theme = &self.theme;

                        let to_color =
                            |rgb: [u8; 3]| egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]);

                        egui::Area::new(ui.make_persistent_id("slideshow_hud"))
                            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 20.0))
                            .order(egui::Order::Foreground)
                            .show(ui.ctx(), |ui| {
                                egui::Frame::NONE
                                    .fill(to_color(theme.bg_elevated))
                                    .corner_radius(20)
                                    .stroke(egui::Stroke::new(1.0, to_color(theme.border)))
                                    .inner_margin(egui::Margin::symmetric(14, 6))
                                    .shadow(egui::epaint::Shadow {
                                        offset: [0, 4],
                                        blur: 12,
                                        spread: 0,
                                        color: egui::Color32::from_black_alpha(100),
                                    })
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 10.0;

                                            ui.scope(|ui| {
                                                let visuals = &mut ui.style_mut().visuals.widgets;

                                                visuals.hovered.weak_bg_fill =
                                                    to_color(theme.bg_hover);
                                                visuals.hovered.fg_stroke =
                                                    egui::Stroke::new(1.0, to_color(theme.accent));
                                                visuals.hovered.corner_radius =
                                                    egui::CornerRadius::same(12);

                                                visuals.active.weak_bg_fill =
                                                    to_color(theme.bg_active);
                                                visuals.active.fg_stroke =
                                                    egui::Stroke::new(1.0, to_color(theme.accent));
                                                visuals.active.corner_radius =
                                                    egui::CornerRadius::same(12);

                                                let exit_btn = egui::Button::new(
                                                    egui::RichText::new("󰅖 Exit")
                                                        .size(13.0)
                                                        .strong()
                                                        .color(to_color(theme.accent)),
                                                )
                                                .frame(false)
                                                .corner_radius(12);

                                                if ui
                                                    .add(exit_btn)
                                                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                                                    .on_hover_text("Press Esc to exit")
                                                    .clicked()
                                                {
                                                    self.view_mode = self.prev_view_mode;
                                                }
                                            });
                                        });
                                    });
                            });
                    }

                    let max_w = (win_rect.width() * 0.8).max(150.0);
                    let panel = egui::Panel::right("slideshow_notes_panel")
                        .resizable(true)
                        .default_size(320.0)
                        .size_range(150.0..=max_w);
                    let panel =
                        self.restore_panel_size(panel, PanelSlot::Slideshow, 320.0, (150.0, max_w));
                    let resp = panel.show(ui, |ui| {
                        // Keep Markdown widgets from growing the panel's measured width.
                        let rect = ui.max_rect();
                        let mut child = ui.new_child(
                            egui::UiBuilder::new()
                                .max_rect(rect)
                                .layout(egui::Layout::top_down(egui::Align::Min)),
                        );
                        child.set_clip_rect(rect.intersect(ui.clip_rect()));
                        self.render_slideshow_notes_contents(&mut child, active_slide_note);
                        ui.allocate_rect(rect, egui::Sense::hover());
                    });
                    self.track_panel_size(ui, PanelSlot::Slideshow, resp.response.rect.width());
                }
            } else if self.show_notes && !in_slideshow {
                if use_bottom_notes {
                    let max_panel_h = (win_rect.height() * 0.75).max(220.0);
                    let notes_panel = egui::Panel::bottom("notes_panel_bottom")
                        .resizable(true)
                        .default_size((win_rect.height() * 0.36).clamp(180.0, 420.0))
                        .size_range(120.0..=max_panel_h)
                        // Keep the same frame in Edit, Preview, and Sketch so switching modes
                        // never shifts the panel contents or changes its apparent height. Restore
                        // a modest horizontal inset so content does not sit against the panel edges.
                        .frame(
                            egui::Frame::side_top_panel(ui.style())
                                .inner_margin(egui::Margin::symmetric(6, 0)),
                        );
                    let default_h = (win_rect.height() * 0.36).clamp(180.0, 420.0);
                    let notes_panel = self.restore_panel_size(
                        notes_panel,
                        PanelSlot::Bottom,
                        default_h,
                        (120.0, max_panel_h),
                    );
                    let resp = notes_panel.show(ui, |ui| {
                        self.render_notes_panel_contents(ui, text_edit_id, true);
                    });
                    self.track_panel_size(ui, PanelSlot::Bottom, resp.response.rect.height());
                } else {
                    let available = ui.available_rect_before_wrap();
                    let min_w = 250.0_f32.min((available.width() - 100.0).max(1.0));
                    let reserved_pdf_w = if self.split_view_enabled {
                        340.0
                    } else {
                        100.0
                    };
                    let max_w = (available.width() - reserved_pdf_w).max(min_w);
                    if fit_notes_layout {
                        let size = self.fit_notes_target_width(available, min_w, max_w);
                        self.panel_sizes.set(PanelSlot::Right, size);
                        self.panel_apply[PanelSlot::Right as usize] = true;
                        self.panel_sizes_dirty = true;
                        self.zoom_mode = ZoomMode::FitPage;
                        self.zoom_anchor = None;
                        ui.ctx().request_repaint();
                    }
                    let notes_panel = egui::Panel::right("notes_panel")
                        .resizable(true)
                        .default_size(350.0)
                        .size_range(min_w..=max_w)
                        // Match the bottom-panel treatment: a stable frame across all note modes,
                        // with a modest horizontal inset around the notes content.
                        .frame(
                            egui::Frame::side_top_panel(ui.style())
                                .inner_margin(egui::Margin::symmetric(6, 0)),
                        );
                    let notes_panel = self.restore_panel_size(
                        notes_panel,
                        PanelSlot::Right,
                        350.0,
                        (min_w, max_w),
                    );
                    let resp = notes_panel.show(ui, |ui| {
                        self.render_notes_panel_contents(ui, text_edit_id, false);
                    });
                    self.track_panel_size(ui, PanelSlot::Right, resp.response.rect.width());
                    if !fit_notes_layout {
                        self.handle_fit_notes_divider_double_click(
                            ui,
                            &resp.response,
                            available,
                            min_w,
                            max_w,
                        );
                    }
                }
            }
        }

        // Central Panel (Welcome Screen when no PDF is loaded, otherwise PDF Canvas)
        // Secondary PDF Pane when Split-View is active (resizable side-by-side PDF view!)
        if self.view_mode != ViewMode::SinglePage {
            self.single_page_wheel = SinglePageWheel::default();
        }
        if self.split_view_enabled && self.has_pdf() && !in_slideshow {
            egui::Panel::right("pdf_split_secondary_pane")
                .resizable(true)
                .default_size(460.0)
                .size_range(240.0..=960.0)
                .frame(
                    egui::Frame::NONE
                        .fill(rgb(self.theme.bg_canvas))
                        .inner_margin(egui::Margin::same(1))
                        .stroke(egui::Stroke::new(1.0, rgb(self.theme.border))),
                )
                .show(ui, |ui| {
                    // Compact Top Navigation Bar for the Secondary PDF View
                    egui::Frame::NONE
                        .fill(rgb(self.theme.bg_panel))
                        .corner_radius(egui::CornerRadius::same(6))
                        .inner_margin(egui::Margin::symmetric(8, 2))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new("󰯌 Split")
                                        .strong()
                                        .size(12.5)
                                        .color(rgb(self.theme.accent)),
                                );
                                ui.separator();

                                let split_h = self.sz(22.0);
                                if ui
                                    .add_sized([split_h, split_h], egui::Button::new(""))
                                    .clicked()
                                    && self.split_secondary_page > 1
                                {
                                    self.split_secondary_page -= 1;
                                }

                                let mut sec_p = self.split_secondary_page;
                                if ui
                                    .add_sized(
                                        [46.0, split_h],
                                        egui::DragValue::new(&mut sec_p)
                                            .prefix("p.")
                                            .range(1..=self.total_pages.max(1))
                                            .speed(0.1),
                                    )
                                    .changed()
                                {
                                    self.split_secondary_page =
                                        sec_p.clamp(1, self.total_pages.max(1));
                                }

                                if ui
                                    .add_sized([split_h, split_h], egui::Button::new(""))
                                    .clicked()
                                    && self.split_secondary_page < self.total_pages
                                {
                                    self.split_secondary_page += 1;
                                }

                                ui.separator();

                                // Independent zoom controls for the secondary pane
                                if ui.small_button("󰍴").clicked() {
                                    self.split_secondary_zoom =
                                        (self.split_secondary_zoom - 0.15).max(0.4);
                                }
                                if ui
                                    .small_button(format!(
                                        "{:.0}%",
                                        self.split_secondary_zoom * 100.0
                                    ))
                                    .on_hover_text("Reset secondary zoom to 100%")
                                    .clicked()
                                {
                                    self.split_secondary_zoom = 1.0;
                                }
                                if ui.small_button("󰐕").clicked() {
                                    self.split_secondary_zoom =
                                        (self.split_secondary_zoom + 0.15).min(4.0);
                                }

                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui
                                            .small_button("󰅖")
                                            .on_hover_text("Close Split-View")
                                            .clicked()
                                        {
                                            self.split_view_enabled = false;
                                        }
                                        if ui
                                            .small_button("󰁨 =Main")
                                            .on_hover_text(
                                                "Jump secondary view to current main slide",
                                            )
                                            .clicked()
                                        {
                                            self.split_secondary_page = self.current_page;
                                        }
                                    },
                                );
                            });
                        });

                    ui.add_space(6.0);

                    let avail = ui.available_size();
                    let sec_page = self.split_secondary_page;

                    let tex_opt = self.viewer.page_or_thumbnail(sec_page).cloned();

                    if let Some((tex, (tw, th))) = tex_opt {
                        let ar = (th as f32 / (tw as f32).max(1.0)).max(0.1);
                        let fit_w = (avail.x - 2.0).max(80.0);
                        let fit_h = fit_w * ar;
                        let (base_w, base_h) = if fit_h > avail.y - 2.0 {
                            let h = (avail.y - 2.0).max(80.0);
                            (h / ar, h)
                        } else {
                            (fit_w, fit_h)
                        };

                        let disp_w = base_w * self.split_secondary_zoom;
                        let disp_h = base_h * self.split_secondary_zoom;

                        // Same wheel speed and middle-button drag as the main page.
                        let pane_rect = ui.available_rect_before_wrap();
                        self.apply_wheel_speed(ui, pane_rect);
                        let pane_pan = self.middle_drag_pan(ui, "split_secondary", pane_rect);

                        egui::ScrollArea::both()
                            .id_salt("split_secondary_scroll")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                if pane_pan != egui::Vec2::ZERO {
                                    ui.scroll_with_delta(pane_pan);
                                }
                                let pad_x = ((avail.x - disp_w) * 0.5).max(0.0);
                                let pad_y = ((avail.y - disp_h) * 0.5).max(0.0);
                                ui.add_space(pad_y);
                                ui.horizontal(|ui| {
                                    ui.add_space(pad_x);
                                    let sense = egui::Sense::click_and_drag();
                                    let img_resp = ui.add(
                                        egui::Image::new((tex.id(), egui::vec2(disp_w, disp_h)))
                                            .sense(sense),
                                    );
                                    let slide_rect = img_resp.rect;
                                    self.handle_slide_annotations(
                                        ui, sec_page, slide_rect, &img_resp,
                                    );
                                });
                            });
                    } else {
                        ui.centered_and_justified(|ui| {
                            ui.spinner();
                        });
                    }
                });
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::central_panel(ui.style())
                    .fill(rgb(self.theme.bg_canvas))
                    .inner_margin(egui::Margin::ZERO),
            )
            .show(ui, |ui| {
                if !self.has_pdf() {
                    self.show_toc = false;
                    self.show_notes = false;

                    let mut file_to_open: Option<PathBuf> = None;
                    let mut file_to_remove: Option<PathBuf> = None;

                    // Collect completed first-page previews and schedule missing ones off the UI thread.
                    while let Ok((path, image)) = self.home_thumb_rx.try_recv() {
                        let size = image.size;
                        let texture = ui.ctx().load_texture(
                            format!("home-preview:{}", path.display()),
                            image,
                            egui::TextureOptions::LINEAR,
                        );
                        self.home_thumb_cache
                            .insert(path.clone(), (texture, size.into()));
                        self.home_thumb_pending.remove(&path);
                    }

                    for entry in &self.app_state.recent_files {
                        let path = entry.path.clone();
                        if path.is_file()
                            && !self.home_thumb_cache.contains_key(&path)
                            && self.home_thumb_pending.insert(path.clone())
                        {
                            let tx = self.home_thumb_tx.clone();
                            let ctx = ui.ctx().clone();
                            thread::spawn(move || {
                                let rendered = path
                                    .to_str()
                                    .and_then(|p| Document::open(p).ok())
                                    .and_then(|doc| render_to_image(&doc, 0, 0.42, false))
                                    .map(|(_, image, _)| image);
                                if let Some(image) = rendered {
                                    let _ = tx.send((path, image));
                                    ctx.request_repaint();
                                }
                            });
                        }
                    }

                            ui.vertical_centered(|ui| {
                                let content_width =
                                    (ui.available_width() - 48.0).clamp(260.0, 1040.0);
                                ui.set_width(content_width);

                                ui.add_space(60.0);

                                // 1. Title and controls
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 12.0;
                                    ui.label(
                                        egui::RichText::new("")
                                            .size(30.0)
                                            .color(rgb(self.theme.accent)),
                                    );
                                    ui.label(
                                        egui::RichText::new(" Lapis")
                                            .size(26.0)
                                            .strong()
                                            .color(rgb(self.theme.fg_primary)),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            let settings = egui::Button::new(
                                                egui::RichText::new("  Settings")
                                                    .size(12.0)
                                                    .color(rgb(self.theme.fg_primary)),
                                            )
                                            .corner_radius(egui::CornerRadius::same(6))
                                            .min_size(egui::vec2(92.0, 30.0));
                                            if ui
                                                .add(settings)
                                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                                .clicked()
                                            {
                                                self.show_settings = true;
                                            }
                                        },
                                    );
                                });

                                ui.add_space(28.0);

                                // 2. Quick actions
                                ui.columns(2, |cols| {
                                    // Open PDF
                                    let open_frame = egui::Frame::NONE
                                        .fill(rgb(self.theme.bg_panel))
                                        .stroke(egui::Stroke::new(1.0, rgb(self.theme.border)))
                                        .corner_radius(egui::CornerRadius::same(
                                            self.theme.corner_radius,
                                        ))
                                        .inner_margin(egui::Margin::same(16))
                                        .show(&mut cols[0], |ui| {
                                            ui.set_width(ui.available_width());

                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    egui::RichText::new("󰝰  Open PDF")
                                                        .size(15.5)
                                                        .strong()
                                                        .color(rgb(self.theme.fg_primary)),
                                                );
                                            });

                                            ui.add_space(6.0);

                                            ui.label(
                                                egui::RichText::new(
                                                    "Import an existing PDF document from your computer.",
                                                )
                                                .size(11.5)
                                                .color(rgb(self.theme.fg_muted)),
                                            );

                                            ui.add_space(14.0);

                                            ui.label(
                                                egui::RichText::new("Browse files  →")
                                                    .size(12.0)
                                                    .strong()
                                                    .color(rgb(self.theme.accent)),
                                            );
                                        });

                                    let open_rect = open_frame.response.rect;
                                    let open_resp = cols[0]
                                        .interact(
                                            open_rect,
                                            cols[0].scope_id().with("action_open_pdf"),
                                            egui::Sense::click(),
                                        )
                                        .on_hover_cursor(egui::CursorIcon::PointingHand);

                                    if open_resp.hovered() {
                                        cols[0].painter().rect_stroke(
                                            open_rect,
                                            egui::CornerRadius::same(self.theme.corner_radius),
                                            egui::Stroke::new(1.0, rgb(self.theme.accent)),
                                            egui::StrokeKind::Inside,
                                        );
                                    }

                                    if open_resp.clicked() {
                                        self.dialogs.open(
                                            cols[0].ctx(),
                                            DialogKind::OpenPdf,
                                            DialogSpec::open_file()
                                                .filter("PDF files", &["pdf"]),
                                        );
                                    }

                                    // New Notebook
                                    let nb_frame = egui::Frame::NONE
                                        .fill(rgb(self.theme.bg_panel))
                                        .stroke(egui::Stroke::new(1.0, rgb(self.theme.border)))
                                        .corner_radius(egui::CornerRadius::same(
                                            self.theme.corner_radius,
                                        ))
                                        .inner_margin(egui::Margin::same(16))
                                        .show(&mut cols[1], |ui| {
                                            ui.set_width(ui.available_width());

                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    egui::RichText::new("󱞁  Blank Notebook")
                                                        .size(15.5)
                                                        .strong()
                                                        .color(rgb(self.theme.fg_primary)),
                                                );
                                            });

                                            ui.add_space(6.0);

                                            ui.label(
                                                egui::RichText::new(
                                                    "Create a new blank study notebook from scratch.",
                                                )
                                                .size(11.5)
                                                .color(rgb(self.theme.fg_muted)),
                                            );

                                            ui.add_space(14.0);

                                            ui.label(
                                                egui::RichText::new("Create notebook  →")
                                                    .size(12.0)
                                                    .strong()
                                                    .color(rgb(self.theme.accent)),
                                            );
                                        });

                                    let nb_rect = nb_frame.response.rect;
                                    let nb_resp = cols[1]
                                        .interact(
                                            nb_rect,
                                            cols[1].scope_id().with("action_new_notebook"),
                                            egui::Sense::click(),
                                        )
                                        .on_hover_cursor(egui::CursorIcon::PointingHand);

                                    if nb_resp.hovered() {
                                        cols[1].painter().rect_stroke(
                                            nb_rect,
                                            egui::CornerRadius::same(self.theme.corner_radius),
                                            egui::Stroke::new(1.0, rgb(self.theme.accent)),
                                            egui::StrokeKind::Inside,
                                        );
                                    }

                                    if nb_resp.clicked() {
                                        self.dialogs.open(
                                            cols[1].ctx(),
                                            DialogKind::NewNotebook,
                                            DialogSpec::save_file()
                                                .file_name("notebook.pdf")
                                                .filter("PDF Notebook", &["pdf"]),
                                        );
                                    }
                                });

                                ui.add_space(32.0);

                                // 3. Recent Documents
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new("Recent documents")
                                            .size(15.5)
                                            .strong()
                                            .color(rgb(self.theme.fg_primary)),
                                    );

                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui
                                                .selectable_label(
                                                    self.app_state.home_grid_view,
                                                    " 󱇙 ",
                                                )
                                                .on_hover_text("Grid view")
                                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                                .clicked()
                                            {
                                                self.app_state.home_grid_view = true;
                                                self.app_state.save();
                                            }
                                            if ui
                                                .selectable_label(
                                                    !self.app_state.home_grid_view,
                                                    "  ",
                                                )
                                                .on_hover_text("List view")
                                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                                .clicked()
                                            {
                                                self.app_state.home_grid_view = false;
                                                self.app_state.save();
                                            }
                                        },
                                    );
                                });

                                ui.add_space(12.0);

                                if self.app_state.recent_files.is_empty() {
                                    egui::Frame::NONE
                                        .fill(rgb(self.theme.bg_panel))
                                        .stroke(egui::Stroke::new(1.0, rgb(self.theme.border)))
                                        .corner_radius(egui::CornerRadius::same(
                                            self.theme.corner_radius,
                                        ))
                                        .inner_margin(egui::Margin::same(28))
                                        .show(ui, |ui| {
                                            ui.set_width((content_width - 58.0).max(100.0));
                                            ui.vertical_centered(|ui| {
                                                ui.label(
                                                    egui::RichText::new("󰈙")
                                                        .size(24.0)
                                                        .color(rgb(self.theme.fg_muted)),
                                                );
                                                ui.label(
                                                    egui::RichText::new("No documents yet")
                                                        .size(14.0)
                                                        .strong()
                                                        .color(rgb(self.theme.fg_primary)),
                                                );
                                                ui.add_space(4.0);
                                                ui.label(
                                                    egui::RichText::new(
                                                        "Your recently opened PDFs will appear here.",
                                                    )
                                                    .size(11.5)
                                                    .color(rgb(self.theme.fg_muted)),
                                                );
                                            });
                                        });
                                } else {
                                    egui::ScrollArea::vertical()
                                        .id_salt("recent_files_scroll")
                                        .auto_shrink([false, true])
                                        .max_height((ui.available_height() - 32.0).max(120.0))
                                        .show(ui, |ui| {
                                            if self.app_state.home_grid_view {
                                                let gap = 12.0_f32;
                                                let available = ui.available_width().max(1.0);
                                                let cols = (((available + gap) / (165.0 + gap))
                                                    .floor()
                                                    as usize)
                                                    .clamp(1, 6);
                                                let card_w = (available
                                                    - gap * (cols - 1) as f32)
                                                    / cols as f32;
                                                let card_h = 152.0_f32;

                                                egui::Grid::new("recent_documents_grid")
                                                    .num_columns(cols)
                                                    .spacing(egui::vec2(gap, gap))
                                                    .show(ui, |ui| {
                                                        for (idx, entry) in self
                                                            .app_state
                                                            .recent_files
                                                            .iter()
                                                            .enumerate()
                                                        {
                                                            let cell = ui.allocate_ui_with_layout(
                                                                egui::vec2(card_w, card_h),
                                                                egui::Layout::top_down(
                                                                    egui::Align::Min,
                                                                ),
                                                                |cell_ui| {
                                                                    let frame = egui::Frame::NONE
                                                                        .fill(rgb(
                                                                            self.theme.bg_panel,
                                                                        ))
                                                                        .stroke(egui::Stroke::new(
                                                                            1.0,
                                                                            rgb(self.theme.border),
                                                                        ))
                                                                        .corner_radius(
                                                                            egui::CornerRadius::same(8),
                                                                        )
                                                                        .inner_margin(
                                                                            egui::Margin::same(8),
                                                                        )
                                                                        .show(cell_ui, |card_ui| {
                                                                            // card_w - margin(16) - stroke(2) = card_w - 18
                                                                            let inner_w = (card_w
                                                                                - 18.0)
                                                                                .max(60.0);
                                                                            card_ui
                                                                                .set_width(inner_w);

                                                                            let preview_h =
                                                                                card_h - 60.0;
                                                                            card_ui.allocate_ui_with_layout(
                                                                                egui::vec2(inner_w, preview_h),
                                                                                egui::Layout::centered_and_justified(egui::Direction::TopDown),
                                                                                |preview_ui| {
                                                                                    if let Some((texture, (w, h))) = self.home_thumb_cache.get(&entry.path) {
                                                                                        let max_w = inner_w - 4.0;
                                                                                        let max_h = preview_h - 4.0;
                                                                                        let scale = (max_w / (*w as f32).max(1.0)).min(max_h / (*h as f32).max(1.0));
                                                                                        let size = egui::vec2(*w as f32 * scale, *h as f32 * scale);
                                                                                        preview_ui.add(egui::Image::new((texture.id(), size)));
                                                                                    } else if self.home_thumb_pending.contains(&entry.path) {
                                                                                        preview_ui.spinner();
                                                                                    } else {
                                                                                        preview_ui.label(
                                                                                            egui::RichText::new("PDF")
                                                                                                .size(10.0)
                                                                                                .color(rgb(self.theme.fg_muted)),
                                                                                        );
                                                                                    }
                                                                                },
                                                                            );

                                                                            card_ui.add_space(6.0);

                                                                            card_ui.add_sized(
                                                                                [inner_w, 17.0],
                                                                                egui::Label::new(
                                                                                    egui::RichText::new(
                                                                                        truncate_filename(&entry.name, 22),
                                                                                    )
                                                                                    .size(13.0)
                                                                                    .strong()
                                                                                    .color(rgb(self.theme.fg_primary)),
                                                                                )
                                                                                .truncate(),
                                                                            );

                                                                            card_ui.label(
                                                                                egui::RichText::new(format!(
                                                                                    "Page {} / {}",
                                                                                    entry.last_page,
                                                                                    entry.total_pages,
                                                                                ))
                                                                                .size(10.0)
                                                                                .color(rgb(self.theme.fg_muted)),
                                                                            );

                                                                            card_ui.add_space(4.0);

                                                                            let progress = (entry.last_page as f32
                                                                                / (entry.total_pages.max(1) as f32))
                                                                                .clamp(0.0, 1.0);
                                                                            let (bar_rect, _) = card_ui.allocate_exact_size(
                                                                                egui::vec2(inner_w, 3.0),
                                                                                egui::Sense::hover(),
                                                                            );
                                                                            card_ui.painter().rect_filled(
                                                                                bar_rect,
                                                                                2.0,
                                                                                rgb(self.theme.border),
                                                                            );
                                                                            let mut fill_rect = bar_rect;
                                                                            fill_rect.set_width(
                                                                                bar_rect.width() * progress,
                                                                            );
                                                                            card_ui.painter().rect_filled(
                                                                                fill_rect,
                                                                                2.0,
                                                                                rgb(self.theme.accent),
                                                                            );
                                                                        });

                                                                    let rect = frame.response.rect;
                                                                    let close_rect =
                                                                        egui::Rect::from_min_size(
                                                                            egui::pos2(
                                                                                rect.right()
                                                                                    - 24.0,
                                                                                rect.top() + 6.0,
                                                                            ),
                                                                            egui::vec2(18.0, 18.0),
                                                                        );

                                                                    let hovered = cell_ui
                                                                        .rect_contains_pointer(
                                                                            rect,
                                                                        );
                                                                    let on_close = hovered
                                                                        && cell_ui
                                                                            .rect_contains_pointer(
                                                                                close_rect,
                                                                            );

                                                                    if hovered {
                                                                        cell_ui
                                                                            .painter()
                                                                            .rect_stroke(
                                                                                rect,
                                                                                egui::CornerRadius::same(8),
                                                                                egui::Stroke::new(
                                                                                    1.0,
                                                                                    rgb(self.theme.accent),
                                                                                ),
                                                                                egui::StrokeKind::Inside,
                                                                            );
                                                                    }

                                                                    let mut remove_clicked = false;
                                                                    let mut card_clicked = false;

                                                                    if on_close {
                                                                        let close_resp = cell_ui
                                                                            .interact(
                                                                                close_rect,
                                                                                cell_ui.scope_id().with((
                                                                                    "remove_btn",
                                                                                    idx,
                                                                                )),
                                                                                egui::Sense::click(),
                                                                            )
                                                                            .on_hover_text(
                                                                                "Remove from history",
                                                                            )
                                                                            .on_hover_cursor(
                                                                                egui::CursorIcon::PointingHand,
                                                                            );
                                                                        remove_clicked =
                                                                            close_resp.clicked();
                                                                    } else {
                                                                        let card_resp = cell_ui
                                                                            .interact(
                                                                                rect,
                                                                                cell_ui.scope_id().with((
                                                                                    "recent_card",
                                                                                    idx,
                                                                                )),
                                                                                egui::Sense::click(),
                                                                            )
                                                                            .on_hover_text(
                                                                                entry
                                                                                    .path
                                                                                    .to_string_lossy(),
                                                                            )
                                                                            .on_hover_cursor(
                                                                                egui::CursorIcon::PointingHand,
                                                                            );
                                                                        card_clicked =
                                                                            card_resp.clicked();
                                                                    }

                                                                    if hovered {
                                                                        cell_ui
                                                                            .painter()
                                                                            .rect_filled(
                                                                                close_rect,
                                                                                4.0,
                                                                                if on_close {
                                                                                    rgb(self.theme.accent)
                                                                                } else {
                                                                                    rgb(self.theme.bg_canvas)
                                                                                },
                                                                            );
                                                                        cell_ui.painter().text(
                                                                            close_rect.center(),
                                                                            egui::Align2::CENTER_CENTER,
                                                                            "×",
                                                                            egui::FontId::proportional(13.0),
                                                                            rgb(self.theme.fg_primary),
                                                                        );
                                                                    }

                                                                    (remove_clicked, card_clicked)
                                                                },
                                                            );

                                                            let (remove_clicked, card_clicked) =
                                                                cell.inner;
                                                            if remove_clicked {
                                                                file_to_remove =
                                                                    Some(entry.path.clone());
                                                            } else if card_clicked {
                                                                file_to_open =
                                                                    Some(entry.path.clone());
                                                            }

                                                            if (idx + 1) % cols == 0 {
                                                                ui.end_row();
                                                            }
                                                        }
                                                    });
                                            } else {
                                                // List
                                                for (idx, entry) in
                                                    self.app_state.recent_files.iter().enumerate()
                                                {
                                                    let row = egui::Frame::NONE
                                                        .fill(rgb(self.theme.bg_panel))
                                                        .stroke(egui::Stroke::new(
                                                            1.0,
                                                            rgb(self.theme.border),
                                                        ))
                                                        .corner_radius(egui::CornerRadius::same(6))
                                                        .inner_margin(egui::Margin::symmetric(
                                                            10, 6,
                                                        ))
                                                        .show(ui, |ui| {
                                                            let mut remove_clicked = false;
                                                            let mut close_rect = egui::Rect::NOTHING;

                                                            ui.horizontal(|ui| {
                                                                let row_h = 32.0;
                                                                ui.set_min_height(row_h);

                                                                ui.allocate_ui_with_layout(
                                                                    egui::vec2(36.0, row_h),
                                                                    egui::Layout::centered_and_justified(
                                                                        egui::Direction::TopDown,
                                                                    ),
                                                                    |thumb_ui| {
                                                                        if let Some((
                                                                            texture,
                                                                            (w, h),
                                                                        )) = self
                                                                            .home_thumb_cache
                                                                            .get(&entry.path)
                                                                        {
                                                                            let max_w = 34.0_f32;
                                                                            let max_h = 30.0_f32;
                                                                            let scale = (max_w
                                                                                / (*w as f32)
                                                                                    .max(1.0))
                                                                            .min(
                                                                                max_h
                                                                                    / (*h as f32)
                                                                                        .max(1.0),
                                                                            );
                                                                            let size = egui::vec2(
                                                                                *w as f32 * scale,
                                                                                *h as f32 * scale,
                                                                            );
                                                                            thumb_ui.add(
                                                                                egui::Image::new((
                                                                                    texture.id(),
                                                                                    size,
                                                                                )),
                                                                            );
                                                                        } else if self
                                                                            .home_thumb_pending
                                                                            .contains(&entry.path)
                                                                        {
                                                                            thumb_ui.spinner();
                                                                        }
                                                                    },
                                                                );

                                                                ui.add_space(6.0);

                                                                ui.add(
                                                                    egui::Label::new(
                                                                        egui::RichText::new(
                                                                            truncate_filename(
                                                                                &entry.name,
                                                                                48,
                                                                            ),
                                                                        )
                                                                        .size(12.5)
                                                                        .strong()
                                                                        .color(rgb(
                                                                            self.theme.fg_primary,
                                                                        )),
                                                                    )
                                                                    .truncate(),
                                                                );

                                                                ui.add_space(8.0);

                                                                ui.label(
                                                                    egui::RichText::new(format!(
                                                                        "Page {} / {}",
                                                                        entry.last_page,
                                                                        entry.total_pages
                                                                    ))
                                                                    .size(10.0)
                                                                    .color(rgb(
                                                                        self.theme.fg_muted,
                                                                    )),
                                                                );

                                                                ui.with_layout(
                                                                    egui::Layout::right_to_left(
                                                                        egui::Align::Center,
                                                                    ),
                                                                    |ui| {
                                                                        let btn = ui
                                                                            .small_button("×")
                                                                            .on_hover_text(
                                                                                "Remove from history",
                                                                            )
                                                                            .on_hover_cursor(
                                                                                egui::CursorIcon::PointingHand,
                                                                            );
                                                                        close_rect = btn.rect;
                                                                        if btn.clicked() {
                                                                            remove_clicked = true;
                                                                        }
                                                                    },
                                                                );
                                                            });

                                                            (remove_clicked, close_rect)
                                                        });

                                                    let row_rect = row.response.rect;
                                                    let (remove_clicked, close_rect) = row.inner;
                                                    let on_close =
                                                        ui.rect_contains_pointer(close_rect);

                                                    if remove_clicked {
                                                        file_to_remove = Some(entry.path.clone());
                                                    } else if !on_close {
                                                        let row_resp = ui
                                                            .interact(
                                                                row_rect,
                                                                ui.scope_id()
                                                                    .with(("recent_row", idx)),
                                                                egui::Sense::click(),
                                                            )
                                                            .on_hover_text(
                                                                entry.path.to_string_lossy(),
                                                            )
                                                            .on_hover_cursor(
                                                                egui::CursorIcon::PointingHand,
                                                            );

                                                        if row_resp.clicked() {
                                                            file_to_open = Some(entry.path.clone());
                                                        }
                                                    }

                                                    if ui.rect_contains_pointer(row_rect) {
                                                        ui.painter().rect_stroke(
                                                            row_rect,
                                                            egui::CornerRadius::same(6),
                                                            egui::Stroke::new(
                                                                1.0,
                                                                rgb(self.theme.accent),
                                                            ),
                                                            egui::StrokeKind::Inside,
                                                        );
                                                    }
                                                    ui.add_space(4.0);
                                                }
                                            }
                                        });
                                }

                                ui.add_space(32.0);
                            });


                    // 4. Actions
                    if let Some(p) = file_to_remove {
                        self.app_state.remove_recent(&p);
                    }

                    if let Some(p) = file_to_open {
                        self.open_pdf_user(p, ui.ctx());
                    }

                    return;
                }
                // egui fades the canvas edges when the page overflows (zoomed); keep it subtle.
                ui.spacing_mut().scroll.fade = egui::style::ScrollFadeStyle {
                    strength: 0.12,
                    size: 10.0,
                };
                let visible_w = ui.available_width();
                let visible_h = ui.available_height();

                let current_texture_ready = self.viewer.page_or_thumbnail(self.current_page).is_some();
                if matches!(self.view_mode, ViewMode::SinglePage | ViewMode::Slideshow | ViewMode::SlideshowWithNotes) {
                    if let Some((handle, dims)) = self.viewer.page_or_thumbnail(self.current_page) {
                        self.texture_handle = Some(handle.clone());
                        self.page_dims = *dims;
                    }
                }
                let page_w = self.page_dims.0 as f32;
                let page_h = self.page_dims.1 as f32;
                let (base_w, base_h) = if page_w > 0.0 && page_h > 0.0 {
                    (page_w, page_h)
                } else {
                    (800.0, 600.0)
                };
                let aspect_ratio = base_h / base_w;

                let effective_zoom = if in_slideshow {
                    ZoomMode::FitPage
                } else {
                    self.zoom_mode
                };

                const EDGE_PAD: f32 = 2.0;
                let (display_w, display_h) = match effective_zoom {
                    ZoomMode::Custom => {
                        let w = (visible_w - EDGE_PAD).max(100.0) * self.zoom_scale;
                        (w, w * aspect_ratio)
                    }
                    ZoomMode::FitWidth => {
                        let w = (visible_w - EDGE_PAD).max(100.0);
                        (w, w * aspect_ratio)
                    }
                    ZoomMode::FitHeight => {
                        let h = (visible_h - EDGE_PAD).max(100.0);
                        (h / aspect_ratio, h)
                    }
                    ZoomMode::FitPage => {
                        let max_w = (visible_w - EDGE_PAD).max(1.0);
                        let max_h = (visible_h - EDGE_PAD).max(1.0);
                        let scale = (max_w / base_w).min(max_h / base_h);
                        (base_w * scale, base_h * scale)
                    }
                };

                self.display_w_pt = display_w;
                self.last_effective_zoom = display_w / (visible_w - EDGE_PAD).max(1.0);

                // Keep the cursor or page center anchored during zoom.
            let anchor_screen = self.zoom_anchor.take(); // Consume the anchor even when zoom is unchanged.
            let zoom_now = (self.zoom_mode, self.zoom_scale);
            let zoom_changed = !in_slideshow
                && (zoom_now.0 != self.zoom_track.0 || (zoom_now.1 - self.zoom_track.1).abs() > 1e-4);
            let old_layout = self.last_layout;
            self.zoom_track = zoom_now;
            self.last_layout = (display_w, display_h, visible_w, visible_h);

            // None means no zoom change.
            // Some((Some(cursor_rel), old_layout)) means cursor-anchored zoom.
            // Some((None, old_layout)) means a fit-mode or UI zoom change.
            let zoom_fix: Option<(Option<egui::Vec2>, (f32, f32, f32, f32))> =
                if zoom_changed && old_layout.0 > 1.0 && old_layout.1 > 1.0 {
                    let cursor_anchor = anchor_screen
                        .filter(|p| self.canvas_rect.contains(*p))
                        .map(|p| p - self.canvas_rect.min);

                    Some((cursor_anchor, old_layout))
                } else {
                    None
                };

            // Middle-button panning follows the pointer.
            let (mid_pressed, mid_down, ptr_delta, ptr_pos) = ui.input(|i| {
                (
                    i.pointer.button_pressed(egui::PointerButton::Middle),
                    i.pointer.button_down(egui::PointerButton::Middle),
                    i.pointer.delta(),
                    i.pointer.hover_pos().or(i.pointer.latest_pos()),
                )
            });
            if mid_pressed && !in_slideshow && ptr_pos.is_some_and(|p| self.canvas_rect.contains(p)) {
                self.pdf_panning = true;
            }
            if !mid_down {
                self.pdf_panning = false;
            }
            let pan_speed = self.pan_speed();
            let pan_delta = if self.pdf_panning {
                ptr_delta * pan_speed
            } else {
                egui::Vec2::ZERO
            };
            // One wheel speed for every page view (single, continuous and book).
            if !in_slideshow
                && !self.pdf_panning
                && ptr_pos.is_some_and(|p| self.canvas_rect.contains(p))
            {
                self.apply_wheel_speed(ui, self.canvas_rect);
            }

                if self.pdf_panning {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                    ui.ctx().request_repaint();
                }

                // Floating Exit button in pure Slideshow mode when mouse is near top
                if self.view_mode == ViewMode::Slideshow {
                    let show_hud = ui
                        .input(|i| i.pointer.hover_pos())
                        .map(|p| p.y < 60.0)
                        .unwrap_or(false);
                    if show_hud {
                        let theme = &self.theme;

                        let to_color = |rgb: [u8; 3]| {
                            egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2])
                        };

                        egui::Area::new(ui.make_persistent_id("slideshow_hud"))
                            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 20.0))
                            .order(egui::Order::Foreground)
                            .show(ui.ctx(), |ui| {
                                egui::Frame::NONE
                                    .fill(to_color(theme.bg_elevated))
                                    .corner_radius(20)
                                    .stroke(egui::Stroke::new(
                                        1.0,
                                        to_color(theme.border),
                                    ))
                                    .inner_margin(egui::Margin::symmetric(14, 6))
                                    .shadow(egui::epaint::Shadow {
                                        offset: [0, 4],
                                        blur: 12,
                                        spread: 0,
                                        color: egui::Color32::from_black_alpha(100),
                                    })
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 10.0;

                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "Slide {} / {}",
                                                    self.current_page,
                                                    self.total_pages
                                                ))
                                                .color(to_color(theme.fg_primary))
                                                .strong(),
                                            );

                                            ui.separator();

                                            ui.scope(|ui| {
                                                let visuals = &mut ui.style_mut().visuals.widgets;

                                                visuals.hovered.weak_bg_fill =
                                                    to_color(theme.bg_hover);
                                                visuals.hovered.fg_stroke =
                                                    egui::Stroke::new(
                                                        1.0,
                                                        to_color(theme.accent),
                                                    );
                                                visuals.hovered.corner_radius =
                                                    egui::CornerRadius::same(12);

                                                visuals.active.weak_bg_fill =
                                                    to_color(theme.bg_active);
                                                visuals.active.fg_stroke =
                                                    egui::Stroke::new(
                                                        1.0,
                                                        to_color(theme.accent),
                                                    );
                                                visuals.active.corner_radius =
                                                    egui::CornerRadius::same(12);

                                                let exit_btn = egui::Button::new(
                                                    egui::RichText::new("󰅖 Exit")
                                                        .size(13.0)
                                                        .strong()
                                                        .color(to_color(theme.accent)),
                                                )
                                                .frame(false)
                                                .corner_radius(12);

                                                if ui
                                                    .add(exit_btn)
                                                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                                                    .on_hover_text("Press Esc to exit")
                                                    .clicked()
                                                {
                                                    self.view_mode = self.prev_view_mode;
                                                }
                                            });
                                        });
                                    });
                            });
                    }

                }

                else if self.view_mode == ViewMode::Continuous {
    ui.spacing_mut().item_spacing.y = 0.0;
    let gap = CONTINUOUS_GAP;
    let row_h = display_h + gap;
    let visible_doc_pages: Vec<u32> = (1..=self.total_pages)
        .filter(|p| !self.hidden_pages.contains(p))
        .collect();

    if visible_doc_pages.is_empty() {
        return;
    }
    let total_rows = visible_doc_pages.len();
    let row_pitch = row_h;
    let visible_w = ui.available_width();
    let visible_h = ui.available_height();

    let approx_viewport_center_y = ui.max_rect().top() + visible_h * 0.5;

    let mut best_page_by_dist: Option<(u32, f32)> = None;
    let mut vis_range: Option<(u32, u32)> = None;

    let scroll_src = if self.pdf_panning {
        ScrollSource {
            drag: DragScroll::Never,
            scroll_bar: true,
            mouse_wheel: true,
        }
    } else {
        ScrollSource::ALL
    };

    let mut scroll_area = egui::ScrollArea::both()
        .id_salt("pdf_continuous_scroll")
        .auto_shrink([false, false])
        .scroll_source(scroll_src);

    let max_scroll_y = (total_rows as f32 * row_pitch - visible_h).max(0.0);

    // The previous bottom-of-document special case was removed.


    let just_jumped = self.continuous_scroll_to_page.is_some();
    let switch_offset = self.mode_switch_offset.take();

    if let Some(target_p) = self.continuous_scroll_to_page.take() {
        let row_idx = visible_doc_pages
            .iter()
            .position(|&p| p >= target_p)
            .unwrap_or(0);

        // From Single mode, keep the position inside the page.
        let in_page_y = switch_offset.map_or(0.0, |o| o.y);
        let offset_y = ((row_idx as f32) * row_pitch + in_page_y).clamp(0.0, max_scroll_y);
        scroll_area = scroll_area.vertical_scroll_offset(offset_y);
        if let Some(o) = switch_offset {
            scroll_area = scroll_area.horizontal_scroll_offset(o.x);
        }

        self.jump_lock_y = Some(offset_y);
        self.jump_lock_frame = ui.ctx().cumulative_frame_nr();
        self.reanchor_left = 0; // Explicit navigation cancels pending reanchoring.
    }

    // Reanchor after zoom, resize, or fit-mode changes.
    let row_layout_changed = (row_h - self.continuous_prev_layout.2).abs() > 0.5
        || (display_w - self.continuous_prev_layout.0).abs() > 0.5
        || (display_h - self.continuous_prev_layout.1).abs() > 0.5;


    let needs_reanchor = !just_jumped && (zoom_fix.is_some() || row_layout_changed);

    if needs_reanchor {

    let from_zoom = zoom_fix.is_some();
    let cursor_y = zoom_fix.as_ref().and_then(|(a, _)| a.map(|a| a.y));

    if let (Some(cy), Some((_, old))) = (cursor_y, zoom_fix.as_ref()) {
        // Point under the cursor in the previous layout.
        let old_display_h = old.1.max(1.0);
        let old_pitch = old_display_h + gap;
        let doc_y = self.pdf_scroll_offset.y + cy;
        let idx = ((doc_y / old_pitch).floor() as isize)
            .clamp(0, total_rows as isize - 1) as usize;
        self.reanchor_page = visible_doc_pages[idx];
        self.reanchor_frac =
            ((doc_y - idx as f32 * old_pitch) / old_display_h).clamp(0.0, 1.0);
        self.reanchor_ref_y = cy;
    } else if from_zoom {
        // Fit, 100%, and keyboard zoom place the current page at the top.
        self.reanchor_page = self.current_page;
        self.reanchor_frac = 0.0;
        self.reanchor_ref_y = 0.0;
    } else {
        // Keep the current page at the top after resize.
        self.reanchor_page = self.continuous_anchor_page;
        self.reanchor_frac = self.continuous_anchor_frac;
        self.reanchor_ref_y = 0.0;
    }
    self.reanchor_left = 6;
    self.scroll_epoch = self.scroll_epoch.wrapping_add(1);
    }
        let user_scrolling = ui.input(|i| {
        let zooming = i.modifiers.ctrl || i.modifiers.command;
        (!zooming && i.smooth_scroll_delta.y.abs() > 0.5)
            || i.key_pressed(egui::Key::ArrowUp)
            || i.key_pressed(egui::Key::ArrowDown)
    }) || pan_delta != egui::Vec2::ZERO;

    if user_scrolling && !needs_reanchor {
        self.reanchor_left = 0;
        self.jump_lock_y = None;
    }
    // Keep the anchor locked throughout reanchoring.
    let reanchoring = self.reanchor_left > 0 && !just_jumped;
    let mut reanchor_desired: Option<f32> = None;
    if reanchoring {
        let new_row_idx = visible_doc_pages
            .iter()
            .position(|&p| p >= self.reanchor_page)
            .unwrap_or(visible_doc_pages.len() - 1);
        let new_offset_y = ((new_row_idx as f32) * row_pitch
            + self.reanchor_frac * display_h
            - self.reanchor_ref_y)
            .clamp(0.0, max_scroll_y);

        reanchor_desired = Some(new_offset_y);   // FUORI da qualsiasi if
        scroll_area = scroll_area.vertical_scroll_offset(new_offset_y);


        if self.reanchor_left == 6 {
            // Apply horizontal positioning only on the first frame.
            let anchor_x = zoom_fix
                .as_ref()
                .and_then(|(a, _)| a.map(|a| a.x))
                .unwrap_or(visible_w * 0.5);
            let (old_vis_w, old_disp_w) = zoom_fix
                .as_ref()
                .map(|(_, old)| (old.2, old.0))
                .unwrap_or((visible_w, display_w));
            let new_offset_x = zoom_scroll_offset(
                self.pdf_scroll_offset.x,
                anchor_x,
                old_vis_w,
                visible_w,
                old_disp_w,
                display_w,
            );
            reanchor_desired = Some(new_offset_y);
            scroll_area = scroll_area.horizontal_scroll_offset(new_offset_x);
        }

        self.pdf_scroll_offset.y = new_offset_y;
        self.jump_lock_y = Some(new_offset_y);
        self.jump_lock_frame = ui.ctx().cumulative_frame_nr();
        ui.ctx().request_repaint();
    }

    // Save the layout snapshot for the next frame.
    self.continuous_prev_layout = (display_w, display_h, row_h);

    scroll_area = scroll_area.id_salt(("pdf_continuous_scroll", self.scroll_epoch));
    let out = scroll_area.show_rows(ui, row_h, total_rows, |ui, row_range| {
        if pan_delta != egui::Vec2::ZERO {
            ui.scroll_with_delta(pan_delta);
            self.jump_lock_y = None;
            self.reanchor_left = 0;
        }

        if self.continuous_scroll_delta.abs() > 0.1 {
            ui.scroll_with_delta(egui::vec2(0.0, self.continuous_scroll_delta));
            self.continuous_scroll_delta = 0.0;
            self.jump_lock_y = None;
            self.reanchor_left = 0;
        }

        if !row_range.is_empty() {
            let first_p = visible_doc_pages[row_range.start];
            let last_p = visible_doc_pages[row_range.end.saturating_sub(1)];
            vis_range = Some((first_p, last_p));
        }

        for idx in row_range {
            let p = visible_doc_pages[idx];
            let margin = ((visible_w - display_w) * 0.5).max(0.0);

            ui.horizontal(|ui| {
                if effective_zoom == ZoomMode::FitPage {
                    ui.spacing_mut().item_spacing.x = 0.0;
                }
                if margin > 0.0 {
                    ui.add_space(margin);
                }

                let sense = egui::Sense::click_and_drag();
                let (slide_rect, slide_resp) =
                    ui.allocate_exact_size(egui::vec2(display_w, display_h), sense);

                let uv = egui::Rect::from_min_max(
                    egui::pos2(0.0, 0.0),
                    egui::pos2(1.0, 1.0),
                );
                if let Some((handle, _)) = self.viewer.page(p) {
                    ui.painter()
                        .image(handle.id(), slide_rect, uv, egui::Color32::WHITE);
                } else if let Some((thumb_h, _)) = self.viewer.thumbnail(p) {
                    ui.painter()
                        .image(thumb_h.id(), slide_rect, uv, egui::Color32::WHITE);
                } else {
                    ui.painter().rect_filled(
                        slide_rect,
                        egui::CornerRadius::same(2),
                        egui::Color32::WHITE,
                    );
                }

                self.handle_slide_annotations(ui, p, slide_rect, &slide_resp);

                let dist_to_center =
                    (slide_rect.center().y - approx_viewport_center_y).abs();
                if best_page_by_dist
                    .map(|(_, best_d)| dist_to_center < best_d)
                    .unwrap_or(true)
                {
                    best_page_by_dist = Some((p, dist_to_center));
                }
            });

            ui.add_space(gap);
        }
    });

    self.canvas_rect = out.inner_rect;


    self.pdf_scroll_offset = clamp_scroll_offset(
        out.state.clamped_offset(),
        out.content_size,
        out.inner_rect.size(),
    );

    if let Some(r) = vis_range {
        self.visible_pages = r;
    }

    let actual_y = out.state.clamped_offset().y;

    if reanchoring {
        let real_max = (out.content_size.y - out.inner_rect.height()).max(0.0);
        let expected_h = total_rows as f32 * row_pitch;
        let content_ok = (out.content_size.y - expected_h).abs() < 2.0;
        let at_target = reanchor_desired
            .is_none_or(|d| (actual_y - d.min(real_max)).abs() < 40.0);
        if content_ok && at_target {
            self.reanchor_left = 0;
            ui.ctx().request_repaint();
        } else {
            self.reanchor_left -= 1;
            ui.ctx().request_repaint();
        }
    }
    let gesture_active = ui.input(|i| {
        i.pointer.any_down() || i.smooth_scroll_delta.y.abs() > 0.5
    });
    let frames_since = ui
        .ctx()
        .cumulative_frame_nr()
        .saturating_sub(self.jump_lock_frame);

    if !reanchoring {
        // 1. Keep the scroll lock until the user scrolls.
        if let Some(lock_y) = self.jump_lock_y {
            if frames_since <= 1 {
                self.jump_lock_y = Some(actual_y);
            } else if gesture_active || (actual_y - lock_y).abs() > 1.5 {
                self.jump_lock_y = None;
            }
        }

        // 2. Keep the top-page anchor and fraction updated.
        let row_idx = ((actual_y / row_pitch.max(1.0)).floor() as isize)
            .clamp(0, total_rows as isize - 1) as usize;
        self.continuous_anchor_page = visible_doc_pages[row_idx];
        self.continuous_anchor_frac =
            ((actual_y - row_idx as f32 * row_pitch) / display_h.max(1.0)).clamp(0.0, 1.0);
    }

    // 3. At the bottom, select the last page;
    // otherwise select the page nearest the center.
    if !self.notes_panel_resizing
        && !reanchoring
        && !just_jumped
        && self.jump_lock_y.is_none()
        && self.continuous_scroll_to_page.is_none()
    {
        let max_y = (out.content_size.y - out.inner_rect.height()).max(0.0);
        let at_bottom = max_y > 0.0 && actual_y >= max_y - 2.0;
        let candidate = if at_bottom {
            visible_doc_pages.last().copied()
        } else {
            best_page_by_dist.map(|(p, _)| p)
        };

        if let Some(vis_p) = candidate
            && vis_p != self.current_page && (1..=self.total_pages).contains(&vis_p) {
                self.sync_and_save();
                self.record_lecture_page(vis_p);
                self.current_page = vis_p;
                self.target_page = vis_p;
                self.save_session_state();
                if !self.lock_note_page {
                    self.note_page = vis_p;
                    self.current_note = self.notes.get(&vis_p).cloned().unwrap_or_default();
                    self.note_is_dirty = false;
                }
                self.selected_textbox = None;
            }
    }
}
                else if self.view_mode == ViewMode::TwoPage {
                    let (left_opt, right_opt) = self.current_book_spread();
                    let spine_gap = 2.0;

                    let (book_page_w, book_page_h) = match self.zoom_mode {
                        ZoomMode::Custom => {
                            let w = ((visible_w - 4.0 - spine_gap) * 0.5).max(80.0) * self.zoom_scale;
                            (w, w * aspect_ratio)
                        }
                        ZoomMode::FitWidth => {
                            let w = ((visible_w - 4.0 - spine_gap) * 0.5).max(80.0);
                            (w, w * aspect_ratio)
                        }
                        ZoomMode::FitHeight => {
                            let h = (visible_h - 4.0).max(80.0);
                            (h / aspect_ratio, h)
                        }
                        ZoomMode::FitPage => {
                            let count = left_opt.iter().count() + right_opt.iter().count();
                            let gap = if count == 2 { spine_gap } else { 0.0 };
                            let max_single_w = ((visible_w - EDGE_PAD - gap) / count.max(1) as f32).max(1.0);
                            let max_h = (visible_h - EDGE_PAD).max(1.0);
                            let scale = (max_single_w / base_w).min(max_h / base_h);
                            (base_w * scale, base_h * scale)
                        }
                    };

                    self.display_w_pt = book_page_w;

                    match (left_opt, right_opt) {
                        (Some(l), Some(r)) => self.visible_pages = (l, r),
                        (None, Some(r)) => self.visible_pages = (r, r),
                        (Some(l), None) => self.visible_pages = (l, l),
                        (None, None) => {}
                    }

                    let num_pages_shown = left_opt.iter().count() + right_opt.iter().count();
                    let total_spread_w = if num_pages_shown == 2 {
                        book_page_w * 2.0 + spine_gap
                    } else {
                        book_page_w
                    };

                    let book_out = egui::ScrollArea::both()
                        .id_salt("pdf_book_twopage_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if pan_delta != egui::Vec2::ZERO {
                                ui.scroll_with_delta(pan_delta);
                            }
                            if effective_zoom == ZoomMode::FitPage {
                                ui.spacing_mut().item_spacing.y = 0.0;
                            }
                            let pad_x = ((visible_w - total_spread_w) * 0.5).max(0.0);
                            let pad_y = ((visible_h - book_page_h) * 0.5).max(0.0);

                            if pad_y > 0.0 {
                                ui.add_space(pad_y);
                            }

                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                if pad_x > 0.0 {
                                    ui.add_space(pad_x);
                                }

                                let pages_in_spread = [left_opt, right_opt];
                                for (slot_idx, p_opt) in pages_in_spread.iter().enumerate() {
                                    let Some(p) = *p_opt else {
                                        continue;
                                    };

                                    // Spine
                                    if slot_idx == 1 && left_opt.is_some() {
                                        let (spine_rect, _) = ui.allocate_exact_size(
                                            egui::vec2(spine_gap, book_page_h),
                                            egui::Sense::hover(),
                                        );
                                        let center_x = spine_rect.center().x;
                                        ui.painter().line_segment(
                                            [
                                                egui::pos2(center_x, spine_rect.top() + 4.0),
                                                egui::pos2(center_x, spine_rect.bottom() - 4.0),
                                            ],
                                            egui::Stroke::new(
                                                2.0,
                                                egui::Color32::from_black_alpha(110),
                                            ),
                                        );
                                    }

                                    let sense = egui::Sense::click_and_drag();
                                    let (slide_rect, slide_resp) = ui.allocate_exact_size(
                                        egui::vec2(book_page_w, book_page_h),
                                        sense,
                                    );

                                    if slide_resp.clicked() && self.current_page != p {
                                        self.sync_and_save();
                                        self.record_lecture_page(p);
                                        self.current_page = p;
                                        self.target_page = p;
                                        self.save_session_state();
                                        if !self.lock_note_page {
                                            self.note_page = p;
                                            self.current_note =
                                                self.notes.get(&p).cloned().unwrap_or_default();
                                            self.note_is_dirty = false;
                                        }
                                    }

                                    if let Some((handle, _)) = self.viewer.page(p) {
                                        ui.painter().image(
                                            handle.id(),
                                            slide_rect,
                                            egui::Rect::from_min_max(
                                                egui::pos2(0.0, 0.0),
                                                egui::pos2(1.0, 1.0),
                                            ),
                                            egui::Color32::WHITE,
                                        );
                                    } else if let Some((thumb_h, _)) = self.viewer.thumbnail(p) {
                                        ui.painter().image(
                                            thumb_h.id(),
                                            slide_rect,
                                            egui::Rect::from_min_max(
                                                egui::pos2(0.0, 0.0),
                                                egui::pos2(1.0, 1.0),
                                            ),
                                            egui::Color32::WHITE,
                                        );
                                    } else {
                                        ui.painter().rect_filled(
                                            slide_rect,
                                            egui::CornerRadius::same(3),
                                            egui::Color32::WHITE,
                                        );
                                    }

                                    // Border for active
                                    let is_active_side = p == self.current_page;
                                    ui.painter().rect_stroke(
                                        slide_rect,
                                        egui::CornerRadius::same(2),
                                        egui::Stroke::new(
                                            if is_active_side { 1.5 } else { 1.0 },
                                            if is_active_side {
                                                rgb(self.theme.accent)
                                            } else {
                                                rgb(self.theme.border)
                                            },
                                        ),
                                        egui::StrokeKind::Inside,
                                    );

                                    self.handle_slide_annotations(ui, p, slide_rect, &slide_resp);
                                }
                            });
                        });
                    self.canvas_rect = book_out.inner_rect;
                    return;
                }
                if self.view_mode == ViewMode::SinglePage || self.view_mode == ViewMode::Slideshow || self.view_mode == ViewMode::SlideshowWithNotes  {
                    let mut canvas_scroll = egui::ScrollArea::both()
                        .id_salt("pdf_canvas_scroll")
                        .auto_shrink([false, false]);

                    if let Some((opt_anchor, old)) = zoom_fix {
                        // `old` = (display_w, display_h, visible_w, visible_h) of the previous frame.
                        // Cursor-anchored zoom (Ctrl + wheel or touchpad pinch) keeps the page
                        // point under the cursor fixed; fit modes and UI zoom keep the view center.
                        let anchor_y = opt_anchor.map(|a| a.y).unwrap_or(visible_h * 0.5);
                        let y = zoom_scroll_offset(
                            self.pdf_scroll_offset.y,
                            anchor_y,
                            old.3,
                            visible_h,
                            old.1,
                            display_h,
                        );

                        let anchor_x = opt_anchor.map(|a| a.x).unwrap_or(visible_w * 0.5);
                        let x = zoom_scroll_offset(
                            self.pdf_scroll_offset.x,
                            anchor_x,
                            old.2,
                            visible_w,
                            old.0,
                            display_w,
                        );

                        canvas_scroll = canvas_scroll.scroll_offset(egui::vec2(x, y));
                    }

                    // Coming from Continuous mode: keep the position inside the page.
                    let pending_offset = self.mode_switch_offset.take();
                    if let Some(o) = pending_offset {
                        canvas_scroll = canvas_scroll.scroll_offset(o);
                    }

                    // A wheel turn must not discard the next frame's scroll events.
                    let wheel_reset = std::mem::take(&mut self.single_page_wheel.reset_offset);
                    if wheel_reset && pending_offset.is_none() && zoom_fix.is_none() {
                        canvas_scroll = canvas_scroll.scroll_offset(egui::Vec2::ZERO);
                    }
                    let old_offset = if wheel_reset { 0.0 } else { self.pdf_scroll_offset.y };
                    let wheel = if self.view_mode == ViewMode::SinglePage
                        && !zoom_changed
                        && zoom_fix.is_none()
                        && pending_offset.is_none()
                        && !self.notes_panel_resizing
                        && !self.pdf_panning
                        && !self.trackpad_drawing_active
                        && !self.show_settings
                        && !self.show_search
                        && !self.show_command_palette
                        && !self.show_flashcards_modal
                        && self.radial_menu_pos.is_none()
                    {
                        ui.input(|i| {
                            if i.modifiers.any()
                                || i.pointer.any_down()
                                || self.app_state.shortcuts.trackpad_draw_hold.is_held(i)
                            {
                                return None;
                            }
                            // Include native momentum, but not egui's synthetic smoothing tail.
                            let mut d = egui::Vec2::ZERO;
                            for event in &i.events {
                                if let egui::Event::MouseWheel {
                                    unit, delta, modifiers, ..
                                } = event
                                    && !modifiers.any()
                                {
                                    let scale = match unit {
                                        egui::MouseWheelUnit::Point => 1.0,
                                        egui::MouseWheelUnit::Line | egui::MouseWheelUnit::Page => {
                                            SinglePageWheel::POINTS_PER_PAGE
                                        }
                                    };
                                    d += *delta * scale;
                                }
                            }
                            let speed = self.wheel_speed() / state::DEFAULT_WHEEL_SCROLL_SPEED;
                            (d.y.abs() >= d.x.abs()).then_some((i.time, d.y * speed))
                        })
                    } else {
                        None
                    };
                    let out = canvas_scroll.show_viewport(ui, |ui, viewport| {
                        if pan_delta != egui::Vec2::ZERO {
                            ui.scroll_with_delta(pan_delta);
                        }
                        self.pdf_scroll_offset = viewport.min.to_vec2();

                        if let Some(handle) = &self.texture_handle {
                            let sized = egui::load::SizedTexture::new(
                                handle.id(),
                                egui::vec2(display_w, display_h),
                            );

                            let sense = if current_texture_ready {
                                egui::Sense::click_and_drag()
                            } else {
                                egui::Sense::hover()
                            };

                            // Keep centering padding independent of widget spacing.
                            if effective_zoom == ZoomMode::FitPage {
                                ui.spacing_mut().item_spacing.y = 0.0;
                            }
                            let pad_x = ((visible_w - display_w) * 0.5).max(0.0);
                            let pad_y = ((visible_h - display_h) * 0.5).max(0.0);

                            if pad_y > 0.0 {
                                ui.add_space(pad_y);
                            }

                            let img_resp = ui
                                .horizontal(|ui| {
                                    if effective_zoom == ZoomMode::FitPage {
                                        ui.spacing_mut().item_spacing.x = 0.0;
                                    }
                                    if pad_x > 0.0 {
                                        ui.add_space(pad_x);
                                    }
                                    ui.add(egui::Image::from_texture(sized).sense(sense))
                                })
                                .inner;

                            // The retained image is only visual; never draw or select on it.
                            if current_texture_ready {
                                self.handle_slide_annotations(
                                    ui,
                                    self.current_page,
                                    img_resp.rect,
                                    &img_resp,
                                );
                            }
                        } else {
                            // On first open there is no previous texture to display.
                            ui.allocate_exact_size(
                                egui::vec2(display_w, display_h), egui::Sense::hover(),
                            );
                        }
                    });

                    // Needed to turn the pointer position into a canvas-relative zoom anchor.
                    self.canvas_rect = out.inner_rect;

                    self.pdf_scroll_offset = clamp_scroll_offset(
                        out.state.clamped_offset(),
                        out.content_size,
                        out.inner_rect.size(),
                    );
                    if let Some((time, delta)) = wheel
                        && ui.rect_contains_pointer(out.inner_rect)
                    {
                        let max_y = if effective_zoom == ZoomMode::FitPage {
                            0.0
                        } else {
                            (out.content_size.y - out.inner_rect.height()).max(0.0)
                        };
                        let offset = old_offset.clamp(0.0, max_y);
                        let steps = self.single_page_wheel.update(time, delta, offset, max_y);
                        if steps != 0 {
                            let target = wheel_page_target(
                                self.current_page,
                                self.total_pages,
                                &self.hidden_pages,
                                steps,
                            );
                            if target != self.current_page {
                                // Commit only the destination, not each skipped page.
                                let mut gesture = self.single_page_wheel;
                                self.go_to_page(target, ui.ctx());
                                gesture.reset_offset = true;
                                self.single_page_wheel = gesture;
                                self.pdf_scroll_offset = egui::Vec2::ZERO;
                                ui.input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
                                ui.ctx().request_repaint();
                            } else {
                                self.single_page_wheel = SinglePageWheel::default();
                            }
                        }
                    } else {
                        self.single_page_wheel = SinglePageWheel::default();
                    }
                }
            });

        // Search
        if self.show_search && self.has_pdf() {
            self.render_search_popup(ui);
        } else if self.show_search {
            self.close_search();
        }

        //Command Palette
        if self.show_command_palette {
            self.render_command_palette(ui);
        }

        //Flashcards
        if self.show_flashcards_modal && self.has_pdf() {
            self.render_flashcards_modal(ui);
        }

        // Advanced Settings
        if self.show_settings {
            self.render_settings_modal(ui);
        }

        // Toast Notification
        let mut clear_toast = false;
        if let Some((msg, time)) = &self.toast_message {
            const TOTAL_DURATION: f32 = 3.0;
            const FADE_IN: f32 = 0.18;
            const FADE_START: f32 = 2.6;
            let elapsed = time.elapsed().as_secs_f32();

            if elapsed < TOTAL_DURATION {
                // Compute alpha with a quick fade-in and smooth fade-out.
                let alpha = if elapsed < FADE_IN {
                    egui::emath::remap_clamp(elapsed, 0.0..=FADE_IN, 0.0..=1.0)
                } else if elapsed >= FADE_START {
                    egui::emath::remap_clamp(elapsed, FADE_START..=TOTAL_DURATION, 1.0..=0.0)
                } else {
                    1.0
                };
                // Use cubic easing for smooth motion.
                let smooth_alpha = 1.0 - (1.0 - alpha).powi(3);

                // Repaint while progress and animations are active.
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(16));

                // Separate the Nerd Font icon from the text and choose its color.
                let (icon, clean_msg, accent_rgb) = if let Some(rest) = msg.strip_prefix("󰄬") {
                    ("󰄬", rest.trim_start(), rgb(self.theme.success))
                } else if let Some(rest) = msg.strip_prefix("󰅚") {
                    ("󰅚", rest.trim_start(), rgb(self.theme.error))
                } else if let Some(rest) = msg.strip_prefix("󰀪") {
                    ("󰀪", rest.trim_start(), rgb(self.theme.warning))
                } else {
                    ("󰋼", msg.as_str(), rgb(self.theme.warning))
                };

                let bg_color = rgb(self.theme.bg_panel).gamma_multiply(smooth_alpha * 0.98);
                let accent_color = accent_rgb.gamma_multiply(smooth_alpha);
                let badge_bg = accent_rgb.gamma_multiply(smooth_alpha * 0.16);
                let border_color = rgb(self.theme.border).gamma_multiply(smooth_alpha);
                let text_color = rgb(self.theme.fg_primary).gamma_multiply(smooth_alpha);
                let shadow_color = egui::Color32::from_black_alpha(88).gamma_multiply(smooth_alpha);

                // Slide-up in entrata e leggero slide-down in uscita
                let y_offset = -24.0 - self.bottom_bars_h() + (1.0 - smooth_alpha) * 12.0;

                egui::Area::new(ui.make_persistent_id("toast_notification"))
                    .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(24.0, y_offset))
                    .order(egui::Order::Foreground)
                    .interactable(false)
                    .show(ui.ctx(), |ui| {
                        let frame_resp = egui::Frame::NONE
                            .fill(bg_color)
                            .stroke(egui::Stroke::new(1.0, border_color))
                            .corner_radius(egui::CornerRadius::same(
                                self.theme.corner_radius.saturating_add(3),
                            ))
                            .inner_margin(egui::Margin::symmetric(14, 9))
                            .shadow(egui::epaint::Shadow {
                                spread: 0,
                                blur: 16,
                                color: shadow_color,
                                offset: [0, 6],
                            })
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 12.0;

                                    // Circular icon badge.
                                    let (rect, _) = ui.allocate_exact_size(
                                        egui::vec2(26.0, 26.0),
                                        egui::Sense::hover(),
                                    );
                                    ui.painter().circle_filled(rect.center(), 13.0, badge_bg);
                                    ui.painter().text(
                                        rect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        icon,
                                        egui::FontId::proportional(15.0),
                                        accent_color,
                                    );

                                    // Message text.
                                    ui.label(
                                        egui::RichText::new(clean_msg)
                                            .color(text_color)
                                            .size(13.5)
                                            .strong(),
                                    );

                                    ui.add_space(4.0);
                                });
                            });

                        // Thin progress bar at the bottom of the toast.
                        let progress = (1.0 - (elapsed / TOTAL_DURATION)).clamp(0.0, 1.0);
                        let rect = frame_resp.response.rect;
                        let bar_margin = 10.0;
                        let max_bar_w = (rect.width() - bar_margin * 2.0).max(0.0);
                        let bar_rect = egui::Rect::from_min_size(
                            egui::pos2(rect.left() + bar_margin, rect.bottom() - 2.5),
                            egui::vec2(max_bar_w * progress, 2.0),
                        );
                        ui.painter().rect_filled(
                            bar_rect,
                            egui::CornerRadius::same(1),
                            accent_rgb.gamma_multiply(smooth_alpha * 0.65),
                        );
                    });
            } else {
                clear_toast = true;
            }
        }
        if clear_toast {
            self.toast_message = None;
        }

        let ctx = ui.ctx().clone();
        self.pump_render_requests(&ctx);

        if self.radial_menu_pos.is_some() && self.has_pdf() {
            self.render_radial_tools_menu(ui);
        }
        self.paint_annotation_cursor(&ctx);
        // End-of-frame local-only checkpoint. A SIGKILL/pkill -9 cannot invoke
        // on_exit, so the most recent completed UI action must already be on disk.
        if self.has_pdf() {
            let snapshot = self.local_checkpoint_hash();
            if snapshot.is_some() && snapshot != self.last_local_checkpoint_hash {
                let backing_off = self
                    .checkpoint_failed_at
                    .is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(2));
                if backing_off {
                    ctx.request_repaint_after(std::time::Duration::from_secs(2));
                } else {
                    self.sync_and_save();
                }
            }
        }
    }
}

//! Opening, closing, and changing the active PDF.

use super::*;

pub(super) fn merge_imported_notes(
    notes: &mut HashMap<u32, String>,
    imported: impl IntoIterator<Item = (u32, String)>,
    note_page: u32,
) -> String {
    notes.extend(imported);
    notes.get(&note_page).cloned().unwrap_or_default()
}

pub(super) fn has_extra_study_content(
    sketch: Option<&notes::SketchPage>,
    memos: Option<&Vec<audio::VoiceMemoEntry>>,
) -> bool {
    sketch.is_some_and(|s| !s.strokes.is_empty()) || memos.is_some_and(|m| !m.is_empty())
}

pub(super) fn page_has_notes_or_sketch(
    page: u32,
    note_page: u32,
    current_note: &str,
    notes: &HashMap<u32, String>,
    sketches: &HashMap<u32, notes::SketchPage>,
) -> bool {
    let has_note = if page == note_page {
        !current_note.trim().is_empty()
    } else {
        notes.get(&page).is_some_and(|text| !text.trim().is_empty())
    };
    has_note
        || sketches
            .get(&page)
            .is_some_and(|sketch| !sketch.strokes.is_empty())
}

/// A page change that would invalidate a digital signature, waiting for the user's decision.
#[derive(Clone, Copy)]
pub(super) enum SignedPageOp {
    Insert {
        after_page: u32,
        style: notes::BlankPageStyle,
    },
    Delete {
        page: u32,
    },
    Reorder {
        from: u32,
        to: u32,
    },
}

impl Lapis {
    /// Page changes rewrite the whole file, which invalidates a digital signature. Ask first;
    /// nothing was written when this is shown.
    pub(super) fn show_signed_page_modal(&mut self, ui: &mut egui::Ui) {
        let Some(op) = self.pending_signed_page_op else {
            return;
        };
        if !self.has_pdf() {
            self.pending_signed_page_op = None;
            return;
        }
        let ctx = ui.ctx().clone();
        let mut proceed = false;
        let mut cancel = false;
        egui::Modal::new(ui.make_persistent_id("signed_page_change"))
            .frame(self.modal_glass_frame(242))
            .show(&ctx, |ui| {
                ui.set_max_width(440.0);
                ui.heading("This PDF is digitally signed");
                ui.label("Inserting, deleting or reordering pages rewrites the file and invalidates its signature. Other viewers will report the signature as broken.");
                ui.weak("Your notes and annotations are not affected. Lapis keeps a copy of the original file in its data folder, but keep your own copy if you need the signature.");
                ui.add_space(10.0);
                proceed = ui.button("Change pages anyway").clicked();
                cancel = ui.button("Cancel").clicked();
            });
        if proceed || cancel {
            self.pending_signed_page_op = None;
            ctx.request_repaint();
        }
        if proceed {
            self.allow_signature_loss = true;
            match op {
                SignedPageOp::Insert { after_page, style } => {
                    self.insert_blank_page_after(after_page, style, &ctx)
                }
                SignedPageOp::Delete { page } => self.delete_pdf_page_and_shift_data(page, &ctx),
                SignedPageOp::Reorder { from, to } => self.reorder_pdf_page(from, to, &ctx),
            }
            self.allow_signature_loss = false;
        }
    }

    fn pdf_structure_busy(&mut self) -> bool {
        if self.audio_engine.is_recording() || self.is_transcribing {
            self.audio_toast("Finish recording or transcription before changing PDF pages");
            return true;
        }
        if self.pdf_save_job.is_some()
            || self.queued_pdf_save.is_some()
            || self.pdf_save_failure.is_some()
        {
            self.toast_message = Some((
                "PDF is saving; retry the page action shortly".into(),
                std::time::Instant::now(),
            ));
            true
        } else {
            false
        }
    }

    pub(super) fn open_pdf(&mut self, path: PathBuf, ctx: &egui::Context) -> bool {
        // Do not open a document already locked by another window.
        // Multiple instances would overwrite notes and annotations.
        let same_doc =
            self.has_pdf() && doc_lock::canonical(&path) == doc_lock::canonical(&self.pdf_path);
        let new_lock = if same_doc && self.doc_lock.is_some() {
            None // The current window already owns the lock.
        } else {
            match doc_lock::acquire(&path) {
                Ok(l) => Some(l),
                Err(doc_lock::LockError::Busy) => {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    self.toast_message = Some((
                        format!("\"{name}\" is already open in another Lapis window"),
                        std::time::Instant::now(),
                    ));
                    return false;
                }
                Err(doc_lock::LockError::Io(_)) => None, // Continue if locking is unavailable.
            }
        };

        let (total_pages, outline, first_page_pt) = match inspect_pdf(&path) {
            Ok(info) => info,
            Err(err) => {
                self.audio_toast(format!("Could not open PDF: {err}"));
                ctx.request_repaint();
                return false;
            }
        };
        if let Err(err) = AudioDocIndex::recover_page_change(&path) {
            self.audio_toast(format!("Could not open the PDF's audio index: {err}"));
            ctx.request_repaint();
            return false;
        }
        if !self.finish_audio_before_transition() {
            ctx.request_repaint();
            return false;
        }
        self.save_session_state();
        self.flush_session_state();
        // Only release the old lock after the candidate has been validated.
        if !same_doc {
            self.doc_lock = new_lock;
        } else if let Some(l) = new_lock {
            self.doc_lock = Some(l);
        }

        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Unknown".to_string());

        self.invalidate_page_jobs();
        self.single_page_wheel = SinglePageWheel::default();
        let doc_data = notes::load_doc_data(&path);
        self.notes = doc_data.notes;
        self.annotations = doc_data.annotations;
        self.active_stroke = None;
        self.selected_textbox = None;
        self.pdf_search_index.clear();
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.revealed_tapes.clear();
        self.laser_trail.clear();
        self.blank_pages = doc_data.blank_pages;
        self.sketches = doc_data.sketches;
        self.sketch.reset();
        let saved_entry = self.app_state.find_entry(&path).cloned();
        let start_page = saved_entry
            .as_ref()
            .map(|e| e.last_page.clamp(1, total_pages))
            .unwrap_or(1);

        if let Some(entry) = saved_entry {
            self.zoom_mode = entry.zoom_mode;
            self.zoom_scale = entry.zoom_scale;
            self.show_notes = if self.app_state.settings.restore_show_notes {
                entry.show_notes
            } else {
                false
            };
            self.show_toc = if self.app_state.settings.restore_show_toc {
                entry.show_toc
            } else {
                false
            };
            self.user_bookmarks = entry.bookmarks;
            self.pdf_dark_mode = entry.pdf_dark_mode;
            self.hidden_pages = entry.hidden_pages;
            self.view_mode = match entry.view_mode {
                ViewMode::Slideshow | ViewMode::SlideshowWithNotes => ViewMode::SinglePage,
                mode => mode,
            };
            self.prev_view_mode = self.view_mode;
            self.panel_sizes = entry.panel_sizes;
        } else {
            self.user_bookmarks.clear();
            self.hidden_pages.clear();
            self.panel_sizes = PanelSizes::default();
        }
        // Panels pick up this document's sizes when they are next shown.
        self.panel_apply = [true; 3];
        self.panel_seen = [None; 3];
        self.panel_sizes_dirty = false;
        self.notes_panel_resizing = false;
        self.fit_notes_layout_pending = false;

        self.audio_index = AudioDocIndex::load(&path);
        self.audio_index_pending = false;

        self.pending_delete_page = None;
        self.last_layout = (0.0, 0.0, 0.0, 0.0);
        self.visible_pages = (0, 0);
        self.checkpointed_revision = None;
        self.checkpoint_failed_at = None;
        self.last_external_merge = None;
        self.pending_signed_page_op = None;
        self.pdf_path = path.clone();
        self.pdf_name = name;
        self.total_pages = total_pages;
        self.outline = outline;
        self.current_page = start_page;
        self.target_page = start_page;
        self.current_note = self.notes.get(&start_page).cloned().unwrap_or_default();
        self.slide_rects.clear();
        self.last_slide_rects.clear();
        self.pdf_selection = None;
        self.selecting = false;
        self.texture_handle = None;
        self.page_dims = (0, 0);
        self.is_rendering = true;
        self.note_page = start_page;
        self.lock_note_page = false;
        self.audio_dock_open = false;
        self.audio_dock_height = 0.0;

        self.page_pt = first_page_pt;
        self.viewer.start(path, ctx);
        // Do not reset a restored continuous-view session to page one.
        self.continuous_anchor_page = start_page;
        self.continuous_anchor_frac = 0.0;
        self.continuous_scroll_to_page = if self.view_mode == ViewMode::Continuous {
            Some(start_page)
        } else {
            None
        };
        self.continuous_prev_layout = (0.0, 0.0, 0.0);
        self.jump_lock_y = None;
        self.flashcards_deck.clear();
        self.flashcard_idx = 0;

        self.save_session_state();
        self.flush_session_state();
        ctx.request_repaint();
        true
    }

    /// Open a user-requested document unless it is already open.
    pub(super) fn open_pdf_user(&mut self, path: PathBuf, ctx: &egui::Context) {
        if self.has_pdf() && doc_lock::canonical(&path) == doc_lock::canonical(&self.pdf_path) {
            self.toast_message = Some((
                "This file is already open".to_string(),
                std::time::Instant::now(),
            ));
            return;
        }
        if self.has_pdf() {
            self.request_pdf_save(AfterPdfSave::Open(path), ctx);
        } else {
            self.open_pdf(path, ctx);
        }
    }

    pub(super) fn close_pdf(&mut self) {
        self.single_page_wheel = SinglePageWheel::default();
        if !self.has_pdf() {
            return;
        }
        self.save_session_state();
        self.flush_session_state();
        // Release the per-document lock, otherwise the file stays marked as open
        // (other windows get "already open" and the storage cleanup stays disabled).
        self.doc_lock = None;

        self.invalidate_page_jobs();
        self.pdf_path = PathBuf::new();
        self.pdf_name.clear();
        self.texture_handle = None;
        self.slide_rects.clear();
        self.last_slide_rects.clear();
        self.pdf_selection = None;
        self.selecting = false;
        self.viewer.close();
        self.notes.clear();
        self.annotations.clear();
        self.sketches.clear();
        self.sketch.reset();
        self.current_note.clear();
        self.selected_textbox = None;
        self.is_rendering = false;
        self.pdf_search_index.clear();
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.revealed_tapes.clear();
        self.laser_trail.clear();
    }

    pub(super) fn add_or_update_bookmark(&mut self, page: u32, title: String) {
        let clean = if title.trim().is_empty() {
            format!("Page {}", page)
        } else {
            title.trim().to_string()
        };
        if let Some(existing) = self.user_bookmarks.iter_mut().find(|b| b.page == page) {
            existing.title = clean;
        } else {
            self.user_bookmarks
                .push(state::BookmarkItem { title: clean, page });
            self.user_bookmarks.sort_unstable_by_key(|b| b.page);
        }
        self.save_session_state();
    }

    pub(super) fn remove_bookmark(&mut self, page: u32) {
        self.user_bookmarks.retain(|b| b.page != page);
        self.save_session_state();
    }

    pub(super) fn insert_blank_page_after(
        &mut self,
        after_page: u32,
        style: notes::BlankPageStyle,
        ctx: &egui::Context,
    ) {
        if self.pdf_structure_busy() {
            return;
        }
        if !self.has_pdf() {
            return;
        }
        self.finish_sketch_edit();
        if !self.checkpoint_before_structure_change() {
            return;
        }
        let original_pages = self.total_pages;
        // Retire the old render session before rewriting the PDF.
        self.viewer.stop_workers();

        match notes::insert_blank_page_with_style_into_pdf(
            &self.pdf_path,
            after_page,
            style,
            self.allow_signature_loss,
            Some(&self.audio_index),
        ) {
            Ok(new_page_num) => {
                self.invalidate_page_jobs();
                notes::shift_doc_data_on_insert(
                    &mut self.notes,
                    &mut self.annotations,
                    &mut self.blank_pages,
                    &mut self.sketches,
                    new_page_num,
                );
                self.sketch.reset();
                if !self.blank_pages.contains(&new_page_num) {
                    self.blank_pages.insert(new_page_num);
                }

                self.hidden_pages = self
                    .hidden_pages
                    .iter()
                    .map(|&p| if p >= new_page_num { p + 1 } else { p })
                    .collect();
                for bm in &mut self.user_bookmarks {
                    if bm.page >= new_page_num {
                        bm.page += 1;
                    }
                }

                // Keep the current note aligned with the updated PDF.
                self.current_note = self.notes.get(&self.note_page).cloned().unwrap_or_default();

                self.save_study_edit();
                if self.current_page >= new_page_num {
                    self.current_page += 1;
                }
                if self.target_page >= new_page_num {
                    self.target_page += 1;
                }
                if self.note_page >= new_page_num {
                    self.note_page += 1;
                }
                if self.split_secondary_page >= new_page_num {
                    self.split_secondary_page += 1;
                }
                if self.continuous_anchor_page >= new_page_num {
                    self.continuous_anchor_page += 1;
                }
                self.reanchor_page = self.continuous_anchor_page;
                self.current_note = self.notes.get(&self.note_page).cloned().unwrap_or_default();
                self.refresh_pdf_after_page_structure_change(ctx);
                self.current_page = new_page_num;
                self.target_page = new_page_num;
                self.note_page = new_page_num;
                self.current_note = self.notes.get(&new_page_num).cloned().unwrap_or_default();
                self.continuous_scroll_to_page = Some(new_page_num);
                self.save_session_state();
                self.finish_audio_page_change(
                    notes::PageChange::Insert(new_page_num),
                    original_pages,
                    "󰄬 Blank page added",
                );
            }
            Err(e) => {
                self.refresh_pdf_after_page_structure_change(ctx);
                if notes::is_signed_pdf_error(&e) {
                    self.pending_signed_page_op = Some(SignedPageOp::Insert { after_page, style });
                } else {
                    self.toast_message = Some((
                        format!("󰅚 Insert page error: {}", e),
                        std::time::Instant::now(),
                    ));
                }
            }
        }
    }

    pub(super) fn is_disposable_blank_page(&self, page: u32) -> bool {
        self.blank_pages.contains(&page)
            && self.annotations.get(&page).is_none_or(|a| a.is_empty())
            && if page == self.note_page {
                self.current_note.trim().is_empty()
            } else {
                self.notes.get(&page).is_none_or(|n| n.trim().is_empty())
            }
            && self
                .sketches
                .get(&page)
                .is_none_or(|sketch| sketch.is_default())
            && self
                .audio_index
                .page_memos
                .get(&page)
                .is_none_or(Vec::is_empty)
    }

    pub(super) fn request_delete_page(&mut self, page: u32, ctx: &egui::Context) {
        if !self.has_pdf() {
            return;
        }
        if self.total_pages <= 1
            || self.is_disposable_blank_page(page)
            || self.pending_delete_page == Some(page)
        {
            self.pending_delete_page = None;
            self.delete_pdf_page_and_shift_data(page, ctx);
        } else {
            self.pending_delete_page = Some(page);
            self.toast_message = Some((
                format!("Delete page {page}? Run the command again to confirm (Esc to cancel)"),
                std::time::Instant::now(),
            ));
        }
    }

    pub(super) fn delete_pdf_page_and_shift_data(&mut self, page_to_del: u32, ctx: &egui::Context) {
        if self.pdf_structure_busy() {
            return;
        }
        if !self.has_pdf()
            || self.total_pages <= 1
            || page_to_del == 0
            || page_to_del > self.total_pages
        {
            self.toast_message = Some((
                "󰅚 Cannot delete this page.".to_string(),
                std::time::Instant::now(),
            ));
            return;
        }

        self.finish_sketch_edit();
        if !self.checkpoint_before_structure_change() {
            return;
        }
        let original_pages = self.total_pages;
        // Retire the old render session before rewriting the PDF.
        self.viewer.stop_workers();
        let old_anchor_page = self.continuous_anchor_page;
        let old_anchor_frac = self.continuous_anchor_frac;
        let old_current = self.current_page;
        let old_note_page = self.note_page;
        let old_split_secondary = self.split_secondary_page;

        match notes::delete_page_from_pdf(
            &self.pdf_path,
            page_to_del,
            self.allow_signature_loss,
            Some(&self.audio_index),
        ) {
            Ok(()) => {
                self.invalidate_page_jobs();
                let change = notes::PageChange::Delete(page_to_del);
                let shift = |page| change.map(page);
                self.notes = std::mem::take(&mut self.notes)
                    .into_iter()
                    .filter_map(|(p, v)| shift(p).map(|np| (np, v)))
                    .collect();
                self.annotations = std::mem::take(&mut self.annotations)
                    .into_iter()
                    .filter_map(|(p, v)| shift(p).map(|np| (np, v)))
                    .collect();
                self.blank_pages = self.blank_pages.iter().copied().filter_map(shift).collect();
                self.sketches = std::mem::take(&mut self.sketches)
                    .into_iter()
                    .filter_map(|(p, v)| shift(p).map(|np| (np, v)))
                    .collect();
                self.sketch.reset();
                self.hidden_pages = self
                    .hidden_pages
                    .iter()
                    .copied()
                    .filter_map(shift)
                    .collect();
                self.user_bookmarks.retain_mut(|bm| match shift(bm.page) {
                    Some(np) => {
                        bm.page = np;
                        true
                    }
                    None => false,
                });

                let new_total = self.total_pages.saturating_sub(1).max(1);
                let keep_page = |p: u32| -> u32 {
                    if p > page_to_del {
                        p - 1
                    } else if p == page_to_del {
                        page_to_del.saturating_sub(1).max(1).min(new_total)
                    } else {
                        p.min(new_total).max(1)
                    }
                };
                self.current_page = keep_page(old_current);
                self.target_page = self.current_page;
                self.continuous_anchor_page = keep_page(old_anchor_page);
                self.reanchor_page = self.continuous_anchor_page;
                self.reanchor_frac = old_anchor_frac;
                self.reanchor_ref_y = 0.0;
                self.reanchor_left = 6;
                self.scroll_epoch = self.scroll_epoch.wrapping_add(1);
                self.note_page = keep_page(old_note_page);
                self.split_secondary_page = keep_page(old_split_secondary);
                self.current_note = self.notes.get(&self.note_page).cloned().unwrap_or_default();
                self.undo_stack.clear();
                self.redo_stack.clear();
                self.revealed_tapes.clear();
                self.selected_image = None;
                self.pending_delete_page = None;
                self.save_study_edit();
                self.refresh_pdf_after_page_structure_change(ctx);
                self.finish_audio_page_change(
                    notes::PageChange::Delete(page_to_del),
                    original_pages,
                    format!("󰆴 Deleted page {page_to_del}"),
                );
            }
            Err(e) => {
                self.refresh_pdf_after_page_structure_change(ctx);
                if notes::is_signed_pdf_error(&e) {
                    self.pending_signed_page_op = Some(SignedPageOp::Delete { page: page_to_del });
                } else {
                    self.toast_message = Some((
                        format!("󰅚 Delete page failed: {e}"),
                        std::time::Instant::now(),
                    ));
                }
            }
        }
    }

    pub(super) fn reorder_pdf_page(&mut self, from_page: u32, to_page: u32, ctx: &egui::Context) {
        if self.pdf_structure_busy() {
            return;
        }
        if !self.has_pdf()
            || from_page == to_page
            || from_page == 0
            || to_page == 0
            || from_page > self.total_pages
            || to_page > self.total_pages
        {
            return;
        }

        self.finish_sketch_edit();
        if !self.checkpoint_before_structure_change() {
            return;
        }
        let original_pages = self.total_pages;
        self.viewer.stop_workers();
        let old_anchor_page = self.continuous_anchor_page;
        let old_anchor_frac = self.continuous_anchor_frac;
        match notes::reorder_pages_in_pdf(
            &self.pdf_path,
            from_page,
            to_page,
            self.allow_signature_loss,
            Some(&self.audio_index),
        ) {
            Ok(()) => {
                self.invalidate_page_jobs();
                let change = notes::PageChange::Reorder {
                    from: from_page,
                    to: to_page,
                };
                let remap = |page| change.map(page).unwrap();
                self.notes = std::mem::take(&mut self.notes)
                    .into_iter()
                    .map(|(p, v)| (remap(p), v))
                    .collect();
                self.annotations = std::mem::take(&mut self.annotations)
                    .into_iter()
                    .map(|(p, v)| (remap(p), v))
                    .collect();
                self.blank_pages = self.blank_pages.iter().copied().map(remap).collect();
                self.sketches = std::mem::take(&mut self.sketches)
                    .into_iter()
                    .map(|(p, v)| (remap(p), v))
                    .collect();
                self.sketch.reset();
                self.hidden_pages = self.hidden_pages.iter().copied().map(remap).collect();
                for bm in &mut self.user_bookmarks {
                    bm.page = remap(bm.page);
                }

                self.current_page = remap(self.current_page);
                self.target_page = remap(self.target_page);
                self.continuous_anchor_page = remap(old_anchor_page);
                self.reanchor_page = self.continuous_anchor_page;
                self.reanchor_frac = old_anchor_frac;
                self.reanchor_ref_y = 0.0;
                self.reanchor_left = 6;
                self.scroll_epoch = self.scroll_epoch.wrapping_add(1);
                self.note_page = remap(self.note_page);
                self.split_secondary_page = remap(self.split_secondary_page);
                self.revealed_tapes = std::mem::take(&mut self.revealed_tapes)
                    .into_iter()
                    .map(|(p, i)| (remap(p), i))
                    .collect();
                self.current_note = self.notes.get(&self.note_page).cloned().unwrap_or_default();
                self.save_study_edit();
                self.save_session_state();
                self.refresh_pdf_after_page_structure_change(ctx);
                self.finish_audio_page_change(
                    change,
                    original_pages,
                    format!("󰄬 Moved page {from_page} to position {to_page}"),
                );
            }
            Err(e) => {
                self.refresh_pdf_after_page_structure_change(ctx);
                if notes::is_signed_pdf_error(&e) {
                    self.pending_signed_page_op = Some(SignedPageOp::Reorder {
                        from: from_page,
                        to: to_page,
                    });
                } else {
                    self.toast_message = Some((
                        format!("󰅚 Page reorder failed: {e}"),
                        std::time::Instant::now(),
                    ));
                }
            }
        }
    }

    fn invalidate_page_jobs(&mut self) {
        self.note_editor_epoch = self.note_editor_epoch.wrapping_add(1);
        self.textbox_epoch = self.textbox_epoch.wrapping_add(1);
        self.shots.cancel_pending();
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.selected_textbox = None;
        self.selected_image = None;
        self.image_drag = None;
        self.active_stroke = None;
        self.active_snip_rect = None;
        self.pending_delete_page = None;
        self.pdf_selection = None;
        self.selecting = false;
        self.slide_rects.clear();
        self.last_slide_rects.clear();
    }

    /// Restart the render workers while preserving the workspace layout.
    fn refresh_pdf_after_page_structure_change(&mut self, ctx: &egui::Context) {
        let path = self.pdf_path.clone();
        let (total_pages, outline, current_page_pt) =
            match Document::open(path.to_str().unwrap_or_default()) {
                Ok(doc) => {
                    let total = doc.page_count().unwrap_or(1).max(1) as u32;
                    let page_idx = self.current_page.saturating_sub(1).min(total - 1) as i32;
                    let pt = doc
                        .load_page(page_idx)
                        .and_then(|p| p.bounds())
                        .map(|b| (b.x1 - b.x0, b.y1 - b.y0))
                        .unwrap_or((0.0, 0.0));
                    (total, outline(&doc), pt)
                }
                Err(_) => (self.total_pages, self.outline.clone(), self.page_pt),
            };

        self.total_pages = total_pages;
        self.current_page = self.current_page.clamp(1, total_pages);
        self.target_page = self.current_page;
        self.note_page = self.note_page.clamp(1, total_pages);
        self.outline = outline;
        self.page_pt = current_page_pt;
        self.pdf_search_index.clear();
        self.visible_pages = (0, 0);
        self.texture_handle = None;
        self.is_rendering = true;
        self.current_note = self.notes.get(&self.note_page).cloned().unwrap_or_default();
        self.viewer.start(path, ctx);
        self.save_session_state();
        ctx.request_repaint();
    }

    pub(super) fn prompt_download_original_pdf(&mut self, ctx: &egui::Context) {
        if !self.has_pdf() {
            return;
        }
        let stem = self
            .pdf_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("document");
        let default_name = format!("{}_original.pdf", stem);

        self.dialogs.open(
            ctx,
            DialogKind::ExportOriginalPdf,
            DialogSpec::save_file()
                .file_name(default_name)
                .filter("PDF Document", &["pdf"]),
        );
    }

    pub(super) fn finish_download_original_pdf(&mut self, dest: PathBuf) {
        if !self.has_pdf() {
            return;
        }
        match notes::export_original_pdf(&self.pdf_path, &dest) {
            Ok(()) => {
                self.toast_message = Some((
                    format!("󰇚 Original PDF saved to {}", dest.display()),
                    std::time::Instant::now(),
                ));
            }
            Err(e) => {
                self.toast_message = Some((
                    format!("󰅚 Failed to save original PDF: {}", e),
                    std::time::Instant::now(),
                ));
            }
        }
    }
}

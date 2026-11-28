use super::*;
use crate::persistence::Completion;
use std::time::{Duration, Instant};

const SESSION_SAVE_INTERVAL: Duration = Duration::from_millis(250);
const SAVE_RETRY_INTERVAL: Duration = Duration::from_secs(2);

pub(super) struct PdfSaveJob {
    pub(super) worker: thread::JoinHandle<std::io::Result<()>>,
    pub(super) after: AfterPdfSave,
    pub(super) snapshot_revision: u64,
}

pub(super) struct PdfSaveFailure {
    pub(super) after: AfterPdfSave,
    pub(super) message: String,
    pub(super) checkpoint_ok: bool,
}

impl Lapis {
    pub(super) fn save_session_state(&mut self) {
        if !self.has_pdf() {
            return;
        }
        let view_mode = self.reading_view_mode();
        self.saved_view_mode = view_mode;
        let changed = self.app_state.upsert_recent(RecentFileEntry {
            path: self.pdf_path.clone(),
            name: self.pdf_name.clone(),
            last_page: self.current_page,
            total_pages: self.total_pages,
            zoom_mode: self.zoom_mode,
            zoom_scale: self.zoom_scale,
            show_notes: self.show_notes,
            show_toc: self.show_toc,
            bookmarks: self.user_bookmarks.clone(),
            hidden_pages: self.hidden_pages.clone(),
            view_mode,
            pdf_dark_mode: self.pdf_dark_mode,
            panel_sizes: self.panel_sizes,
        });
        if changed {
            self.session_save_due
                .get_or_insert_with(|| Instant::now() + SESSION_SAVE_INTERVAL);
        }
    }

    pub(super) fn flush_session_state(&mut self) {
        if self.session_save_due.take().is_some()
            && let Err(err) = self.app_state.try_save()
        {
            eprintln!("Lapis: could not save session state: {err}");
            self.session_save_due = Some(Instant::now() + SAVE_RETRY_INTERVAL);
        }
    }

    pub(super) fn service_session_state(&mut self, ctx: &egui::Context) {
        if self
            .session_save_due
            .is_some_and(|due| due <= Instant::now())
        {
            self.flush_session_state();
        }
        if let Some(due) = self.session_save_due {
            ctx.request_repaint_after(due.saturating_duration_since(Instant::now()));
        }
    }

    pub(super) fn mark_study_changed(&mut self) {
        self.study_revision = self.study_revision.wrapping_add(1);
    }

    pub(super) fn save_study_edit(&mut self) {
        self.mark_study_changed();
        self.sync_and_save();
    }

    fn editor_has_changes(&self) -> bool {
        match self.notes.get(&self.note_page) {
            Some(saved) => *saved != self.current_note,
            None => !self.current_note.trim().is_empty(),
        }
    }

    pub(super) fn has_uncheckpointed_edits(&self) -> bool {
        self.checkpointed_revision != Some(self.study_revision) || self.editor_has_changes()
    }

    pub(super) fn checkpoint_after_frame(&mut self, ctx: &egui::Context) {
        if !self.has_pdf() || !self.has_uncheckpointed_edits() {
            return;
        }
        // Completed edits reach the local backup before the next frame.
        if let Some(left) = self
            .checkpoint_failed_at
            .and_then(|at| SAVE_RETRY_INTERVAL.checked_sub(at.elapsed()))
        {
            ctx.request_repaint_after(left);
        } else {
            self.sync_and_save();
        }
    }

    pub(super) fn sync_and_save(&mut self) {
        if !self.has_pdf() || !self.has_uncheckpointed_edits() {
            return;
        }
        match self.sync_local_checkpoint() {
            Ok(()) => self.checkpoint_failed_at = None,
            Err(err) => {
                eprintln!("Lapis: failed to checkpoint document locally: {err}");
                if self.checkpoint_failed_at.is_none() {
                    self.toast_message = Some((
                        format!("󰀪 Local backup failed: {err}"),
                        std::time::Instant::now(),
                    ));
                }
                self.checkpoint_failed_at = Some(std::time::Instant::now());
            }
        }
    }

    /// Page edits rebuild the PDF from the local backup, so it must be current first.
    pub(super) fn checkpoint_before_structure_change(&mut self) -> bool {
        let checkpoint = self
            .sync_local_checkpoint()
            .and_then(|()| AudioDocIndex::recover_page_change(&self.pdf_path));
        match checkpoint {
            Ok(()) => true,
            Err(err) => {
                self.toast_message = Some((
                    format!("󰅚 Page change cancelled: local backup failed ({err})"),
                    std::time::Instant::now(),
                ));
                false
            }
        }
    }

    /// Checkpoint the imported annotations before acknowledging the source revision.
    pub(super) fn merge_external_into_model(&mut self) -> std::io::Result<()> {
        self.finish_sketch_edit();
        self.mark_study_changed();
        let observed = notes::merge_external_annotations(&self.pdf_path, &mut self.annotations)?;
        self.sync_local_checkpoint()?;
        notes::mark_pdf_observed(&self.pdf_path, observed)?;
        self.undo_stack.clear();
        self.redo_stack.clear();
        Ok(())
    }

    pub(super) fn sync_local_checkpoint(&mut self) -> std::io::Result<()> {
        if !self.has_pdf() {
            return Ok(());
        }
        if self.editor_has_changes() {
            if self.current_note.trim().is_empty() {
                self.notes.remove(&self.note_page);
            } else {
                self.notes.insert(self.note_page, self.current_note.clone());
            }
            self.mark_study_changed();
        }
        notes::save_doc_data(
            &self.pdf_path,
            &self.notes,
            &self.annotations,
            self.blank_pages.clone(),
            &self.sketches,
        )?;
        self.checkpointed_revision = Some(self.study_revision);
        self.checkpoint_failed_at = None;
        Ok(())
    }

    // Start next frame, after all edits in this frame have been checkpointed.
    pub(super) fn request_pdf_save(&mut self, after: AfterPdfSave, ctx: &egui::Context) {
        if !self.has_pdf() {
            return;
        }
        if let Some(failure) = &mut self.pdf_save_failure {
            persistence::request_during_failure(&mut failure.after, after);
        } else if let Some(job) = &mut self.pdf_save_job {
            persistence::request_during_job(&mut job.after, &mut self.queued_pdf_save, after);
        } else {
            persistence::request_when_idle(&mut self.queued_pdf_save, after);
        }
        ctx.request_repaint();
    }

    fn start_pdf_save(&mut self, after: AfterPdfSave, ctx: &egui::Context) {
        self.finish_sketch_edit();
        if self.has_uncheckpointed_edits()
            && let Err(err) = self.sync_local_checkpoint()
        {
            self.pdf_save_failed(after, err.to_string(), false);
            return;
        }
        self.save_session_state();
        self.flush_session_state();
        if let Err(err) = AudioDocIndex::recover_page_change(&self.pdf_path) {
            self.pdf_save_failed(
                after,
                format!("Could not recover the audio page mapping: {err}"),
                true,
            );
            return;
        }
        let path = self.pdf_path.clone();
        let snapshot = match notes::prepare_pdf_commit(
            &path,
            &self.notes,
            &self.annotations,
            self.blank_pages.clone(),
            &self.sketches,
        ) {
            Ok(data) => data,
            Err(err) => {
                self.pdf_save_failed(after, err.to_string(), true);
                return;
            }
        };
        let snapshot_revision = self.study_revision;
        let repaint = ctx.clone();
        match thread::Builder::new()
            .name("lapis-pdf-save".into())
            .spawn(move || {
                let result = notes::commit_pdf_snapshot(&path, snapshot);
                repaint.request_repaint();
                result
            }) {
            Ok(worker) => {
                self.pdf_save_job = Some(PdfSaveJob {
                    worker,
                    after,
                    snapshot_revision,
                });
                self.toast_message = Some(("Saving PDF…".into(), std::time::Instant::now()));
            }
            Err(err) => self.pdf_save_failed(after, err.to_string(), true),
        }
    }

    fn pdf_save_failed(&mut self, after: AfterPdfSave, message: String, checkpoint_ok: bool) {
        if matches!(&after, AfterPdfSave::Stay) {
            self.toast_message = Some((
                format!("PDF save failed: {message}"),
                std::time::Instant::now(),
            ));
        } else {
            self.pdf_save_failure = Some(PdfSaveFailure {
                after,
                message,
                checkpoint_ok,
            });
        }
    }

    pub(super) fn complete_pdf_save(&mut self, after: AfterPdfSave, ctx: &egui::Context) -> bool {
        if matches!(&after, AfterPdfSave::Home | AfterPdfSave::Exit)
            && !self.finish_audio_before_transition()
        {
            ctx.request_repaint();
            return false;
        }
        match after {
            AfterPdfSave::Stay => {
                self.toast_message = Some(("󰄬 Saved".into(), std::time::Instant::now()));
            }
            AfterPdfSave::Home => {
                self.close_pdf();
                self.toast_message = Some(("󰄬 Saved".into(), std::time::Instant::now()));
            }
            AfterPdfSave::Open(path) => {
                self.toast_message = Some(("󰄬 Saved".into(), std::time::Instant::now()));
                return self.open_pdf(path, ctx);
            }
            AfterPdfSave::Exit => {
                self.save_session_state();
                self.flush_session_state();
                // Send Close in the next frame, separate from any CancelClose command.
                self.allow_window_close = true;
            }
        }
        ctx.request_repaint();
        true
    }

    // Keep the workspace live while the worker commits an immutable snapshot.
    pub(super) fn service_pdf_save(&mut self, ui: &mut egui::Ui) -> bool {
        let ctx = ui.ctx().clone();
        if self.allow_window_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return true;
        }
        if self.has_pdf() && ui.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if let Some(failure) = &mut self.pdf_save_failure {
                failure.after = AfterPdfSave::Exit;
            } else {
                self.request_pdf_save(AfterPdfSave::Exit, &ctx);
            }
        }
        if self
            .pdf_save_job
            .as_ref()
            .is_some_and(|job| job.worker.is_finished())
        {
            let job = self.pdf_save_job.take().unwrap();
            let queued = self.queued_pdf_save.take().is_some();
            match job.worker.join() {
                Ok(Ok(())) => {
                    if !matches!(&job.after, AfterPdfSave::Stay) || queued {
                        self.finish_sketch_edit();
                    }
                    let current_revision =
                        (!self.editor_has_changes()).then_some(self.study_revision);
                    match persistence::on_commit_success(
                        &job.after,
                        queued,
                        Some(job.snapshot_revision),
                        current_revision,
                    ) {
                        Completion::Followup => self.request_pdf_save(job.after, &ctx),
                        Completion::Done { newer_edits } => {
                            if self.complete_pdf_save(job.after, &ctx) && newer_edits {
                                self.toast_message = Some((
                                    "󰄬 PDF saved; newer edits are local".into(),
                                    std::time::Instant::now(),
                                ));
                            }
                        }
                    }
                }
                Ok(Err(err)) if notes::is_external_modification(&err) => {
                    let repeated = self
                        .last_external_merge
                        .is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(5));
                    self.last_external_merge = Some(std::time::Instant::now());
                    match self.merge_external_into_model() {
                        Ok(()) if !repeated => {
                            self.toast_message = Some((
                                "PDF changed outside Lapis; its annotations were merged".into(),
                                std::time::Instant::now(),
                            ));
                            self.request_pdf_save(job.after, &ctx);
                        }
                        Ok(()) => self.pdf_save_failed(job.after, err.to_string(), true),
                        Err(merge_err) => {
                            self.pdf_save_failed(job.after, merge_err.to_string(), false)
                        }
                    }
                }
                Ok(Err(err)) => self.pdf_save_failed(job.after, err.to_string(), true),
                Err(_) => {
                    self.pdf_save_failed(job.after, "Save worker stopped unexpectedly".into(), true)
                }
            }
            if self.allow_window_close {
                return true;
            }
        }
        if self.pdf_save_job.is_none() && self.pdf_save_failure.is_none() {
            if let Some(after) = self.queued_pdf_save.take() {
                self.start_pdf_save(after, &ctx);
            }
        }
        if self.pdf_save_job.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(40));
        }
        if let Some(failure) = self.pdf_save_failure.take() {
            let mut retry = false;
            let mut keep_working = false;
            let mut continue_local = false;
            egui::Modal::new(ui.make_persistent_id("pdf_save_failure"))
                .frame(self.modal_glass_frame(242))
                .show(&ctx, |ui| {
                ui.set_max_width(440.0);
                ui.heading("Could not save PDF");
                ui.label(&failure.message);
                if failure.checkpoint_ok {
                    ui.weak("Your edits are in the local backup, but may not be embedded in the PDF.");
                } else {
                    ui.weak("The local backup could not be updated. Retry or keep working.");
                }
                ui.add_space(10.0);
                retry = ui.button("Retry Save").clicked();
                keep_working = ui.button("Keep working").clicked();
                if failure.checkpoint_ok {
                    continue_local = ui.button("Continue with local backup only").clicked();
                }
            });
            if retry {
                self.request_pdf_save(failure.after, &ctx);
            } else if continue_local {
                self.finish_sketch_edit();
                match self.sync_local_checkpoint() {
                    Ok(()) => {
                        self.save_session_state();
                        if self.complete_pdf_save(failure.after, &ctx) {
                            self.toast_message = Some((
                                "󰀪 PDF not saved; local backup kept".into(),
                                std::time::Instant::now(),
                            ));
                        }
                    }
                    Err(err) => self.pdf_save_failed(failure.after, err.to_string(), false),
                }
            } else if !keep_working {
                self.pdf_save_failure = Some(failure);
            }
            if retry || continue_local || keep_working {
                ctx.request_repaint();
            }
            return self.allow_window_close;
        }
        false
    }
}

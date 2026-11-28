//! Document audio workflows and their controls.

use super::*;

pub(super) fn replace_asr_channel(
    rx: &mut Receiver<audio::AsrJobResult>,
    cancel: &mut Arc<std::sync::atomic::AtomicBool>,
) -> Sender<audio::AsrJobResult> {
    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    let (tx, next_rx) = channel();
    *rx = next_rx;
    *cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    tx
}

/// Text of every slide (PDF text + the user's note for that slide), used to align the transcript.
fn extract_slide_texts(
    pdf_path: &Path,
    total_pages: u32,
    notes_map: &HashMap<u32, String>,
) -> Vec<String> {
    let mut slide_texts = Vec::with_capacity(total_pages as usize);
    if let Ok(doc) = Document::open(pdf_path.to_str().unwrap_or_default()) {
        for p_idx in 0..(total_pages as i32) {
            let mut combined = String::new();
            if let Ok(page) = doc.load_page(p_idx)
                && let Ok(tp) = page.to_text_page(mupdf::TextPageFlags::empty())
            {
                for block in tp.blocks() {
                    for line in block.lines() {
                        for ch in line.chars() {
                            if let Some(c) = ch.char() {
                                combined.push(c);
                            }
                        }
                        combined.push(' ');
                    }
                }
            }
            if let Some(user_note) = notes_map.get(&((p_idx + 1) as u32)) {
                for line in user_note.lines() {
                    let t = line.trim_start();
                    if t.starts_with("- **[") || t.contains("Lecture Transcript") {
                        continue;
                    }
                    combined.push(' ');
                    combined.push_str(line);
                }
            }
            slide_texts.push(combined);
        }
    }
    slide_texts
}

impl Lapis {
    pub(super) fn audio_toast(&mut self, msg: impl Into<String>) {
        self.toast_message = Some((msg.into(), std::time::Instant::now()));
    }

    /// Each worker owns a fresh channel and token, even when reopening the same PDF.
    pub(super) fn new_asr_job(
        &mut self,
    ) -> (
        Sender<audio::AsrJobResult>,
        Arc<std::sync::atomic::AtomicBool>,
    ) {
        let tx = replace_asr_channel(&mut self.asr_rx, &mut self.asr_cancel);
        (tx, self.asr_cancel.clone())
    }

    fn retire_asr_job(&mut self) {
        drop(replace_asr_channel(&mut self.asr_rx, &mut self.asr_cancel));
        self.is_transcribing = false;
        self.asr_progress = None;
    }

    /// Finalize the recording and persist its index before releasing the document.
    pub(super) fn finish_audio_before_transition(&mut self) -> bool {
        if !self.has_pdf() {
            return true;
        }
        if self.audio_engine.is_recording() {
            let dir = AudioDocIndex::media_dir_for_pdf(&self.pdf_path);
            if let Err(err) = self.finish_active_recording(&dir) {
                self.audio_toast(format!("Could not finish recording: {err}"));
                return false;
            }
        }
        if self.audio_index_pending || (self.is_transcribing && self.asr_progress.is_some()) {
            if let Err(err) = self.audio_index.save_checked(&self.pdf_path) {
                self.audio_toast(format!("Could not save audio index: {err}"));
                return false;
            }
            self.audio_index_pending = false;
        }
        self.retire_asr_job();
        self.audio_engine.shutdown_output();
        self.memo_audio_engine.shutdown_output();
        self.transcript_last_seg = None;
        self.show_audio_trimmer = false;
        self.trim_memo_target = None;
        self.rename_memo_target = None;
        self.show_align_panel = false;
        self.dialogs = Dialogs::default(); // Results from old document dialogs are obsolete.
        true
    }

    pub(super) fn persist_audio_index(&mut self) -> Result<(), String> {
        self.audio_index_pending = true;
        self.audio_index.save_checked(&self.pdf_path)?;
        self.audio_index_pending = false;
        Ok(())
    }

    fn publish_audio_index(
        &mut self,
        index: AudioDocIndex,
        media_dir: &Path,
        retired: Option<&str>,
    ) -> Result<(), String> {
        index.save_checked(&self.pdf_path)?;
        self.audio_index = index;
        self.audio_index_pending = false;
        if let Some(name) = retired
            && !self.audio_index.references_wav(name)
            && let Err(err) = remove_media_file(media_dir, name)
        {
            eprintln!("Lapis: could not remove retired audio: {err}");
        }
        Ok(())
    }

    pub(super) fn record_lecture_page(&mut self, page: u32) {
        if self.audio_engine.rec_target == Some(RecordingTarget::Lecture) {
            self.recording_timeline
                .push((self.audio_engine.recording_elapsed_ms(), page));
        }
    }

    pub(super) fn report_audio_index_result(&mut self, message: impl Into<String>) {
        match self.persist_audio_index() {
            Ok(()) => self.audio_toast(message),
            Err(err) => self.audio_toast(format!("Audio index save failed: {err}")),
        }
    }

    fn start_memo_recording(&mut self, media_dir: &Path, page: u32) {
        let raw = audio::recording_path(media_dir, "memo_raw");
        if let Err(err) = self
            .audio_engine
            .start_recording(raw, RecordingTarget::PageMemo(page))
        {
            self.audio_toast(format!("Mic error: {err}"));
        }
    }

    fn finish_active_recording(&mut self, media_dir: &Path) -> Result<(), String> {
        let target = self
            .audio_engine
            .rec_target
            .clone()
            .ok_or("No active recording")?;
        let prefix = match target {
            RecordingTarget::Lecture => "lecture",
            RecordingTarget::PageMemo(_) => "memo",
        };
        let final_wav = audio::recording_path(media_dir, prefix);
        let (target, path, dur) = self.audio_engine.stop_recording(&final_wav)?;
        let name = path
            .file_name()
            .ok_or("Invalid recording filename")?
            .to_string_lossy()
            .into_owned();
        let raw_fallback = path != final_wav;
        let mut replaced_lecture = None;
        match target {
            RecordingTarget::Lecture => {
                replaced_lecture = self.audio_index.lecture_wav.replace(name);
                self.audio_index.lecture_duration_ms = dur;
                self.audio_index.waveform_peaks = audio::compute_waveform_peaks(&path, 240);
                self.audio_index.timeline = std::mem::take(&mut self.recording_timeline);
                for (time, _) in &mut self.audio_index.timeline {
                    *time = (*time).min(dur);
                }
                self.audio_index.timeline_is_live = true;
                self.audio_index.segments.clear();
                self.audio_index.page_range = self
                    .audio_index
                    .timeline
                    .iter()
                    .map(|t| t.1)
                    .min()
                    .zip(self.audio_index.timeline.iter().map(|t| t.1).max());
            }
            RecordingTarget::PageMemo(page) => {
                // The recording belongs to the page where the microphone was started.
                let list = self.audio_index.page_memos.entry(page).or_default();
                let id = (1..)
                    .map(|n| format!("Memo #{n}"))
                    .find(|id| !list.iter().any(|m| &m.id == id))
                    .unwrap();
                list.push(audio::VoiceMemoEntry {
                    id,
                    filename: name,
                    duration_ms: dur,
                    transcript: None,
                    title: None,
                });
            }
        }
        self.persist_audio_index().map_err(|err| {
            format!(
                "Recording kept at {}; audio index save failed: {err}",
                path.display()
            )
        })?;
        if let Some(old) = replaced_lecture
            && !self.audio_index.references_wav(&old)
        {
            // Remove the replaced recording only after the new one and its index are safe.
            let _ = remove_media_file(media_dir, &old);
        }
        self.preload_asr_model();
        self.audio_toast(if raw_fallback {
            "Recording saved as raw WAV (conversion unavailable)"
        } else {
            "Recording saved"
        });
        Ok(())
    }

    /// Loads the ASR model in the background so the first transcription starts right away.
    fn preload_asr_model(&self) {
        if let Some(model) = audio::find_asr_model() {
            thread::spawn(move || audio::preload_asr_model(&model));
        }
    }

    fn toggle_lecture_recording(&mut self, media_dir: &Path) {
        if self.is_transcribing {
            return;
        }
        self.confirm_remove_lecture = false;
        if self.audio_engine.rec_target == Some(RecordingTarget::Lecture) {
            if let Err(err) = self.finish_active_recording(media_dir) {
                self.audio_toast(format!("Could not finish recording: {err}"));
            }
        } else if !self.audio_engine.is_recording() {
            self.audio_engine.stop_and_release();
            let raw_wav = audio::recording_path(media_dir, "lecture_raw");
            match self
                .audio_engine
                .start_recording(raw_wav, RecordingTarget::Lecture)
            {
                Ok(()) => {
                    self.recording_timeline = vec![(0, self.current_page)];
                }
                Err(e) => self.audio_toast(format!("󰅚 Mic error: {}", e)),
            }
        }
    }

    fn import_lecture_media(&mut self, media_dir: &Path, ctx: &egui::Context) {
        self.dialogs.open(
            ctx,
            DialogKind::ImportLectureMedia {
                media_dir: media_dir.to_path_buf(),
            },
            DialogSpec::open_file().filter(
                "Audio & Video",
                &["mp4", "mkv", "webm", "mp3", "m4a", "wav", "ogg", "flac"],
            ),
        );
    }

    pub(super) fn finish_import_lecture_media(&mut self, src_path: PathBuf, media_dir: &Path) {
        if !self.has_pdf()
            || self.is_transcribing
            || self.audio_engine.is_recording()
            || AudioDocIndex::media_dir_for_pdf(&self.pdf_path) != media_dir
        {
            self.audio_toast("Audio import cancelled: document or recording changed");
            return;
        }
        self.confirm_remove_lecture = false;
        let out_wav = audio::recording_path(media_dir, "lecture");
        let result = (|| {
            let duration = audio::convert_media_to_16k_wav(&src_path, &out_wav)?;
            self.install_imported_lecture(media_dir, &out_wav, duration)?;
            Ok::<_, String>(duration)
        })();
        match result {
            Ok(dur) => {
                self.show_audio_trimmer = true;
                self.trim_start_ms = 0;
                self.trim_end_ms = dur;
                self.trim_memo_target = None;
                let playback = self
                    .audio_engine
                    .load_and_play(&out_wav, "Lecture".to_string(), 0);
                self.preload_asr_model();
                match playback {
                    Ok(()) => self.audio_toast("󰄬 Lecture imported"),
                    Err(err) => self
                        .audio_toast(format!("Lecture imported. Playback could not start: {err}")),
                }
            }
            Err(err) => {
                let _ = std::fs::remove_file(&out_wav);
                self.audio_toast(format!("󰅚 Lecture import failed: {err}"));
            }
        }
    }

    pub(super) fn install_imported_lecture(
        &mut self,
        media_dir: &Path,
        wav: &Path,
        duration: u64,
    ) -> Result<(), String> {
        let mut index = self.audio_index.clone();
        let old = index.lecture_wav.replace(
            wav.file_name()
                .ok_or("Invalid WAV path")?
                .to_string_lossy()
                .into_owned(),
        );
        index.lecture_duration_ms = duration;
        index.waveform_peaks = audio::compute_waveform_peaks(wav, 240);
        index.page_range = None;
        index.segments.clear();
        index.timeline.clear();
        index.timeline_is_live = false;
        self.audio_engine.stop_and_release();
        self.publish_audio_index(index, media_dir, old.as_deref())
    }

    pub(super) const AUDIO_BTN_SIZE: f32 = 22.0;
    /// Empty space under the rendered Markdown in the notes Preview.
    pub(super) const PREVIEW_BOTTOM_PAD: f32 = 28.0;

    pub(super) fn render_audio_dock_in_rect(&mut self, ui: &mut egui::Ui, show_ticker: bool) {
        let media_dir = AudioDocIndex::media_dir_for_pdf(&self.pdf_path);
        let mut close_dock_clicked = false;

        if self.audio_dock_open {
            egui::Frame::NONE
                .fill(ui.visuals().panel_fill)
                .stroke(egui::Stroke::NONE)
                .corner_radius(egui::CornerRadius::ZERO)
                .inner_margin(egui::Margin {
                    left: 8,
                    right: 8,
                    top: 1,
                    bottom: 1,
                })
                .show(ui, |ui| {
                    let avail_w = ui.available_width();
                    ui.set_width(avail_w);
                    ui.spacing_mut().item_spacing.y = 1.0;

                    // Header fisso
                    if self.show_audio_handle {
                        self.render_audio_handle(ui);
                    }

                    ui.horizontal(|ui| {
                        let row_w = ui.available_width();
                        let btn_space = Self::AUDIO_BTN_SIZE + 8.0;
                        let title_max_w = (row_w - btn_space).max(20.0);

                        let header_title = if row_w < 145.0 {
                            "Audio"
                        } else {
                            "Audio recording"
                        };

                        ui.add_sized(
                            [title_max_w, 20.0],
                            egui::Label::new(
                                egui::RichText::new(header_title)
                                    .size(10.5)
                                    .strong()
                                    .color(rgb(self.theme.fg_primary)),
                            )
                            .truncate(),
                        );

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let response = Self::audio_fab_button(
                                ui,
                                "×",
                                rgb(self.theme.accent),
                                rgb(self.theme.bg_elevated),
                                rgb(self.theme.border),
                                rgb(self.theme.fg_primary),
                                rgb(self.theme.fg_muted),
                                true,
                                true,
                                Self::AUDIO_BTN_SIZE,
                                "Close Lesson Audio",
                                self.audio_index.lecture_wav.is_some(),
                            );

                            close_dock_clicked = response.clicked();
                        });
                    });

                    ui.separator();

                    // Fill the remaining vertical space.
                    let body_height = ui.available_height();
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), body_height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            self.render_audio_body(ui, &media_dir);
                        },
                    );
                });
        } else if show_ticker {
            // Full-width transcript row.
            egui::Frame::NONE
                .fill(ui.visuals().panel_fill)
                .stroke(egui::Stroke::NONE)
                .inner_margin(egui::Margin::symmetric(6, 3))
                .show(ui, |ui| {
                    self.render_compact_transcript_ticker(ui, &media_dir);
                });
        }

        if close_dock_clicked {
            self.audio_dock_open = false;
        }
    }

    fn render_compact_transcript_ticker(&mut self, ui: &mut egui::Ui, media_dir: &Path) {
        let wav = self
            .audio_index
            .lecture_wav
            .as_ref()
            .map(|name| media_dir.join(name));

        let playing = self.audio_engine.is_playing();
        let pos_ms = self.audio_engine.current_pos_ms();
        let is_transcribing = self.is_transcribing;

        // ---------------------------------------------------------
        // Current transcript segment.
        // ---------------------------------------------------------
        let seg_opt = if playing || pos_ms > 0 {
            self.audio_index
                .segment_index_at(pos_ms)
                .and_then(|i| self.audio_index.segments.get(i))
                .or_else(|| {
                    self.audio_index
                        .segments
                        .iter()
                        .find(|s| s.page == self.current_page)
                })
        } else {
            self.audio_index
                .segments
                .iter()
                .find(|s| s.page == self.current_page)
        }
        .map(|s| (s.start_ms, s.text.clone()));

        // ---------------------------------------------------------
        // Transcription progress.
        // ---------------------------------------------------------
        let progress_pct = self
            .asr_progress
            .filter(|_| is_transcribing)
            .map(|(done, total)| (done as f32 / total.max(1) as f32 * 100.0) as u32);

        let accent = rgb(self.theme.accent);
        let panel_bg = ui.visuals().panel_fill;
        let primary = rgb(self.theme.fg_primary);
        let muted = rgb(self.theme.fg_muted);

        let mut toggle_play = false;
        let mut open_dock = false;

        // ---------------------------------------------------------
        // Full-width transcript ticker.
        // ---------------------------------------------------------
        let full_w = ui.available_width();
        let ultra_narrow = full_w < 150.0;

        egui::Frame::NONE
            .fill(panel_bg)
            .stroke(egui::Stroke::NONE)
            .corner_radius(egui::CornerRadius::ZERO)
            .inner_margin(egui::Margin::symmetric(7, 2))
            .show(ui, |ui| {
                ui.set_width((full_w - 14.0).max(40.0));

                ui.horizontal(|row| {
                    row.spacing_mut().item_spacing.x = 6.0;

                    // PLAY / PAUSE
                    if wav.is_some() {
                        let icon = if playing { "󰏤" } else { "󰐊" };

                        let play_button = egui::Button::new(
                            egui::RichText::new(icon)
                                .size(11.0)
                                .color(self.theme.on_accent()),
                        )
                        .fill(accent)
                        .stroke(egui::Stroke::NONE)
                        .corner_radius(egui::CornerRadius::same(5));

                        if row
                            .add_sized([20.0, 19.0], play_button)
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text("Play / Pause lesson")
                            .clicked()
                        {
                            toggle_play = true;
                        }
                    }

                    // Full-width text.
                    match (progress_pct, seg_opt.as_ref()) {
                        (Some(pct), _) => {
                            row.spinner();
                            let text_width = row.available_width().max(12.0);
                            let lbl = if ultra_narrow {
                                format!("{}%", pct)
                            } else {
                                format!("Transcribing {}%", pct)
                            };
                            row.add_sized(
                                [text_width, 19.0],
                                egui::Label::new(
                                    egui::RichText::new(lbl).size(10.0).strong().color(accent),
                                )
                                .truncate(),
                            );
                        }

                        (None, Some((start_ms, text))) => {
                            // Hide the timestamp in narrow panels.
                            if !ultra_narrow {
                                row.add_sized(
                                    [38.0, 19.0],
                                    egui::Label::new(
                                        egui::RichText::new(audio::format_ms(*start_ms))
                                            .monospace()
                                            .size(9.5)
                                            .color(accent),
                                    )
                                    .truncate(),
                                );
                            }

                            let text_width = row.available_width().max(12.0);
                            let text_response = row.add_sized(
                                [text_width, 19.0],
                                egui::Label::new(
                                    egui::RichText::new(text).size(10.2).color(primary),
                                )
                                .truncate()
                                .sense(egui::Sense::click()),
                            );

                            if text_response
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .on_hover_text("Open full transcript")
                                .clicked()
                            {
                                open_dock = true;
                            }
                        }

                        (None, None) if is_transcribing => {
                            row.spinner();
                            let text_width = row.available_width().max(12.0);
                            row.add_sized(
                                [text_width, 19.0],
                                egui::Label::new(
                                    egui::RichText::new("Preparing transcript...")
                                        .size(10.0)
                                        .color(accent),
                                )
                                .truncate(),
                            );
                        }

                        (None, None) => {
                            let text_width = row.available_width().max(12.0);
                            let resp = row.add_sized(
                                [text_width, 19.0],
                                egui::Label::new(
                                    egui::RichText::new("No transcript available")
                                        .size(10.0)
                                        .color(muted),
                                )
                                .truncate()
                                .sense(egui::Sense::click()),
                            );
                            if resp.clicked() {
                                open_dock = true;
                            }
                        }
                    }
                });
            });

        if toggle_play && let Some(wav_path) = wav {
            if self.audio_engine.total_duration_ms == 0 {
                let _ = self
                    .audio_engine
                    .load_and_play(&wav_path, "Lecture".to_string(), 0);
            } else {
                self.audio_engine.toggle_play_pause();
            }
        }

        if open_dock {
            self.audio_dock_open = true;
        }
    }

    pub(super) fn render_page_voice_memos_strip(&mut self, ui: &mut egui::Ui) {
        if !self.app_state.settings.audio_notes {
            return;
        }

        let media_dir = AudioDocIndex::media_dir_for_pdf(&self.pdf_path);
        let page = self.current_page;

        let Some(memos) = self.audio_index.page_memos.get(&page).cloned() else {
            return;
        };

        if memos.is_empty() {
            return;
        }

        let strip_w = ui.available_width();
        // Use wider thresholds to prevent overlap.
        let show_page_lbl = strip_w >= 235.0;
        let compact_title = strip_w < 185.0;
        let ultra_narrow = strip_w < 135.0;

        let accent = rgb(self.theme.accent);
        let muted = rgb(self.theme.fg_muted);
        let primary = rgb(self.theme.fg_primary);
        let error = rgb(self.theme.error);
        let elevated = rgb(self.theme.bg_elevated);
        let border = rgb(self.theme.border);

        let is_transcribing = self.is_transcribing;
        let is_recording = self.audio_engine.is_recording();

        let memo_is_recording = matches!(
            self.audio_engine.rec_target,
            Some(RecordingTarget::PageMemo(_))
        );

        let lesson_btn_info = self.lesson_button_state();

        let mut rename_text = self.rename_memo_text.clone();
        let active_rename_idx = self
            .rename_memo_target
            .as_ref()
            .and_then(|(target_page, idx)| {
                if *target_page == page && *idx < memos.len() {
                    Some(*idx)
                } else {
                    None
                }
            });

        // Azioni differite
        let mut memo_fab_clicked = false;
        let mut lesson_fab_clicked = false;

        let mut to_play: Option<(PathBuf, String)> = None;
        let mut to_delete: Option<usize> = None;
        let mut to_transcribe: Option<(usize, PathBuf)> = None;
        let mut to_trim: Option<(usize, u64)> = None;

        let mut to_start_rename: Option<(usize, String)> = None;
        let mut to_save_rename: Option<(usize, String)> = None;
        let mut cancel_rename = false;

        // ---------------------------------------------------------
        // Header with memo and lesson controls on the right.
        // ---------------------------------------------------------
        ui.horizontal(|ui| {
            ui.set_height(22.0);
            ui.spacing_mut().item_spacing.x = 4.0;

            ui.label(egui::RichText::new("󰍬").size(11.5).color(accent));

            if !ultra_narrow {
                let hdr_text = if compact_title {
                    "Memos"
                } else {
                    "Voice Memos"
                };
                ui.label(
                    egui::RichText::new(hdr_text)
                        .size(10.5)
                        .strong()
                        .color(primary),
                );
            }

            ui.label(
                egui::RichText::new(format!("{:02}", memos.len()))
                    .size(9.5)
                    .monospace()
                    .color(accent),
            );

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 5.0;

                // 1. Voice memo recording button.
                let fab_response = Self::audio_fab_button(
                    ui,
                    if memo_is_recording { "■" } else { "󰍬" },
                    if memo_is_recording { error } else { accent },
                    elevated,
                    border,
                    primary,
                    muted,
                    memo_is_recording,
                    memo_is_recording || !is_recording,
                    Self::AUDIO_BTN_SIZE,
                    if memo_is_recording {
                        "Stop Voice Memo"
                    } else {
                        "Record Voice Memo"
                    },
                    false,
                );
                memo_fab_clicked = fab_response.clicked();

                // 2. Lesson audio button.
                if let Some((l_glyph, l_accent, l_active, l_tooltip)) = lesson_btn_info {
                    let lesson_resp = Self::audio_fab_button(
                        ui,
                        l_glyph,
                        l_accent,
                        elevated,
                        border,
                        primary,
                        muted,
                        l_active,
                        true,
                        Self::AUDIO_BTN_SIZE,
                        l_tooltip,
                        self.audio_index.lecture_wav.is_some(),
                    );
                    lesson_fab_clicked = lesson_resp.clicked();
                }

                if show_page_lbl {
                    ui.label(
                        egui::RichText::new(format!("Page {}", page))
                            .size(8.5)
                            .color(muted),
                    );
                }
            });
        });

        // ---------------------------------------------------------
        // LISTA
        // ---------------------------------------------------------
        let list_height = (ui.available_height()).max(22.0);

        egui::ScrollArea::vertical()
            .id_salt(("voice_memos_list", page))
            .max_height(list_height)
            .auto_shrink([false, false])
            .show(ui, |list_ui| {
                list_ui.spacing_mut().item_spacing.y = 2.0;

                for (i, memo) in memos.iter().enumerate() {
                    let memo_filename = memo.filename.clone();
                    let memo_duration = memo.duration_ms;
                    let memo_transcript = memo.transcript.clone();

                    let display_name = memo
                        .title
                        .as_deref()
                        .filter(|title| !title.trim().is_empty())
                        .unwrap_or(&memo.id)
                        .to_owned();

                    let row_width = list_ui.available_width();
                    let show_duration = row_width >= 195.0;
                    let memo_path = media_dir.join(&memo_filename);
                    let memo_is_active =
                        self.memo_audio_engine.active_file.as_ref() == Some(&memo_path);
                    let mut memo_pos_ms = if memo_is_active {
                        self.memo_audio_engine.current_pos_ms().min(memo_duration)
                    } else {
                        0
                    };
                    let show_memo_player = memo_is_active && memo_pos_ms < memo_duration;

                    list_ui.allocate_ui_with_layout(
                        egui::vec2(row_width, 22.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |row| {
                            row.set_width(row_width);
                            row.spacing_mut().item_spacing.x = 4.0;

                            // -------------------------------------
                            // Independent memo playback.
                            // -------------------------------------
                            let memo_is_loaded = memo_is_active
                                && memo_pos_ms < self.memo_audio_engine.total_duration_ms;
                            let is_this_memo_playing =
                                memo_is_loaded && self.memo_audio_engine.is_playing();

                            let play_icon = if is_this_memo_playing { "󰏤" } else { "󰐊" };

                            let play_button = egui::Button::new(
                                egui::RichText::new(play_icon).size(10.5).color(primary),
                            )
                            .frame(false)
                            .corner_radius(egui::CornerRadius::same(10));

                            if row
                                .add_sized([20.0, 20.0], play_button)
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .on_hover_text(if is_this_memo_playing {
                                    "Pause Voice Memo"
                                } else {
                                    memo_transcript.as_deref().unwrap_or("Play Voice Memo")
                                })
                                .clicked()
                            {
                                if memo_is_loaded {
                                    self.memo_audio_engine.toggle_play_pause();
                                } else {
                                    to_play = Some((memo_path.clone(), display_name.clone()));
                                }
                            }

                            // -------------------------------------
                            // NOME O EDITOR DI RINOMINA
                            // -------------------------------------
                            let is_renaming = active_rename_idx == Some(i);
                            // Reserve space for the right-side controls.
                            let right_controls_w = if is_renaming {
                                76.0
                            } else if show_memo_player {
                                22.0
                            } else if show_duration {
                                78.0
                            } else {
                                32.0
                            };
                            let name_width = (row.available_width() - right_controls_w).max(18.0);

                            if is_renaming {
                                let edit_resp = row.add_sized(
                                    [name_width, 20.0],
                                    egui::TextEdit::singleline(&mut rename_text)
                                        .font(egui::TextStyle::Small)
                                        .margin(egui::vec2(3.0, 1.0)),
                                );

                                if edit_resp.lost_focus()
                                    && row.input(|inp| inp.key_pressed(egui::Key::Enter))
                                {
                                    to_save_rename = Some((i, rename_text.trim().to_owned()));
                                }

                                if row
                                    .add_sized(
                                        [18.0, 19.0],
                                        egui::Button::new(
                                            egui::RichText::new("✓").size(11.0).color(accent),
                                        )
                                        .frame(false),
                                    )
                                    .clicked()
                                {
                                    to_save_rename = Some((i, rename_text.trim().to_owned()));
                                }

                                if row
                                    .add_sized(
                                        [18.0, 19.0],
                                        egui::Button::new(
                                            egui::RichText::new("×").size(12.0).color(muted),
                                        )
                                        .frame(false),
                                    )
                                    .clicked()
                                {
                                    cancel_rename = true;
                                }
                            } else if show_memo_player {
                                let title_width = 42.0_f32.min(name_width);
                                row.add_sized(
                                    [title_width, 18.0],
                                    egui::Label::new(
                                        egui::RichText::new(&display_name).size(9.0).color(primary),
                                    )
                                    .truncate(),
                                )
                                .on_hover_text(&display_name);

                                let time_width = 39.0;
                                let slider_width =
                                    (row.available_width() - right_controls_w - time_width - 4.0)
                                        .max(18.0);
                                let seek = row.add_sized(
                                    [slider_width, 12.0],
                                    egui::Slider::new(&mut memo_pos_ms, 0..=memo_duration.max(1))
                                        .show_value(false)
                                        .trailing_fill(true),
                                );
                                if seek.changed() {
                                    self.memo_audio_engine.seek_to_ms(memo_pos_ms);
                                }
                                row.add_sized(
                                    [time_width, 16.0],
                                    egui::Label::new(
                                        egui::RichText::new(format!(
                                            "{}/{}s",
                                            memo_pos_ms / 1000,
                                            memo_duration / 1000
                                        ))
                                        .monospace()
                                        .size(8.0)
                                        .color(muted),
                                    ),
                                );
                            } else {
                                row.add_sized(
                                    [name_width, 20.0],
                                    egui::Label::new(
                                        egui::RichText::new(&display_name)
                                            .size(10.0)
                                            .color(primary),
                                    )
                                    .truncate(),
                                )
                                .on_hover_text(&display_name);
                            }

                            // -------------------------------------
                            // Duration and menu.
                            // -------------------------------------
                            row.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |right| {
                                    Self::glass_menu_button(
                                        right,
                                        self.menu_glass(),
                                        "⋯",
                                        |menu| {
                                            menu.set_min_width(150.0);

                                            if menu.button("󰑕 Rename").clicked() {
                                                to_start_rename = Some((
                                                    i,
                                                    memo.title
                                                        .clone()
                                                        .unwrap_or_else(|| memo.id.clone()),
                                                ));
                                                menu.close();
                                            }

                                            if menu
                                                .add_enabled(
                                                    !is_transcribing,
                                                    egui::Button::new("󰗊 Transcribe"),
                                                )
                                                .clicked()
                                            {
                                                to_transcribe =
                                                    Some((i, media_dir.join(&memo_filename)));
                                                menu.close();
                                            }

                                            if menu.button("󰆐 Trim").clicked() {
                                                to_trim = Some((i, memo_duration));
                                                menu.close();
                                            }

                                            if menu
                                                .add(egui::Button::new(
                                                    egui::RichText::new("󰆴 Delete").color(error),
                                                ))
                                                .clicked()
                                            {
                                                to_delete = Some(i);
                                                menu.close();
                                            }
                                        },
                                    );

                                    if show_duration && !is_renaming && !show_memo_player {
                                        right.label(
                                            egui::RichText::new(audio::format_ms(memo_duration))
                                                .monospace()
                                                .size(9.0)
                                                .color(muted),
                                        );
                                    }
                                },
                            );
                        },
                    );
                }
            });

        // ---------------------------------------------------------
        // Apply actions after the UI closures.
        // ---------------------------------------------------------
        if memo_fab_clicked {
            if memo_is_recording {
                self.finish_memo_recording(&media_dir);
            } else if !is_recording {
                self.start_memo_recording(&media_dir, page);
            }
        }

        if lesson_fab_clicked {
            self.audio_dock_open = !self.audio_dock_open;
        }

        if let Some((idx, initial_text)) = to_start_rename {
            self.rename_memo_target = Some((page, idx));
            self.rename_memo_text = initial_text;
        }

        if cancel_rename {
            self.rename_memo_target = None;
            self.rename_memo_text.clear();
        }

        if let Some((idx, title)) = to_save_rename {
            if let Some(memo) = self
                .audio_index
                .page_memos
                .get_mut(&page)
                .and_then(|list| list.get_mut(idx))
            {
                let title = title.trim();
                memo.title = if title.is_empty() {
                    None
                } else {
                    Some(title.to_owned())
                };
                self.audio_index.save(&self.pdf_path);
            }
            self.rename_memo_target = None;
            self.rename_memo_text.clear();
        } else if active_rename_idx.is_some() {
            self.rename_memo_text = rename_text;
        }

        if let Some((path, label)) = to_play {
            let _ = self.memo_audio_engine.load_and_play(&path, label, 0);
        }

        if let Some((idx, path)) = to_transcribe {
            self.start_memo_transcription(page, idx, path, ui.ctx());
        }

        if let Some((idx, duration)) = to_trim {
            self.show_audio_trimmer = true;
            self.audio_dock_open = true;
            self.trim_start_ms = 0;
            self.trim_end_ms = duration.max(1000);
            self.trim_memo_target = Some((page, idx));
        }

        if let Some(idx) = to_delete {
            match self.delete_memo_recording(&media_dir, page, idx) {
                Ok(()) => {
                    if self
                        .rename_memo_target
                        .is_some_and(|(p, target)| p == page && target >= idx)
                    {
                        self.rename_memo_target = None;
                        self.rename_memo_text.clear();
                    }
                    if self
                        .trim_memo_target
                        .is_some_and(|(p, target)| p == page && target >= idx)
                    {
                        self.trim_memo_target = None;
                        self.show_audio_trimmer = false;
                    }
                }
                Err(err) => self.audio_toast(format!("Could not remove voice memo: {err}")),
            }
        }
    }

    pub(super) fn delete_memo_recording(
        &mut self,
        media_dir: &Path,
        page: u32,
        memo: usize,
    ) -> Result<(), String> {
        let mut index = self.audio_index.clone();
        let list = index
            .page_memos
            .get_mut(&page)
            .filter(|list| memo < list.len())
            .ok_or("The voice memo is no longer available")?;
        let removed = list.remove(memo);
        if list.is_empty() {
            index.page_memos.remove(&page);
        }
        if self.memo_audio_engine.active_file.as_ref() == Some(&media_dir.join(&removed.filename)) {
            self.memo_audio_engine.stop_and_release();
        }
        self.publish_audio_index(index, media_dir, Some(&removed.filename))
    }

    fn render_audio_body(&mut self, ui: &mut egui::Ui, media_dir: &Path) {
        egui::ScrollArea::vertical()
            .id_salt("audio_dock_scroll_area")
            .auto_shrink([false, false])
            .max_height(ui.available_height())
            .show(ui, |ui| {
                let body_w = ui.available_width();
                let compact_controls = body_w < 265.0;
                let ultra_narrow = body_w < 175.0;

                ui.spacing_mut().item_spacing = if compact_controls {
                    egui::vec2(4.0, 6.0)
                } else {
                    egui::vec2(8.0, 9.0)
                };
                ui.spacing_mut().button_padding = if compact_controls {
                    egui::vec2(6.0, 4.0)
                } else {
                    egui::vec2(9.0, 5.0)
                };

                let wav_path = self
                    .audio_index
                    .lecture_wav
                    .as_ref()
                    .map(|n| media_dir.join(n));
                let is_rec = self.audio_engine.rec_target == Some(RecordingTarget::Lecture);
                let busy = self.is_transcribing;

                if self.audio_index.lecture_wav.is_some()
                    || !self.audio_index.other_lectures.is_empty()
                {
                    self.render_lecture_picker(ui);
                }

                if let Some(wav_path) = wav_path {
                    let is_playing = self.audio_engine.is_playing();
                    let mut pos_ms = self.audio_engine.current_pos_ms();
                    let total_ms = self
                        .audio_engine
                        .total_duration_ms
                        .max(self.audio_index.lecture_duration_ms)
                        .max(1);

                    if self.audio_index.waveform_peaks.is_empty() && wav_path.exists() {
                        self.audio_index.waveform_peaks =
                            audio::compute_waveform_peaks(&wav_path, 200);
                        self.audio_index.save(&self.pdf_path);
                    }

                    // 1. Player controls and waveform.
                    ui.horizontal(|ui| {
                        let play_icon = if is_playing { "󰏤" } else { "󰐊" };
                        let play_btn = egui::Button::new(
                            egui::RichText::new(play_icon)
                                .size(16.0)
                                .color(self.theme.on_accent()),
                        )
                        .fill(rgb(self.theme.accent))
                        .stroke(egui::Stroke::NONE)
                        .corner_radius(egui::CornerRadius::same(8));

                        if ui
                            .add_sized([30.0, 30.0], play_btn)
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text("Play / Pause")
                            .clicked()
                        {
                            if self.audio_engine.total_duration_ms == 0 {
                                let _ = self.audio_engine.load_and_play(
                                    &wav_path,
                                    "Lecture".to_string(),
                                    0,
                                );
                            } else {
                                self.audio_engine.toggle_play_pause();
                            }
                        }

                        let wave_h = 30.0;
                        let (wave_rect, wave_resp) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width().max(8.0), wave_h),
                            egui::Sense::click_and_drag(),
                        );
                        let wave_resp = wave_resp
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text(format!(
                                "{} / {}",
                                audio::format_ms(pos_ms),
                                audio::format_ms(total_ms)
                            ));
                        let painter = ui.painter_at(wave_rect);

                        // Waveform background.
                        painter.rect_filled(
                            wave_rect,
                            egui::CornerRadius::same(8),
                            rgb(self.theme.bg_canvas),
                        );
                        painter.rect_stroke(
                            wave_rect,
                            egui::CornerRadius::same(8),
                            egui::Stroke::new(1.0, rgb(self.theme.border).gamma_multiply(0.6)),
                            egui::StrokeKind::Inside,
                        );

                        let frac = (pos_ms as f32 / total_ms as f32).clamp(0.0, 1.0);
                        let peaks = &self.audio_index.waveform_peaks;
                        let n = peaks.len().max(1);
                        let pad_x = 6.0;
                        let usable_w = (wave_rect.width() - pad_x * 2.0).max(10.0);
                        let step_x = usable_w / n as f32;

                        for (idx, &pk) in peaks.iter().enumerate() {
                            let x = wave_rect.left() + pad_x + (idx as f32 + 0.5) * step_x;
                            let half_h = (pk * (wave_h * 0.40)).max(1.5);
                            let passed = (idx as f32 / n as f32) <= frac;
                            let col = if passed {
                                rgb(self.theme.accent)
                            } else {
                                rgb(self.theme.fg_muted).gamma_multiply(0.35)
                            };
                            painter.line_segment(
                                [
                                    egui::pos2(x, wave_rect.center().y - half_h),
                                    egui::pos2(x, wave_rect.center().y + half_h),
                                ],
                                egui::Stroke::new((step_x * 0.55).clamp(1.2, 2.6), col),
                            );
                        }

                        // Slide markers.
                        for &(ts, _) in &self.audio_index.timeline {
                            let tx = wave_rect.left()
                                + pad_x
                                + (ts as f32 / total_ms as f32).clamp(0.0, 1.0) * usable_w;
                            painter.line_segment(
                                [
                                    egui::pos2(tx, wave_rect.bottom() - 6.0),
                                    egui::pos2(tx, wave_rect.bottom() - 1.0),
                                ],
                                egui::Stroke::new(2.0, rgb(self.theme.warning)),
                            );
                        }

                        // Playhead nitido
                        let head_x = wave_rect.left() + pad_x + frac * usable_w;
                        painter.line_segment(
                            [
                                egui::pos2(head_x, wave_rect.top() + 2.0),
                                egui::pos2(head_x, wave_rect.bottom() - 2.0),
                            ],
                            egui::Stroke::new(1.5, egui::Color32::WHITE),
                        );
                        painter.circle_filled(
                            egui::pos2(head_x, wave_rect.top() + 4.0),
                            3.0,
                            egui::Color32::WHITE,
                        );

                        // Hover scrub preview line
                        if let Some(hover_pos) = wave_resp.hover_pos() {
                            let h_x = hover_pos
                                .x
                                .clamp(wave_rect.left() + pad_x, wave_rect.right() - pad_x);
                            painter.line_segment(
                                [
                                    egui::pos2(h_x, wave_rect.top() + 2.0),
                                    egui::pos2(h_x, wave_rect.bottom() - 2.0),
                                ],
                                egui::Stroke::new(1.0, egui::Color32::from_white_alpha(70)),
                            );
                        }

                        // Responsive timestamp badge.
                        if wave_rect.width() > 68.0 {
                            let time_str = if wave_rect.width() > 125.0 {
                                format!(
                                    "{} / {}",
                                    audio::format_ms(pos_ms),
                                    audio::format_ms(total_ms)
                                )
                            } else {
                                audio::format_ms(pos_ms)
                            };
                            let time_galley = painter.layout_no_wrap(
                                time_str,
                                egui::FontId::monospace(9.5),
                                egui::Color32::from_gray(235),
                            );
                            let badge_size = time_galley.size() + egui::vec2(8.0, 4.0);
                            let badge_rect = egui::Rect::from_min_size(
                                egui::pos2(
                                    wave_rect.right() - badge_size.x - 5.0,
                                    wave_rect.top() + 4.0,
                                ),
                                badge_size,
                            );
                            painter.rect_filled(
                                badge_rect,
                                egui::CornerRadius::same(5),
                                egui::Color32::from_black_alpha(175),
                            );
                            painter.galley(
                                badge_rect.min + egui::vec2(4.0, 2.0),
                                time_galley,
                                egui::Color32::WHITE,
                            );
                        }

                        // Keep seeking enabled at narrow widths.
                        if (wave_resp.clicked() || wave_resp.dragged())
                            && let Some(ptr) = wave_resp.interact_pointer_pos()
                        {
                            let new_frac =
                                ((ptr.x - (wave_rect.left() + pad_x)) / usable_w).clamp(0.0, 1.0);
                            pos_ms = (new_frac * total_ms as f32) as u64;
                            self.audio_engine.seek_to_ms(pos_ms);
                        }
                    });

                    // 2. Action Pill Bar
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = if compact_controls { 4.0 } else { 5.0 };

                        const SPEEDS: [f32; 4] = [1.0, 1.25, 1.5, 2.0];
                        let cur = self.audio_engine.speed;
                        let speed_txt = format!("{:.2}x", cur)
                            .trim_end_matches('0')
                            .trim_end_matches('.')
                            .to_string();
                        let speed_btn = egui::Button::new(
                            egui::RichText::new(speed_txt)
                                .monospace()
                                .size(11.0)
                                .strong(),
                        )
                        .corner_radius(egui::CornerRadius::same(6));

                        if ui.add(speed_btn).on_hover_text("Playback speed").clicked() {
                            let next = SPEEDS
                                .iter()
                                .position(|s| (s - cur).abs() < 0.05)
                                .map(|i| SPEEDS[(i + 1) % SPEEDS.len()])
                                .unwrap_or(1.0);
                            self.audio_engine.set_speed(next);
                        }

                        ui.toggle_value(
                            &mut self.audio_engine.follow_pdf,
                            if compact_controls {
                                "󰁔"
                            } else {
                                "󰁔 Sync"
                            },
                        )
                        .on_hover_text("Automatically synchronize slides with audio");

                        if let Some(slide_ts) = self.audio_index.time_for_page(self.current_page) {
                            let jmp_txt = if ultra_narrow {
                                "󰁨".to_string()
                            } else {
                                format!("󰁨 {}", audio::format_ms(slide_ts))
                            };
                            let jmp_btn = egui::Button::new(
                                egui::RichText::new(jmp_txt)
                                    .size(11.5)
                                    .color(rgb(self.theme.accent)),
                            )
                            .fill(rgb(self.theme.accent).gamma_multiply(0.14))
                            .corner_radius(egui::CornerRadius::same(6));

                            if ui
                                .add(jmp_btn)
                                .on_hover_text(format!(
                                    "Jump to slide timestamp ({})",
                                    audio::format_ms(slide_ts)
                                ))
                                .clicked()
                            {
                                self.audio_engine.seek_to_ms(slide_ts);
                            }
                        } else {
                            let pin_txt = if compact_controls {
                                "󰃀"
                            } else {
                                "󰃀 Pin Slide"
                            };
                            if ui
                                .add(
                                    egui::Button::new(egui::RichText::new(pin_txt).size(11.5))
                                        .corner_radius(egui::CornerRadius::same(6)),
                                )
                                .on_hover_text(
                                    "Associate the current minute of the audio with this slide",
                                )
                                .clicked()
                            {
                                self.audio_index.timeline.push((pos_ms, self.current_page));
                                self.audio_index.timeline.sort_unstable_by_key(|k| k.0);
                                self.audio_index.timeline_is_live = true;
                                self.audio_index.save(&self.pdf_path);
                            }
                        }

                        if !ultra_narrow {
                            ui.separator();
                        }

                        let trim_open = self.show_audio_trimmer && self.trim_memo_target.is_none();
                        if ui
                            .add_enabled(
                                !busy,
                                crate::theme::selected_button("󰆐", trim_open, ui.visuals())
                                    .corner_radius(egui::CornerRadius::same(6)),
                            )
                            .on_hover_text("Trim lesson start/end")
                            .clicked()
                        {
                            self.show_audio_trimmer = !trim_open;
                            self.trim_start_ms = 0;
                            self.trim_end_ms = total_ms;
                            self.trim_memo_target = None;
                        }

                        if ui
                            .add_enabled(
                                !busy && !self.audio_engine.is_recording(),
                                egui::Button::new("󰖁").corner_radius(egui::CornerRadius::same(6)),
                            )
                            .on_hover_text("Remove silences and adjust timestamps")
                            .clicked()
                        {
                            self.remove_lecture_silence(media_dir, &wav_path);
                        }

                        if ui
                            .add_enabled(
                                !busy && !self.audio_engine.is_recording(),
                                egui::Button::new(if self.confirm_remove_lecture {
                                    "󰆴 Confirm"
                                } else {
                                    "󰆴"
                                })
                                .corner_radius(egui::CornerRadius::same(6)),
                            )
                            .on_hover_text(if self.confirm_remove_lecture {
                                "Confirm removal of the lesson audio and its transcript"
                            } else {
                                "Remove lesson audio and its transcript"
                            })
                            .clicked()
                        {
                            if self.confirm_remove_lecture {
                                let mut index = self.audio_index.clone();
                                let old = index.lecture_wav.clone();
                                index.remove_active_lecture();
                                self.audio_engine.stop_and_release();
                                match self.publish_audio_index(index, media_dir, old.as_deref()) {
                                    Ok(()) => {
                                        self.show_audio_trimmer = false;
                                        self.trim_memo_target = None;
                                        self.trim_start_ms = 0;
                                        self.trim_end_ms = 0;
                                        self.transcript_last_seg = None;
                                        self.transcript_all = false;
                                        self.audio_toast("Lesson audio removed");
                                    }
                                    Err(err) => self.audio_toast(format!(
                                        "Could not remove lesson audio: {err}"
                                    )),
                                }
                                self.confirm_remove_lecture = false;
                            } else {
                                self.confirm_remove_lecture = true;
                            }
                        }

                        let can_rec = is_rec || (!self.audio_engine.is_recording() && !busy);
                        let rec_lbl = if is_rec {
                            let txt = if ultra_narrow {
                                "󰓛".to_string()
                            } else {
                                format!(
                                    "󰓛 {}",
                                    audio::format_ms(self.audio_engine.recording_elapsed_ms())
                                )
                            };
                            egui::RichText::new(txt)
                                .color(rgb(self.theme.error))
                                .strong()
                        } else {
                            egui::RichText::new("󰍬")
                        };
                        if ui
                            .add_enabled(
                                can_rec,
                                egui::Button::new(rec_lbl)
                                    .corner_radius(egui::CornerRadius::same(6)),
                            )
                            .on_hover_text(if is_rec {
                                "Stop and save recording"
                            } else {
                                "Record a new lesson from the microphone"
                            })
                            .clicked()
                        {
                            self.toggle_lecture_recording(media_dir);
                        }

                        if ui
                            .add_enabled(
                                !self.audio_engine.is_recording() && !busy,
                                egui::Button::new("󰈦").corner_radius(egui::CornerRadius::same(6)),
                            )
                            .on_hover_text("Replace with audio/video file (mp4, mkv, mp3, wav...)")
                            .clicked()
                        {
                            self.import_lecture_media(media_dir, ui.ctx());
                        }

                        if !busy {
                            let tx_label = if compact_controls {
                                "󰗊"
                            } else {
                                "󰗊 Transcribe"
                            };

                            let tx_btn = egui::Button::new(
                                egui::RichText::new(tx_label)
                                    .size(11.0)
                                    .strong()
                                    .color(rgb(self.theme.accent)),
                            )
                            .fill(rgb(self.theme.accent).gamma_multiply(0.15))
                            .stroke(egui::Stroke::new(
                                1.0,
                                rgb(self.theme.accent).gamma_multiply(0.35),
                            ))
                            .corner_radius(egui::CornerRadius::same(6));

                            if ui
                                .add_enabled(!self.audio_engine.is_recording(), tx_btn)
                                .on_hover_text(
                                    "Transcribe the audio lesson and align it with the slides",
                                )
                                .clicked()
                            {
                                self.start_lecture_transcription(wav_path.clone(), ui.ctx());
                            }
                            let has_tx = !self.audio_index.segments.is_empty();
                            let al_label = if compact_controls {
                                "󰌷"
                            } else {
                                "󰌷 Align"
                            };
                            if ui
                                .add_enabled(
                                    has_tx && !self.audio_engine.is_recording(),
                                    egui::Button::new(al_label)
                                        .corner_radius(egui::CornerRadius::same(6)),
                                )
                                .on_hover_text("Map the transcript to a range of slides")
                                .clicked()
                            {
                                let (a, b) = self
                                    .audio_index
                                    .page_range
                                    .unwrap_or((1, self.total_pages.max(1)));
                                self.align_from = a;
                                self.align_to = b;
                                self.show_align_panel = !self.show_align_panel;
                            }
                        }
                    });

                    if busy {
                        egui::Frame::NONE
                            .fill(rgb(self.theme.bg_canvas))
                            .corner_radius(egui::CornerRadius::same(6))
                            .inner_margin(egui::Margin::symmetric(8, 5))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    let bar_w = (ui.available_width() - 32.0).max(24.0);
                                    if let Some((done, total)) = self.asr_progress {
                                        ui.add(
                                            egui::ProgressBar::new(
                                                done as f32 / total.max(1) as f32,
                                            )
                                            .desired_width(bar_w)
                                            .show_percentage(),
                                        );
                                    } else {
                                        let w = (ui.available_width() - 28.0).max(20.0);
                                        ui.add_sized(
                                            [w, 18.0],
                                            egui::Label::new(
                                                egui::RichText::new("Transcribing audio...")
                                                    .size(11.5),
                                            )
                                            .truncate(),
                                        );
                                    }
                                    if ui
                                        .small_button("󰅖")
                                        .on_hover_text("Cancel (retains partial transcription)")
                                        .clicked()
                                    {
                                        self.asr_cancel
                                            .store(true, std::sync::atomic::Ordering::Relaxed);
                                    }
                                });
                            });
                    }
                } else {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;

                        let compact = ui.available_width() < 235.0;

                        if is_rec {
                            let elapsed =
                                audio::format_ms(self.audio_engine.recording_elapsed_ms());

                            if ui.button(format!("■ {}", elapsed)).clicked() {
                                self.toggle_lecture_recording(media_dir);
                            }
                        } else {
                            let record = if compact {
                                "󰍬"
                            } else {
                                "󰍬 Record Lesson"
                            };

                            if ui
                                .add_enabled(
                                    !self.audio_engine.is_recording(),
                                    egui::Button::new(record),
                                )
                                .on_hover_text("Record a new lesson")
                                .clicked()
                            {
                                self.toggle_lecture_recording(media_dir);
                            }

                            let import = if compact {
                                "󰈦"
                            } else {
                                "󰈦 Import Audio / Video"
                            };

                            if ui
                                .add_enabled(
                                    !self.audio_engine.is_recording() && !busy,
                                    egui::Button::new(import),
                                )
                                .on_hover_text("Import audio or video")
                                .clicked()
                            {
                                self.import_lecture_media(media_dir, ui.ctx());
                            }
                        }
                    });
                }

                if self.show_audio_trimmer {
                    self.render_audio_trimmer(ui, media_dir);
                }
                if self.show_align_panel {
                    self.render_align_panel(ui);
                }

                // 3. Transcript section with interactive cards.
                if !self.audio_index.segments.is_empty() {
                    let avail_tx_w = ui.available_width();
                    let compact_tx = avail_tx_w < 245.0;
                    let tiny_tx = avail_tx_w < 145.0;

                    let title = if tiny_tx {
                        if self.transcript_all {
                            "󰗊".to_string()
                        } else {
                            format!("P.{}", self.current_page)
                        }
                    } else if compact_tx {
                        if self.transcript_all {
                            "󰗊 ALL".to_string()
                        } else {
                            format!("󰗊 P.{}", self.current_page)
                        }
                    } else if self.transcript_all {
                        "󰗊 Full Transcript".to_string()
                    } else {
                        format!("󰗊 Slide {} Transcript", self.current_page)
                    };

                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 5.0;

                        ui.label(
                            egui::RichText::new(title)
                                .strong()
                                .size(11.5)
                                .color(rgb(self.theme.fg_primary)),
                        );

                        let note_label = if compact_tx { "󰈙" } else { "󰈙 +Note" };

                        let note_btn = egui::Button::new(
                            egui::RichText::new(note_label)
                                .size(10.5)
                                .color(rgb(self.theme.accent)),
                        )
                        .fill(rgb(self.theme.accent).gamma_multiply(0.14))
                        .stroke(egui::Stroke::NONE)
                        .corner_radius(egui::CornerRadius::same(5));

                        if ui
                            .add(note_btn)
                            .on_hover_text("Append transcript to Markdown notes")
                            .clicked()
                        {
                            let lines: Vec<String> = self
                                .audio_index
                                .segments
                                .iter()
                                .filter(|s| s.page == self.current_page)
                                .map(|s| {
                                    format!("- **[{}]**: {}", audio::format_ms(s.start_ms), s.text)
                                })
                                .collect();
                            if !lines.is_empty() {
                                if !self.current_note.is_empty() {
                                    self.current_note.push_str("\n\n");
                                }
                                self.current_note.push_str("### 󰗊 Lecture Transcript\n");
                                self.current_note.push_str(&lines.join("\n"));
                                self.note_is_dirty = true;
                                self.sync_and_save();
                                self.audio_toast("󰄬 Transcription added to the notes");
                            }
                        }

                        let all_label = if compact_tx { "All" } else { "All Slides" };
                        ui.toggle_value(&mut self.transcript_all, all_label)
                            .on_hover_text("Show transcript for all slides");
                    });

                    let active = if self.audio_engine.total_duration_ms > 0 {
                        let pos = self.audio_engine.current_pos_ms();
                        self.audio_index.segment_index_at(pos)
                    } else {
                        None
                    };
                    let scroll_to_active = active != self.transcript_last_seg;
                    self.transcript_last_seg = active;

                    let rows: Vec<(usize, u64, String)> = self
                        .audio_index
                        .segments
                        .iter()
                        .enumerate()
                        .filter(|(_, s)| self.transcript_all || s.page == self.current_page)
                        .map(|(i, s)| (i, s.start_ms, s.text.clone()))
                        .collect();

                    egui::Frame::NONE
                        .fill(rgb(self.theme.bg_canvas))
                        .corner_radius(egui::CornerRadius::same(8))
                        .inner_margin(egui::Margin::symmetric(6, 6))
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt("audio_transcript_rows")
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.spacing_mut().item_spacing.y = 3.0;
                                    if rows.is_empty() {
                                        ui.horizontal_wrapped(|ui| {
                                            ui.weak(
                                            "No transcript segments associated with this slide.",
                                        );
                                            if ui.small_button("Show all slides").clicked() {
                                                self.transcript_all = true;
                                            }
                                        });
                                    } else {
                                        let narrow_rows = ui.available_width() < 155.0;

                                        for (i, start_ms, text) in rows {
                                            let is_active = Some(i) == active;
                                            let row_bg = if is_active {
                                                rgb(self.theme.accent).gamma_multiply(0.16)
                                            } else {
                                                egui::Color32::TRANSPARENT
                                            };

                                            let frame_resp = egui::Frame::NONE
                                                .fill(row_bg)
                                                .corner_radius(egui::CornerRadius::same(6))
                                                .inner_margin(egui::Margin::symmetric(8, 5))
                                                .show(ui, |ui| {
                                                    ui.set_width(ui.available_width());

                                                    let ts_col = if is_active {
                                                        rgb(self.theme.accent)
                                                    } else {
                                                        rgb(self.theme.fg_muted)
                                                    };
                                                    let txt_col = if is_active {
                                                        rgb(self.theme.fg_primary)
                                                    } else {
                                                        rgb(self.theme.fg_primary)
                                                            .gamma_multiply(0.88)
                                                    };
                                                    let rt = egui::RichText::new(text)
                                                        .size(11.8)
                                                        .color(txt_col);

                                                    // In narrow panels, place the timestamp
                                                    // above the text to give it the full row.
                                                    if narrow_rows {
                                                        ui.vertical(|ui| {
                                                            ui.spacing_mut().item_spacing.y = 2.0;
                                                            ui.label(
                                                                egui::RichText::new(
                                                                    audio::format_ms(start_ms),
                                                                )
                                                                .monospace()
                                                                .size(9.5)
                                                                .color(ts_col),
                                                            );
                                                            ui.add(
                                                                egui::Label::new(if is_active {
                                                                    rt.strong()
                                                                } else {
                                                                    rt
                                                                })
                                                                .wrap(),
                                                            );
                                                        });
                                                    } else {
                                                        ui.horizontal(|ui| {
                                                            ui.spacing_mut().item_spacing.x = 8.0;
                                                            ui.label(
                                                                egui::RichText::new(
                                                                    audio::format_ms(start_ms),
                                                                )
                                                                .monospace()
                                                                .size(10.5)
                                                                .color(ts_col),
                                                            );
                                                            ui.add(
                                                                egui::Label::new(if is_active {
                                                                    rt.strong()
                                                                } else {
                                                                    rt
                                                                })
                                                                .wrap(),
                                                            );
                                                        });
                                                    }
                                                });

                                            let row_rect = frame_resp.response.rect;
                                            if is_active {
                                                let bar = egui::Rect::from_min_size(
                                                    egui::pos2(
                                                        row_rect.left() + 1.5,
                                                        row_rect.top() + 4.0,
                                                    ),
                                                    egui::vec2(
                                                        2.5,
                                                        (row_rect.height() - 8.0).max(4.0),
                                                    ),
                                                );
                                                ui.painter().rect_filled(
                                                    bar,
                                                    egui::CornerRadius::same(1),
                                                    rgb(self.theme.accent),
                                                );
                                            }

                                            let interact = ui
                                                .interact(
                                                    row_rect,
                                                    ui.make_persistent_id(("tx_row", i)),
                                                    egui::Sense::click(),
                                                )
                                                .on_hover_cursor(egui::CursorIcon::PointingHand);

                                            if interact.hovered() && !is_active {
                                                ui.painter().rect_filled(
                                                    row_rect,
                                                    egui::CornerRadius::same(6),
                                                    egui::Color32::from_white_alpha(8),
                                                );
                                            }

                                            if is_active && scroll_to_active {
                                                interact.scroll_to_me(Some(egui::Align::Center));
                                            }
                                            if interact.clicked() {
                                                self.audio_engine.seek_to_ms(start_ms);
                                            }
                                        }
                                    }
                                });
                        });
                }
            });
    }

    fn render_audio_handle(&mut self, ui: &mut egui::Ui) {
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), 5.0),
            egui::Sense::click_and_drag(),
        );

        let hovered = response.hovered();
        let dragging = response.dragged();

        ui.painter().rect_filled(
            egui::Rect::from_center_size(
                rect.center(),
                egui::vec2(if hovered || dragging { 38.0 } else { 26.0 }, 4.0),
            ),
            egui::CornerRadius::same(2),
            if hovered || dragging {
                rgb(self.theme.accent)
            } else {
                rgb(self.theme.fg_muted).gamma_multiply(0.45)
            },
        );

        if dragging {
            let delta = response.drag_delta().y;

            self.audio_dock_height = (self.audio_dock_height - delta).clamp(100.0, 600.0);
        }

        response
            .on_hover_cursor(egui::CursorIcon::ResizeVertical)
            .on_hover_text("Drag to resize Lesson Audio");
    }

    /// Return the lesson audio button state and icon.
    fn lesson_button_state(&self) -> Option<(&'static str, egui::Color32, bool, &'static str)> {
        // Keep the button visible when the audio dock is enabled,
        // even when no lesson audio is loaded.
        if !self.app_state.settings.audio_dock {
            return None;
        }

        let is_rec = self.audio_engine.rec_target == Some(RecordingTarget::Lecture);
        let playing = self.audio_engine.is_playing();
        let has_wav = self.audio_index.lecture_wav.is_some();

        let accent = if is_rec {
            rgb(self.theme.error)
        } else {
            rgb(self.theme.accent)
        };

        // Show a suitable icon when no audio or transcript is loaded.
        // The button still opens the lesson audio dock.
        let glyph = if is_rec {
            "●"
        } else if playing && has_wav {
            "󰏤"
        } else if has_wav {
            "󰕾"
        } else {
            "󰋋"
        };

        let active = self.audio_dock_open || is_rec || playing;
        let tooltip = if self.audio_dock_open {
            "Close Lesson Audio"
        } else {
            "Open Lesson Audio"
        };

        Some((glyph, accent, active, tooltip))
    }

    pub(super) fn render_floating_audio_fabs(
        &mut self,
        ctx: &egui::Context,
        bottom_rect: egui::Rect,
        media_dir: &Path,
    ) {
        let page = self.current_page;

        let show_memo = self.app_state.settings.audio_notes && self.show_memo_fab;
        // Keep the lesson audio button visible beside the ticker.
        // Hide it only when the full dock is open.
        let lesson_info = self.lesson_button_state();

        if !show_memo && lesson_info.is_none() {
            return;
        }

        let is_rec_memo = matches!(
            self.audio_engine.rec_target,
            Some(RecordingTarget::PageMemo(_))
        );

        // Anchor the controls 6 px above `bottom_rect.top()`.
        // Place it above the ticker when visible.
        // Otherwise place it above the panel bottom.
        let anchor = egui::pos2(bottom_rect.right() - 8.0, bottom_rect.top() - 6.0);

        let mut memo_clicked = false;
        let mut lesson_clicked = false;

        egui::Area::new(egui::Id::unique("floating_audio_fabs_cluster"))
            // Keep the floating controls above panels and notes.
            .order(egui::Order::Tooltip)
            .pivot(egui::Align2::RIGHT_BOTTOM)
            .fixed_pos(anchor)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;

                    // 1. Lesson audio button.
                    if let Some((l_glyph, l_accent, l_active, l_tooltip)) = lesson_info
                        && Self::audio_fab_button(
                            ui,
                            l_glyph,
                            l_accent,
                            rgb(self.theme.bg_elevated),
                            rgb(self.theme.border),
                            rgb(self.theme.fg_primary),
                            rgb(self.theme.fg_muted),
                            l_active,
                            true,
                            Self::AUDIO_BTN_SIZE,
                            l_tooltip,
                            self.audio_index.lecture_wav.is_some(),
                        )
                        .clicked()
                    {
                        lesson_clicked = true;
                    }

                    // 2. Voice memo recording button.
                    if show_memo {
                        let accent = if is_rec_memo {
                            rgb(self.theme.error)
                        } else {
                            rgb(self.theme.accent)
                        };

                        if Self::audio_fab_button(
                            ui,
                            if is_rec_memo { "■" } else { "󰍬" },
                            accent,
                            rgb(self.theme.bg_elevated),
                            rgb(self.theme.border),
                            rgb(self.theme.fg_primary),
                            rgb(self.theme.fg_muted),
                            is_rec_memo,
                            is_rec_memo || !self.audio_engine.is_recording(),
                            Self::AUDIO_BTN_SIZE,
                            if is_rec_memo {
                                "Stop Voice Memo"
                            } else {
                                "Record Voice Memo"
                            },
                            false,
                        )
                        .clicked()
                        {
                            memo_clicked = true;
                        }
                    }
                });
            });

        if lesson_clicked {
            self.audio_dock_open = !self.audio_dock_open;
        }

        if memo_clicked {
            if is_rec_memo {
                self.finish_memo_recording(media_dir);
            } else if !self.audio_engine.is_recording() {
                self.start_memo_recording(media_dir, page);
            }
        }
    }

    fn audio_fab_button(
        ui: &mut egui::Ui,
        glyph: &str,
        accent: egui::Color32,
        base: egui::Color32,
        border: egui::Color32,
        foreground: egui::Color32,
        muted: egui::Color32,
        active: bool,
        enabled: bool,
        size: f32,
        tooltip: &str,
        has_content: bool, // True when lesson audio is available.
    ) -> egui::Response {
        ui.scope(|ui| {
            // Use the content color when audio is available.
            let effective_accent = if has_content && !active {
                foreground
            } else {
                accent
            };

            let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());

            let hovered = resp.hovered();

            // Compute background and border colors.
            let bg_fill = if hovered {
                effective_accent.gamma_multiply(0.24)
            } else if active {
                effective_accent.gamma_multiply(0.30)
            } else if has_content {
                effective_accent.gamma_multiply(0.12)
            } else {
                base.gamma_multiply(0.92)
            };

            let stroke_col = if hovered || active {
                effective_accent.gamma_multiply(0.9)
            } else if has_content {
                effective_accent.gamma_multiply(0.5)
            } else {
                border.gamma_multiply(0.55)
            };

            // Draw the circular button background.
            ui.painter().circle(
                rect.center(),
                size / 2.0,
                bg_fill,
                egui::Stroke::new(1.0, stroke_col),
            );

            let icon_color = if !enabled {
                muted
            } else if active || has_content {
                effective_accent
            } else {
                foreground
            };

            let icon_galley = ui.painter().layout_no_wrap(
                glyph.to_owned(),
                egui::FontId::monospace(12.0),
                icon_color,
            );
            ui.painter().galley(
                rect.center() - icon_galley.size() * 0.5,
                icon_galley,
                icon_color,
            );

            resp.on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(tooltip)
        })
        .inner
    }

    fn render_audio_trimmer(&mut self, ui: &mut egui::Ui, media_dir: &Path) {
        let avail_w = ui.available_width();
        let h_margin = if avail_w < 165.0 { 6 } else { 10 };

        egui::Frame::NONE
            .fill(rgb(self.theme.bg_canvas))
            .stroke(egui::Stroke::new(
                1.0,
                rgb(self.theme.accent).gamma_multiply(0.5),
            ))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::symmetric(h_margin, 8))
            .show(ui, |ui| {
                let inner_w = ui.available_width();
                let compact = inner_w < 215.0;
                let ultra_narrow = inner_w < 150.0;

                ui.spacing_mut().item_spacing = if compact {
                    egui::vec2(4.0, 5.0)
                } else {
                    egui::vec2(6.0, 6.0)
                };

                let (full_title, short_title, max_dur_ms, target_wav) = match self.trim_memo_target
                {
                    None => (
                        "󰆐 Trim Lecture".to_string(),
                        "󰆐 Trim".to_string(),
                        self.audio_index.lecture_duration_ms.max(1),
                        self.audio_index
                            .lecture_wav
                            .as_ref()
                            .map(|n| media_dir.join(n)),
                    ),
                    Some((p, idx)) => {
                        let memo = self.audio_index.page_memos.get(&p).and_then(|v| v.get(idx));
                        match memo {
                            Some(m) => {
                                let name = m
                                    .title
                                    .as_deref()
                                    .filter(|t| !t.trim().is_empty())
                                    .unwrap_or(&m.id);
                                (
                                    format!("󰆐 Trim {} (p.{})", name, p),
                                    "󰆐 Trim".to_string(),
                                    m.duration_ms.max(1),
                                    Some(media_dir.join(&m.filename)),
                                )
                            }
                            None => {
                                self.show_audio_trimmer = false;
                                return;
                            }
                        }
                    }
                };

                self.trim_end_ms = self.trim_end_ms.clamp(200, max_dur_ms);
                self.trim_start_ms = self
                    .trim_start_ms
                    .clamp(0, self.trim_end_ms.saturating_sub(200));
                let cur_pos = if self.trim_memo_target.is_some() {
                    self.memo_audio_engine.current_pos_ms()
                } else {
                    self.audio_engine.current_pos_ms()
                };
                let kept_ms = self.trim_end_ms.saturating_sub(self.trim_start_ms);
                let kept_str = audio::format_ms(kept_ms);

                // ---------------------------------------------------------
                // Responsive header without close-button overlap.
                // ---------------------------------------------------------
                ui.horizontal(|ui| {
                    let row_w = ui.available_width();
                    let close_btn_space = 24.0;
                    let left_max_w = (row_w - close_btn_space).max(24.0);

                    ui.allocate_ui_with_layout(
                        egui::vec2(left_max_w, 18.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |left_ui| {
                            left_ui.set_max_width(left_max_w);
                            left_ui.spacing_mut().item_spacing.x = 5.0;

                            if ultra_narrow {
                                left_ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(format!("󰆐 {}", kept_str))
                                            .strong()
                                            .monospace()
                                            .size(10.5)
                                            .color(rgb(self.theme.accent)),
                                    )
                                    .truncate(),
                                );
                            } else {
                                let hdr_txt = if compact { &short_title } else { &full_title };
                                left_ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(hdr_txt)
                                            .strong()
                                            .size(11.0)
                                            .color(rgb(self.theme.accent)),
                                    )
                                    .truncate(),
                                );

                                let sel_txt = if compact {
                                    kept_str.clone()
                                } else {
                                    format!("Selected: {}", kept_str)
                                };
                                if left_ui.available_width() > 28.0 {
                                    left_ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(sel_txt)
                                                .monospace()
                                                .size(10.0)
                                                .color(rgb(self.theme.fg_muted)),
                                        )
                                        .truncate(),
                                    );
                                }
                            }
                        },
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .small_button("󰅖")
                            .on_hover_text("Close trimmer without cutting")
                            .clicked()
                        {
                            self.show_audio_trimmer = false;
                        }
                    });
                });

                // ---------------------------------------------------------
                // Timeline with trim handles.
                // ---------------------------------------------------------
                let bar_h = 28.0;
                let (bar_rect, bar_resp) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width().max(8.0), bar_h),
                    egui::Sense::click_and_drag(),
                );
                let bar_resp = bar_resp
                    .on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
                    .on_hover_text(format!(
                        "In: {}  |  Out: {}  (Selected: {})",
                        audio::format_ms(self.trim_start_ms),
                        audio::format_ms(self.trim_end_ms),
                        kept_str
                    ));

                let start_frac = (self.trim_start_ms as f32 / max_dur_ms as f32).clamp(0.0, 1.0);
                let end_frac = (self.trim_end_ms as f32 / max_dur_ms as f32).clamp(0.0, 1.0);
                let play_frac = (cur_pos as f32 / max_dur_ms as f32).clamp(0.0, 1.0);

                let x_start = bar_rect.left() + start_frac * bar_rect.width();
                let x_end = bar_rect.left() + end_frac * bar_rect.width();
                let x_play = bar_rect.left() + play_frac * bar_rect.width();

                if (bar_resp.clicked() || bar_resp.dragged())
                    && let Some(ptr) = ui.input(|i| i.pointer.interact_pos())
                {
                    let frac = ((ptr.x - bar_rect.left()) / bar_rect.width()).clamp(0.0, 1.0);
                    let target_ms = (frac * max_dur_ms as f32) as u64;
                    let dist_to_start = (ptr.x - x_start).abs();
                    let dist_to_end = (ptr.x - x_end).abs();

                    if dist_to_start <= dist_to_end {
                        self.trim_start_ms = target_ms.min(self.trim_end_ms.saturating_sub(200));
                    } else {
                        self.trim_end_ms = target_ms.max(self.trim_start_ms + 200).min(max_dur_ms);
                    }
                }

                let painter = ui.painter_at(bar_rect);
                painter.rect_filled(
                    bar_rect,
                    egui::CornerRadius::same(6),
                    rgb(self.theme.bg_panel),
                );

                if self.trim_memo_target.is_none() && !self.audio_index.waveform_peaks.is_empty() {
                    let peaks = &self.audio_index.waveform_peaks;
                    let n = peaks.len().max(1);
                    let step_x = bar_rect.width() / n as f32;
                    for (idx, &pk) in peaks.iter().enumerate() {
                        let x = bar_rect.left() + (idx as f32 + 0.5) * step_x;
                        let half_h = (pk * (bar_h * 0.40)).max(1.0);
                        let in_keep = x >= x_start && x <= x_end;
                        let col = if in_keep {
                            rgb(self.theme.accent)
                        } else {
                            egui::Color32::from_gray(70)
                        };
                        painter.line_segment(
                            [
                                egui::pos2(x, bar_rect.center().y - half_h),
                                egui::pos2(x, bar_rect.center().y + half_h),
                            ],
                            egui::Stroke::new((step_x * 0.55).clamp(1.0, 2.2), col),
                        );
                    }
                }

                // Selected region and dimmed trim areas.
                let keep_rect = egui::Rect::from_min_max(
                    egui::pos2(x_start, bar_rect.top()),
                    egui::pos2(x_end, bar_rect.bottom()),
                );
                painter.rect_filled(
                    keep_rect,
                    egui::CornerRadius::same(3),
                    rgb(self.theme.accent).gamma_multiply(0.14),
                );
                painter.rect_stroke(
                    keep_rect,
                    egui::CornerRadius::same(3),
                    egui::Stroke::new(1.5, rgb(self.theme.accent)),
                    egui::StrokeKind::Inside,
                );

                if x_start > bar_rect.left() + 1.0 {
                    let cut_l = egui::Rect::from_min_max(
                        bar_rect.left_top(),
                        egui::pos2(x_start, bar_rect.bottom()),
                    );
                    painter.rect_filled(
                        cut_l,
                        egui::CornerRadius::same(4),
                        egui::Color32::from_black_alpha(155),
                    );
                }
                if x_end < bar_rect.right() - 1.0 {
                    let cut_r = egui::Rect::from_min_max(
                        egui::pos2(x_end, bar_rect.top()),
                        bar_rect.right_bottom(),
                    );
                    painter.rect_filled(
                        cut_r,
                        egui::CornerRadius::same(4),
                        egui::Color32::from_black_alpha(155),
                    );
                }

                painter.line_segment(
                    [
                        egui::pos2(x_play, bar_rect.top()),
                        egui::pos2(x_play, bar_rect.bottom()),
                    ],
                    egui::Stroke::new(1.2, egui::Color32::from_white_alpha(190)),
                );

                // Handles with center notches.
                for &hx in &[x_start, x_end] {
                    let grip = egui::Rect::from_center_size(
                        egui::pos2(hx, bar_rect.center().y),
                        egui::vec2(6.0, bar_h - 2.0),
                    );
                    painter.rect_filled(grip, egui::CornerRadius::same(3), rgb(self.theme.accent));
                    painter.line_segment(
                        [
                            egui::pos2(hx, bar_rect.center().y - 4.0),
                            egui::pos2(hx, bar_rect.center().y + 4.0),
                        ],
                        egui::Stroke::new(1.0, rgb(self.theme.bg_canvas)),
                    );
                }

                // ---------------------------------------------------------
                // CONTROLLI INFERIORI (DragValue + Set + Preview + Apply)
                // ---------------------------------------------------------
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = if compact { 4.0 } else { 6.0 };

                    let in_prefix = if ultra_narrow { "" } else { "In " };
                    let out_prefix = if ultra_narrow { "" } else { "Out " };

                    let mut start_sec = self.trim_start_ms as f64 / 1000.0;
                    let max_start_sec =
                        (self.trim_end_ms.saturating_sub(200) as f64 / 1000.0).max(0.0);
                    if ui
                        .add(
                            egui::DragValue::new(&mut start_sec)
                                .prefix(in_prefix)
                                .suffix("s")
                                .speed(0.1)
                                .fixed_decimals(1)
                                .range(0.0..=max_start_sec),
                        )
                        .on_hover_text("Drag to fine-tune Start time")
                        .changed()
                    {
                        self.trim_start_ms = (start_sec * 1000.0) as u64;
                    }
                    if ui
                        .small_button("[󰁨")
                        .on_hover_text("Set Start point to current playhead")
                        .clicked()
                    {
                        self.trim_start_ms = cur_pos.min(self.trim_end_ms.saturating_sub(200));
                    }

                    let mut end_sec = self.trim_end_ms as f64 / 1000.0;
                    let min_end_sec = ((self.trim_start_ms + 200) as f64 / 1000.0)
                        .min(max_dur_ms as f64 / 1000.0);
                    let max_end_sec = (max_dur_ms as f64 / 1000.0).max(0.2);
                    if ui
                        .add(
                            egui::DragValue::new(&mut end_sec)
                                .prefix(out_prefix)
                                .suffix("s")
                                .speed(0.1)
                                .fixed_decimals(1)
                                .range(min_end_sec..=max_end_sec),
                        )
                        .on_hover_text("Drag to fine-tune End time")
                        .changed()
                    {
                        self.trim_end_ms = (end_sec * 1000.0) as u64;
                    }
                    if ui
                        .small_button("󰁨]")
                        .on_hover_text("Set End point to current playhead")
                        .clicked()
                    {
                        self.trim_end_ms = cur_pos.max(self.trim_start_ms + 200).min(max_dur_ms);
                    }

                    let preview_lbl = if compact { "󰐊" } else { "󰐊 Preview" };
                    if ui
                        .small_button(preview_lbl)
                        .on_hover_text("Preview audio from the selected Start point")
                        .clicked()
                        && let Some(p) = &target_wav
                    {
                        let engine = if self.trim_memo_target.is_some() {
                            &mut self.memo_audio_engine
                        } else {
                            &mut self.audio_engine
                        };
                        let _ = engine.load_and_play(p, "Preview".to_string(), self.trim_start_ms);
                    }

                    let cut_lbl = if ultra_narrow {
                        "󰆐"
                    } else if compact {
                        "󰆐 Cut"
                    } else {
                        "󰆐 Apply Cut"
                    };

                    let cut_btn = egui::Button::new(
                        egui::RichText::new(cut_lbl)
                            .size(11.0)
                            .strong()
                            .color(rgb(self.theme.success)),
                    )
                    .fill(rgb(self.theme.success).gamma_multiply(0.15))
                    .stroke(egui::Stroke::new(
                        1.0,
                        rgb(self.theme.success).gamma_multiply(0.4),
                    ))
                    .corner_radius(egui::CornerRadius::same(5));

                    if ui
                        .add(cut_btn)
                        .on_hover_text("Permanently trim audio to [In .. Out]")
                        .clicked()
                        && let Some(wav) = target_wav
                    {
                        self.apply_audio_trim(&wav);
                    }
                });
            });
    }

    pub(super) fn remove_lecture_silence(&mut self, media_dir: &Path, wav: &Path) {
        let output = audio::recording_path(media_dir, "lecture");
        let mut index = self.audio_index.clone();
        let old = index.lecture_wav.clone();
        let result = (|| {
            let (before, after) =
                audio::strip_silences_from_lecture(wav, &output, &mut index, 1800)?;
            index.lecture_wav = Some(output.file_name().unwrap().to_string_lossy().into_owned());
            self.audio_engine.stop_and_release();
            self.publish_audio_index(index, media_dir, old.as_deref())?;
            Ok::<_, String>((before, after))
        })();
        match result {
            Ok((before, after)) => {
                self.show_audio_trimmer = false;
                self.transcript_last_seg = None;
                let playback = self
                    .audio_engine
                    .load_and_play(&output, "Lecture".into(), 0);
                if let Err(err) = playback {
                    self.audio_toast(format!("Silence removed. Playback could not start: {err}"));
                } else {
                    self.audio_toast(format!(
                        "󰄬 Removed {}s of silence",
                        before.saturating_sub(after) / 1000
                    ));
                }
            }
            Err(err) => {
                let _ = std::fs::remove_file(&output);
                self.audio_toast(format!("Could not remove silence: {err}"));
            }
        }
    }

    pub(super) fn apply_audio_trim(&mut self, wav: &Path) {
        if self.audio_engine.is_recording() || self.is_transcribing {
            self.audio_toast("Finish recording or transcription before trimming audio");
            return;
        }
        let media_dir = AudioDocIndex::media_dir_for_pdf(&self.pdf_path);
        let prefix = if self.trim_memo_target.is_some() {
            "memo"
        } else {
            "lecture"
        };
        let output = audio::recording_path(&media_dir, prefix);
        let result = (|| {
            let mut index = self.audio_index.clone();
            let old = match self.trim_memo_target {
                Some((page, idx)) => index
                    .page_memos
                    .get(&page)
                    .and_then(|memos| memos.get(idx))
                    .map(|memo| &memo.filename),
                None => index.lecture_wav.as_ref(),
            }
            .filter(|name| media_dir.join(name) == wav)
            .ok_or("The audio selected for trimming has changed")?
            .clone();
            let duration =
                audio::trim_wav_file(wav, &output, self.trim_start_ms, self.trim_end_ms)?;
            let new_name = output.file_name().unwrap().to_string_lossy().into_owned();
            match self.trim_memo_target {
                Some((page, idx)) => {
                    let memo = &mut index.page_memos.get_mut(&page).unwrap()[idx];
                    memo.filename = new_name;
                    memo.duration_ms = duration;
                    self.memo_audio_engine.stop_and_release();
                }
                None => {
                    index.apply_lecture_trim(
                        self.trim_start_ms,
                        self.trim_start_ms.saturating_add(duration),
                    );
                    index.lecture_wav = Some(new_name);
                    index.waveform_peaks = audio::compute_waveform_peaks(&output, 240);
                    self.audio_engine.stop_and_release();
                }
            }
            self.publish_audio_index(index, &media_dir, Some(&old))?;
            Ok::<_, String>(duration)
        })();
        match result {
            Ok(duration) => {
                self.show_audio_trimmer = false;
                self.transcript_last_seg = None;
                if self.trim_memo_target.is_none()
                    && let Err(err) = self
                        .audio_engine
                        .load_and_play(&output, "Lecture".into(), 0)
                {
                    self.audio_toast(format!("Audio trimmed. Playback could not start: {err}"));
                } else {
                    self.audio_toast(format!("󰄬 Audio trimmed ({})", audio::format_ms(duration)));
                }
            }
            Err(err) => {
                let _ = std::fs::remove_file(&output);
                self.audio_toast(format!("Trim error: {err}"));
            }
        }
    }

    fn finish_memo_recording(&mut self, media_dir: &Path) {
        if let Err(err) = self.finish_active_recording(media_dir) {
            self.audio_toast(format!("Could not finish recording: {err}"));
        }
    }

    fn ensure_asr_model(&mut self, ctx: &egui::Context, retry: AsrRetry) -> Option<PathBuf> {
        if let Some(p) = &self.asr_model_dir {
            return Some(p.clone());
        }
        if let Some(p) = audio::find_asr_model() {
            return Some(p);
        }
        // Open the folder picker without blocking; transcription resumes in
        // `handle_dialog_result` after the model is selected.
        self.dialogs.open(
            ctx,
            DialogKind::PickAsrModelDir { retry },
            DialogSpec::pick_folder()
                .title("Select the Parakeet model folder (vocab.txt + encoder/decoder .onnx)"),
        );
        None
    }

    fn render_lecture_picker(&mut self, ui: &mut egui::Ui) {
        self.audio_index.ensure_lecture_ids();

        let ix = &self.audio_index;

        let mut entries: Vec<(Option<usize>, String, Option<(u32, u32)>)> = Vec::new();

        let active_title = if ix.lecture_title.trim().is_empty() {
            format!("Lecture {}", ix.lecture_id)
        } else {
            format!("Lecture {} — {}", ix.lecture_id, ix.lecture_title)
        };

        entries.push((None, active_title, ix.page_range));

        for (i, lecture) in ix.other_lectures.iter().enumerate() {
            let title = if lecture.title.trim().is_empty() {
                format!("Lecture {}", lecture.id)
            } else {
                format!("Lecture {} — {}", lecture.id, lecture.title)
            };

            entries.push((Some(i), title, lecture.page_range));
        }

        let current_text = entries
            .iter()
            .find(|entry| entry.0.is_none())
            .map(|entry| match entry.2 {
                Some((a, b)) => {
                    format!("{}  (p.{}–{})", entry.1, a, b)
                }
                None => entry.1.clone(),
            })
            .unwrap_or_default();

        let mut switch_to: Option<usize> = None;
        let mut new_lec = false;

        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("lecture_picker")
                .selected_text(current_text)
                .show_ui(ui, |ui| {
                    for (slot, title, range) in &entries {
                        let display_str = match range {
                            Some((a, b)) => {
                                format!("{}  (p.{}–{})", title, a, b)
                            }
                            None => title.clone(),
                        };

                        let selected = slot.is_none();

                        if ui.selectable_label(selected, display_str).clicked()
                            && let Some(i) = slot
                        {
                            switch_to = Some(*i);
                        }
                    }
                });

            if ui
                .small_button("󰐕")
                .on_hover_text("Add another lecture")
                .clicked()
            {
                new_lec = true;
            }
        });

        if switch_to.is_some() || new_lec {
            if self.audio_engine.is_recording() || self.is_transcribing {
                self.audio_toast("Finish recording or transcription before switching lectures");
                return;
            }
            self.audio_engine.stop_and_release();

            match switch_to {
                Some(i) => {
                    self.audio_index.switch_to_other(i);
                }
                None => {
                    self.audio_index.start_new_lecture();
                }
            }

            self.transcript_last_seg = None;
            self.show_audio_trimmer = false;

            self.audio_index.save(&self.pdf_path);
        }
    }

    fn render_align_panel(&mut self, ui: &mut egui::Ui) {
        let total = self.total_pages.max(1);
        let (mut run, mut close, mut use_pins) = (false, false, false);
        egui::Frame::NONE
            .fill(rgb(self.theme.bg_canvas))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::symmetric(8, 8))
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new("󰌷 Align transcript to slides")
                        .strong()
                        .size(11.5),
                );
                ui.horizontal_wrapped(|ui| {
                    ui.label("From");
                    ui.add(egui::DragValue::new(&mut self.align_from).range(1..=total));
                    ui.label("to");
                    ui.add(egui::DragValue::new(&mut self.align_to).range(1..=total));
                    if ui.small_button("Current → from").clicked() {
                        self.align_from = self.current_page;
                    }
                    if ui.small_button("Current → to").clicked() {
                        self.align_to = self.current_page;
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button("Align").clicked() {
                        run = true;
                    }
                    if !self.audio_index.timeline.is_empty()
                        && ui
                            .button("Use pins")
                            .on_hover_text("Assign segment pages from existing pins")
                            .clicked()
                    {
                        use_pins = true;
                    }
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            });
        if self.align_from > self.align_to {
            self.align_to = self.align_from;
        }
        if run {
            self.show_align_panel = false;
            self.start_alignment(ui.ctx());
        }
        if use_pins {
            self.show_align_panel = false;
            self.audio_index.assign_pages_from_timeline();
            self.audio_index.timeline_is_live = true;
            self.transcript_all = false;
            self.audio_index.save(&self.pdf_path);
            self.audio_toast("󰄬 Assigned segment pages from pins");
        }
        if close {
            self.show_align_panel = false;
        }
    }

    pub(super) fn start_lecture_transcription(&mut self, wav_path: PathBuf, ctx: &egui::Context) {
        if !self.has_pdf() || self.is_transcribing || self.audio_engine.is_recording() {
            return;
        }
        let dir = AudioDocIndex::media_dir_for_pdf(&self.pdf_path);
        if self
            .audio_index
            .lecture_wav
            .as_ref()
            .map(|n| dir.join(n))
            .as_ref()
            != Some(&wav_path)
        {
            return;
        }
        let Some(model_dir) = self.ensure_asr_model(
            ctx,
            AsrRetry::Lecture {
                wav_path: wav_path.clone(),
            },
        ) else {
            return;
        };

        self.is_transcribing = true;
        self.asr_progress = Some((0, self.audio_index.lecture_duration_ms.max(1)));

        let (tx, cancel) = self.new_asr_job();
        let w_ctx = ctx.clone();

        thread::spawn(move || {
            audio::transcribe_lecture(&model_dir, &wav_path, &cancel, &mut |msg| {
                let _ = tx.send(msg);
                w_ctx.request_repaint();
            });
        });
    }

    fn start_alignment(&mut self, ctx: &egui::Context) {
        if self.is_transcribing
            || self.audio_engine.is_recording()
            || self.audio_index.segments.is_empty()
        {
            return;
        }
        let (lo, hi) = (self.align_from.max(1), self.align_to.min(self.total_pages));
        if lo > hi {
            self.audio_toast("󰅚 Invalid page range");
            return;
        }
        self.is_transcribing = true; // riusa il flag "busy" del dock
        self.asr_progress = None;
        let mut idx = self.audio_index.clone();
        let (pdf, total, notes) = (self.pdf_path.clone(), self.total_pages, self.notes.clone());
        let (tx, cancel) = self.new_asr_job();
        let ctx = ctx.clone();
        thread::spawn(move || {
            let texts = extract_slide_texts(&pdf, total, &notes);
            idx.align_to_slides(&texts, (lo, hi));
            let msg = if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                audio::AsrJobResult::Cancelled
            } else {
                audio::AsrJobResult::Aligned {
                    segments: idx.segments,
                    timeline: idx.timeline,
                    range: (lo, hi),
                }
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    pub(super) fn start_memo_transcription(
        &mut self,
        page: u32,
        memo_idx: usize,
        wav_path: PathBuf,
        ctx: &egui::Context,
    ) {
        if !self.has_pdf() || self.is_transcribing || self.audio_engine.is_recording() {
            return;
        }
        let Some(memo) = self
            .audio_index
            .page_memos
            .get(&page)
            .and_then(|list| list.get(memo_idx))
        else {
            return;
        };
        let filename = memo.filename.clone();
        if AudioDocIndex::media_dir_for_pdf(&self.pdf_path).join(&filename) != wav_path {
            return;
        }
        let Some(model_dir) = self.ensure_asr_model(
            ctx,
            AsrRetry::Memo {
                page,
                memo_idx,
                wav_path: wav_path.clone(),
            },
        ) else {
            return;
        };

        self.is_transcribing = true;
        self.asr_progress = None;
        self.audio_toast("󱑚 Transcribing voice memo...");

        let (tx, cancel) = self.new_asr_job();
        let w_ctx = ctx.clone();
        thread::spawn(move || {
            let msg = audio::transcribe_memo(&model_dir, &wav_path, filename, &cancel);
            let _ = tx.send(msg);
            w_ctx.request_repaint();
        });
    }

    pub(super) fn finish_audio_page_change(
        &mut self,
        change: notes::PageChange,
        original_pages: u32,
        message: impl Into<String>,
    ) {
        self.audio_index.remap_pages(change, original_pages);
        self.trim_memo_target = None;
        self.rename_memo_target = None;
        self.show_audio_trimmer = false;
        self.transcript_last_seg = None;
        let result = if self.audio_index_pending {
            self.persist_audio_index()
        } else {
            AudioDocIndex::recover_page_change(&self.pdf_path).map_err(|err| err.to_string())
        };
        match result {
            Ok(()) => {
                self.audio_index_pending = false;
                self.audio_toast(message);
            }
            Err(err) => {
                self.audio_index_pending = true;
                self.audio_toast(format!(
                    "Page changed. Audio index update is pending: {err}"
                ));
            }
        }
    }
}

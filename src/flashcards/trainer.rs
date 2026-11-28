use super::*;

impl Lapis {
    pub(super) fn export_anki_dialog(&mut self, ctx: &egui::Context) {
        let default_name = format!("{}_anki_cards.tsv", self.pdf_name.trim_end_matches(".pdf"));
        self.dialogs.open(
            ctx,
            DialogKind::ExportAnki,
            DialogSpec::save_file()
                .file_name(default_name)
                .filter("Anki TSV Deck", &["tsv", "txt"]),
        );
    }

    pub(super) fn finish_export_anki(&mut self, path: PathBuf) {
        match flashcards::export_flashcards_anki_tsv(&path, &self.pdf_name, &self.flashcards_deck) {
            Ok(_) => {
                self.toast_message = Some((
                    format!("󰄬 Exported {} cards", self.flashcards_deck.len()),
                    std::time::Instant::now(),
                ));
            }
            Err(e) => {
                self.toast_message =
                    Some((format!("󰅚 Export error: {}", e), std::time::Instant::now()));
            }
        }
    }

    pub(super) fn open_flashcards_trainer(&mut self) {
        self.sync_and_save();
        self.flashcards_deck = flashcards::extract_flashcards_from_notes(&self.notes);
        self.flashcard_idx = 0;
        self.flashcard_revealed = false;
        self.flashcard_stats = (0, 0, 0);
        self.show_flashcards_modal = true;
    }

    /// Opens the flashcards modal, resuming an active session if one is in progress,
    /// or scanning the document to start a new session if empty/finished.
    pub(super) fn toggle_flashcards_modal(&mut self) {
        if self.show_flashcards_modal {
            self.show_flashcards_modal = false;
            return;
        }

        let session_in_progress =
            !self.flashcards_deck.is_empty() && self.flashcard_idx < self.flashcards_deck.len();

        if session_in_progress {
            // Resume right where we left off!
            self.show_flashcards_modal = true;
        } else {
            // Deck is empty or session was finished -> scan notes and start fresh
            self.open_flashcards_trainer();
        }
    }

    /// Returns true if any slide in the document has at least one flashcard.
    pub(super) fn document_has_flashcards(&self) -> bool {
        self.notes
            .values()
            .any(|text| flashcards::page_has_flashcards(text))
    }

    pub(super) fn render_flashcards_modal(&mut self, ui: &mut egui::Ui) {
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.show_flashcards_modal = false;
            return;
        }

        // Dimmed backdrop
        let screen_rect = ui.ctx().content_rect();
        egui::Area::new(ui.make_persistent_id("flashcards_backdrop"))
            .fixed_pos(screen_rect.min)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                let (rect, resp) = ui.allocate_exact_size(screen_rect.size(), egui::Sense::click());
                ui.painter().rect_filled(
                    rect,
                    egui::CornerRadius::ZERO,
                    egui::Color32::from_black_alpha(165),
                );
                if resp.clicked() {
                    self.show_flashcards_modal = false;
                }
            });

        let mut jump_to_slide: Option<u32> = None;

        egui::Area::new(ui.make_persistent_id("flashcards_trainer_card"))
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .order(egui::Order::Tooltip)
            .show(ui.ctx(), |ui| {
                egui::Frame::window(ui.style())
                    .fill(rgb(self.theme.bg_elevated))
                    .stroke(egui::Stroke::new(1.5, rgb(self.theme.accent)))
                    .corner_radius(egui::CornerRadius::same(12))
                    .inner_margin(egui::Margin::same(20))
                    .show(ui, |ui| {
                        ui.set_width(580.0);
                        ui.set_min_height(350.0);

                        // Top Header Bar — allocate remaining width explicitly so right-to-left never overlaps title
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("󰘸  Active Recall & Flashcards")
                                    .strong()
                                    .size(16.5)
                                    .color(rgb(self.theme.fg_primary)),
                            );

                            let remaining = ui.available_size_before_wrap();
                            ui.allocate_ui_with_layout(
                                remaining,
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .add_sized([26.0, 24.0], egui::Button::new("󰅖"))
                                        .on_hover_text("Close [Esc]")
                                        .clicked()
                                    {
                                        self.show_flashcards_modal = false;
                                    }

                                    ui.add_space(4.0);

                                    if !self.flashcards_deck.is_empty()
                                        && ui
                                            .add_sized(
                                                [118.0, 24.0],
                                                egui::Button::new(
                                                    egui::RichText::new("󰈇 Export Anki").size(12.0),
                                                ),
                                            )
                                            .on_hover_text("Export deck (.tsv) for Anki Desktop / AnkiDroid")
                                            .clicked()
                                    {
                                        self.export_anki_dialog(ui.ctx());
                                    }

                                    if ui
                                        .add_sized(
                                            [80.0, 24.0],
                                            egui::Button::new(
                                                egui::RichText::new("󰑐 Refresh").size(12.0),
                                            ),
                                        )
                                        .on_hover_text("Re-scan document notes for flashcards")
                                        .clicked()
                                    {
                                        self.open_flashcards_trainer();
                                    }
                                },
                            );
                        });

                        ui.add_space(6.0);
                        ui.separator();

                        // Empty Deck State
                        if self.flashcards_deck.is_empty() {
                            ui.add_space(22.0);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    egui::RichText::new("󰘸  No Flashcards Found in Document")
                                        .strong()
                                        .size(16.0),
                                );
                                ui.add_space(8.0);
                                ui.label(
                                    egui::RichText::new(
                                        "Write flashcards directly inside any slide's Markdown notes using either syntax:",
                                    )
                                    .color(rgb(self.theme.fg_muted)),
                                );
                            });
                            ui.add_space(14.0);
                            egui::Frame::NONE
                                .fill(rgb(self.theme.bg_panel))
                                .stroke(egui::Stroke::new(1.0, rgb(self.theme.border)))
                                .corner_radius(egui::CornerRadius::same(8))
                                .inner_margin(egui::Margin::same(14))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.label(
                                        egui::RichText::new("1. Inline syntax (Term :: Definition)")
                                            .strong()
                                            .size(12.0)
                                            .color(rgb(self.theme.accent)),
                                    );
                                    ui.add_space(4.0);
                                    ui.label(
                                        egui::RichText::new(
                                            "Page Fault :: Exception raised by MMU when PTE valid bit is 0",
                                        )
                                        .monospace()
                                        .size(12.0),
                                    );
                                    ui.add_space(12.0);
                                    ui.label(
                                        egui::RichText::new("2. Multi-line Q/A syntax")
                                            .strong()
                                            .size(12.0)
                                            .color(rgb(self.theme.accent)),
                                    );
                                    ui.add_space(4.0);
                                    ui.label(
                                        egui::RichText::new(
                                            "Q: What is the time complexity of Heapify?\nA: $O(\\log n)$ worst-case per node, $O(n)$ bottom-up build.",
                                        )
                                        .monospace()
                                        .size(12.0),
                                    );
                                });
                            return;
                        }

                        // Completed Session State
                        if self.flashcard_idx >= self.flashcards_deck.len() {
                            ui.add_space(28.0);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    egui::RichText::new("󰄬  Deck Review Complete!")
                                        .strong()
                                        .size(19.0)
                                        .color(rgb(self.theme.success)),
                                );
                                ui.add_space(14.0);

                                let (hard, good, easy) = self.flashcard_stats;
                                egui::Frame::NONE
                                    .fill(rgb(self.theme.bg_panel))
                                    .stroke(egui::Stroke::new(1.0, rgb(self.theme.border)))
                                    .corner_radius(egui::CornerRadius::same(8))
                                    .inner_margin(egui::Margin::symmetric(20, 12))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new(format!("Again: {}", hard))
                                                    .strong()
                                                    .color(rgb(self.theme.error)),
                                            );
                                            ui.separator();
                                            ui.label(
                                                egui::RichText::new(format!("Good: {}", good))
                                                    .strong()
                                                    .color(rgb(self.theme.accent)),
                                            );
                                            ui.separator();
                                            ui.label(
                                                egui::RichText::new(format!("Easy: {}", easy))
                                                    .strong()
                                                    .color(rgb(self.theme.success)),
                                            );
                                        });
                                    });

                                ui.add_space(22.0);
                                if ui
                                    .add_sized(
                                        [190.0, 36.0],
                                        egui::Button::new(
                                            egui::RichText::new("󰑐  Restart Session").strong(),
                                        ),
                                    )
                                    .clicked()
                                {
                                    self.open_flashcards_trainer();
                                }
                            });
                            return;
                        }

                        let total_cards = self.flashcards_deck.len();
                        let card = self.flashcards_deck[self.flashcard_idx].clone();
                        let progress = (self.flashcard_idx + 1) as f32 / total_cards as f32;

                        ui.add_space(6.0);

                        // Progress & Slide Sync Bar
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "Card {} of {}",
                                    self.flashcard_idx + 1,
                                    total_cards
                                ))
                                .strong()
                                .color(rgb(self.theme.accent)),
                            );

                            ui.add_space(6.0);
                            ui.separator();
                            ui.add_space(6.0);

                            if ui
                                .small_button(format!("󰈙 Go to Slide {}", card.page))
                                .on_hover_text("Jump to this slide and hide modal to read (session progress is saved)")
                                .clicked()
                            {
                                jump_to_slide = Some(card.page);
                                self.show_flashcards_modal = false; // Hides modal so you can read the slide immediately!
                            }

                            let remaining = ui.available_size_before_wrap();
                            ui.allocate_ui_with_layout(
                                remaining,
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .checkbox(&mut self.flashcard_sync_slide, "Auto-sync slide")
                                        .changed()
                                        && self.flashcard_sync_slide
                                    {
                                        jump_to_slide = Some(card.page);
                                    }
                                },
                            );
                        });
                        ui.add_space(6.0);

                        // Sleek thin progress bar
                        let (prog_rect, _) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), 4.0),
                            egui::Sense::hover(),
                        );
                        ui.painter().rect_filled(
                            prog_rect,
                            egui::CornerRadius::same(2),
                            rgb(self.theme.bg_panel),
                        );
                        let mut fill_rect = prog_rect;
                        fill_rect.set_width(prog_rect.width() * progress.clamp(0.0, 1.0));
                        ui.painter().rect_filled(
                            fill_rect,
                            egui::CornerRadius::same(2),
                            rgb(self.theme.accent),
                        );

                        ui.add_space(12.0);

                        // Question Card Box (Front)
                        egui::Frame::NONE
                            .fill(rgb(self.theme.bg_panel))
                            .stroke(egui::Stroke::new(1.0, rgb(self.theme.border)))
                            .corner_radius(egui::CornerRadius::same(8))
                            .inner_margin(egui::Margin::same(14))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.label(
                                    egui::RichText::new("QUESTION")
                                        .size(10.5)
                                        .strong()
                                        .color(rgb(self.theme.fg_muted)),
                                );
                                ui.add_space(6.0);
                                egui::ScrollArea::vertical()
                                    .id_salt("fc_question_scroll")
                                    .max_height(130.0)
                                    .show(ui, |ui| {
                                        let q_md = markdown::preprocess_markdown_view(
                                            &flashcards::escape_stray_asterisks(&card.front),
                                        );
                                        egui_commonmark::CommonMarkViewer::new().show(
                                            ui,
                                            &mut self.commonmark_cache,
                                            &q_md,
                                        );
                                    });
                            });

                        ui.add_space(12.0);

                        // Answer Card Box (Back) or Reveal Button
                        if !self.flashcard_revealed {
                            let flip_key = ui.input(|i| {
                                i.key_pressed(egui::Key::Space) || i.key_pressed(egui::Key::Enter)
                            });
                            ui.vertical_centered(|ui| {
                                ui.add_space(24.0);
                                if ui
                                    .add_sized(
                                        [250.0, 40.0],
                                        egui::Button::new(
                                            egui::RichText::new("󰈈  Show Answer  [Space]")
                                                .strong()
                                                .size(14.0),
                                        ),
                                    )
                                    .clicked()
                                    || flip_key
                                {
                                    self.flashcard_revealed = true;
                                }
                                ui.add_space(12.0);
                            });
                        } else {
                            egui::Frame::NONE
                                .fill(rgb(self.theme.bg_canvas))
                                .stroke(egui::Stroke::new(1.2, rgb(self.theme.success)))
                                .corner_radius(egui::CornerRadius::same(8))
                                .inner_margin(egui::Margin::same(14))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.label(
                                        egui::RichText::new("ANSWER")
                                            .size(10.5)
                                            .strong()
                                            .color(rgb(self.theme.success)),
                                    );
                                    ui.add_space(6.0);
                                    egui::ScrollArea::vertical()
                                        .id_salt("fc_answer_scroll")
                                        .max_height(150.0)
                                        .show(ui, |ui| {
                                            let a_md = markdown::preprocess_markdown_view(&card.back);
                                            egui_commonmark::CommonMarkViewer::new().show(
                                                ui,
                                                &mut self.commonmark_cache,
                                                &a_md,
                                            );
                                        });
                                });

                            ui.add_space(16.0);

                            // Self-grading buttons (1 = Again/Requeue, 2 = Good, 3 = Easy)
                            let (k1, k2, k3) = ui.input(|i| {
                                (
                                    i.key_pressed(egui::Key::Num1),
                                    i.key_pressed(egui::Key::Num2)
                                        || i.key_pressed(egui::Key::ArrowRight),
                                    i.key_pressed(egui::Key::Num3),
                                )
                            });

                            ui.columns(3, |cols| {
                                if cols[0]
    .add_sized(
        [cols[0].available_width(), 34.0],
        egui::Button::new(
            egui::RichText::new("󰑐 Again [1]")
                .strong()
                .color(rgb(self.theme.error)),
        ),
    )
    .on_hover_text("Move card to the end of the remaining queue")
    .clicked()
    || k1
{
    self.flashcard_stats.0 += 1;
    // Move current card to the back of the deck WITHOUT changing deck length or flashcard_idx!
    let missed_card = self.flashcards_deck.remove(self.flashcard_idx);
    self.flashcards_deck.push(missed_card);
    self.flashcard_revealed = false;

    if self.flashcard_sync_slide
        && let Some(next_c) = self.flashcards_deck.get(self.flashcard_idx) {
            jump_to_slide = Some(next_c.page);
        }
}

                                if cols[1]
                                    .add_sized(
                                        [cols[1].available_width(), 34.0],
                                        egui::Button::new(
                                            egui::RichText::new("󰄬 Good [2 / →]")
                                                .strong()
                                                .color(rgb(self.theme.accent)),
                                        ),
                                    )
                                    .clicked()
                                    || k2
                                {
                                    self.flashcard_stats.1 += 1;
                                    self.flashcard_idx += 1;
                                    self.flashcard_revealed = false;
                                    if self.flashcard_sync_slide
                                        && let Some(next_c) =
                                            self.flashcards_deck.get(self.flashcard_idx)
                                        {
                                            jump_to_slide = Some(next_c.page);
                                        }
                                }

                                if cols[2]
                                    .add_sized(
                                        [cols[2].available_width(), 34.0],
                                        egui::Button::new(
                                            egui::RichText::new("󰄬 Easy [3]")
                                                .strong()
                                                .color(rgb(self.theme.success)),
                                        ),
                                    )
                                    .clicked()
                                    || k3
                                {
                                    self.flashcard_stats.2 += 1;
                                    self.flashcard_idx += 1;
                                    self.flashcard_revealed = false;
                                    if self.flashcard_sync_slide
                                        && let Some(next_c) =
                                            self.flashcards_deck.get(self.flashcard_idx)
                                        {
                                            jump_to_slide = Some(next_c.page);
                                        }
                                }
                            });
                        }
                    });
            });

        if let Some(target_p) = jump_to_slide {
            self.go_to_page(target_p, ui.ctx());
        }
    }
}

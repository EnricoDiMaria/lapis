//! Searchable workspace commands and keyboard navigation.

use super::super::*;

pub(in crate::workspace) const COMMAND_PALETTE_LABEL: &str = "󰘳  Command Palette…";
const INPUT_ID: &str = "lapis_command_palette_input";

const COMMANDS: &[(&str, &str, &str)] = &[
    (
        "toggle_split",
        "󰯌  Toggle Split-View (Side-by-Side PDF)",
        "View",
    ),
    (
        "mode_continuous",
        "󰍜  Switch to Continuous Scroll Mode",
        "View",
    ),
    ("mode_single", "󰈙  Switch to Single Page Mode", "View"),
    ("mode_book", "󰂺  Switch to Book View (Two Pages)", "View"),
    (
        "mode_toggle_cont_single",
        "󰍜  Toggle Continuous / Single Page",
        "View",
    ),
    (
        "mode_slideshow",
        "󰐨  Enter Presentation / Slideshow Mode",
        "View",
    ),
    ("toggle_fullscreen", "󰊓  Toggle Fullscreen", "View"),
    (
        "toggle_sketch",
        "󰠱  Toggle Sketch (page scratchpad)",
        "View",
    ),
    ("toggle_topbar", "󰁝  Toggle Topbar Auto-hide", "View"),
    ("fit_page", "󰊓  Fit PDF and Notes Layout", "View"),
    ("fit_pdf_page", "󰊓  Fit Page", "View"),
    (
        "toggle_topbar_position",
        "󰁝  Move Topbar to Bottom / Top",
        "View",
    ),
    ("toggle_dark", "󰔎  Toggle PDF Smart Dark Mode", "Appearance"),
    (
        "insert_blank",
        "󰐕  Insert Blank Ruled Page After Current Slide",
        "Document",
    ),
    ("delete_page", "󰆴  Delete Current Page from PDF", "Document"),
    (
        "download_orig",
        "󰇚  Download / Save Original Unmodified PDF...",
        "Document",
    ),
    ("global_search", "󰍉  Search notes, slides & audio", "Search"),
    (
        "flashcards",
        "󰘸  Open Active Recall Flashcards Deck",
        "Study",
    ),
    (
        "export_anki",
        "󰈇  Export Notes Flashcards to Anki (.tsv)",
        "Study",
    ),
    ("tool_cursor", "󰇀  Tool: Cursor / Text Selection", "Tools"),
    (
        "tool_pen",
        "󰏫  Tool: Freehand Pen (Chaikin Smoothed)",
        "Tools",
    ),
    ("tool_highlight", "  Tool: Highlighter", "Tools"),
    (
        "tool_tape",
        "󰝤  Tool: Occlusion Tape (Active Recall)",
        "Tools",
    ),
    ("tool_laser", "󰐾  Tool: Laser Pointer", "Tools"),
    (
        "tool_snip",
        "󰆞  Tool: Snip Region to Markdown Notes",
        "Tools",
    ),
    ("tool_eraser", "󰇾  Tool: Eraser", "Tools"),
    ("undo", "󰕌  Undo PDF Annotation", "Edit"),
    ("redo", "󰑎  Redo PDF Annotation", "Edit"),
];

struct CommandMatch {
    id: &'static str,
    label: String,
    category: &'static str,
    page: Option<u32>,
    enabled: bool,
}

fn palette_size(screen: egui::Vec2) -> (f32, f32) {
    (
        (screen.x - 56.0).clamp(1.0, 540.0),
        (screen.y - 128.0).clamp(24.0, 280.0),
    )
}

impl Lapis {
    pub(in crate::workspace) fn open_command_palette(&mut self, ctx: &egui::Context) {
        egui::Popup::close_all(ctx);
        self.show_command_palette = true;
        self.command_palette_query.clear();
        self.command_palette_selected = 0;
    }

    pub(in crate::workspace) fn close_command_palette(&mut self, ctx: &egui::Context) {
        self.show_command_palette = false;
        ctx.memory_mut(|m| m.surrender_focus(egui::Id::unique(INPUT_ID)));
    }

    fn matching_palette_commands(&self) -> Vec<CommandMatch> {
        let q = self.command_palette_query.trim().to_lowercase();
        let mut filtered = Vec::new();
        if let Some(page) = q
            .strip_prefix('p')
            .unwrap_or(&q)
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|page| self.has_pdf() && (1..=self.total_pages).contains(page))
        {
            filtered.push(CommandMatch {
                id: "__jump_page__",
                label: format!("󰁔  Jump to Page {page}"),
                category: "Navigation",
                page: Some(page),
                enabled: true,
            });
        }
        for &(id, label, cat) in COMMANDS {
            let search_label = label.to_lowercase();
            if q.is_empty()
                || search_label.contains(&q)
                || cat.to_lowercase().contains(&q)
                || q.split_whitespace().all(|tok| search_label.contains(tok))
            {
                let sc = &self.app_state.shortcuts;
                let binding = match id {
                    "mode_continuous" => Some(&sc.view_continuous),
                    "mode_single" => Some(&sc.view_single),
                    "mode_book" => Some(&sc.view_twopage),
                    "mode_toggle_cont_single" => Some(&sc.toggle_continuous_single),
                    "mode_slideshow" => Some(&sc.view_slideshow),
                    "toggle_fullscreen" => Some(&sc.toggle_fullscreen),
                    "toggle_sketch" => Some(&sc.toggle_sketch),
                    "toggle_topbar" => Some(&sc.toggle_topbar),
                    "fit_page" => Some(&sc.fit_page),
                    "fit_pdf_page" => Some(&sc.fit_pdf_page),
                    "toggle_dark" => Some(&sc.toggle_night),
                    "global_search" => Some(&sc.search_notes),
                    "tool_cursor" => Some(&sc.select_cursor),
                    "tool_pen" => Some(&sc.select_pen),
                    "tool_highlight" => Some(&sc.select_highlighter),
                    "tool_tape" => Some(&sc.select_tape),
                    "tool_snip" => Some(&sc.select_snip),
                    "tool_eraser" => Some(&sc.select_eraser),
                    _ => None,
                };
                let enabled = match id {
                    "undo" => self.can_undo_annotation(self.current_page),
                    "redo" => self.can_redo_annotation(self.current_page),
                    _ => true,
                };
                filtered.push(CommandMatch {
                    id,
                    label: binding.map_or_else(|| label.to_owned(), |b| b.hint(label)),
                    category: cat,
                    page: None,
                    enabled,
                });
            }
        }
        filtered
    }

    pub(in crate::workspace) fn render_command_palette(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let (up, down, enter) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
            )
        });
        let (width, list_height) = palette_size(ctx.content_rect().size());
        let modal = egui::Modal::new(ui.make_persistent_id("command_palette"))
            .frame(self.modal_glass_frame(MENU_GLASS_ALPHA))
            .show(&ctx, |ui| {
                ui.set_width(width);
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut self.command_palette_query)
                        .id(egui::Id::unique(INPUT_ID))
                        .hint_text("Search commands or jump to a page")
                        .desired_width(width),
                );
                edit.request_focus();

                // Match after TextEdit processes this frame's input, including text and Enter together.
                let matches = self.matching_palette_commands();
                let last = matches.len().saturating_sub(1);
                let selection = &mut self.command_palette_selected;
                *selection = (*selection).min(last);
                let moved = (up || down) && !matches.is_empty();
                if edit.changed() {
                    *selection = 0;
                } else if up && moved {
                    *selection = if *selection == 0 {
                        last
                    } else {
                        *selection - 1
                    };
                } else if down && moved {
                    *selection = (*selection + 1) % matches.len();
                }

                let mut activated = enter
                    .then(|| matches.get(*selection))
                    .flatten()
                    .filter(|command| command.enabled)
                    .map(|command| (command.id, command.page));
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_salt("command_palette_results")
                    .auto_shrink([false, true])
                    .max_height(list_height)
                    .show(ui, |ui| {
                        if matches.is_empty() {
                            ui.weak("No matching commands");
                        }
                        for (index, command) in matches.iter().enumerate() {
                            let selected = index == *selection;
                            let button =
                                theme::selected_button(&command.label, selected, ui.visuals())
                                    .truncate()
                                    .min_size(egui::vec2(ui.available_width(), 28.0));
                            let response = ui
                                .add_enabled(command.enabled, button)
                                .on_hover_text(format!("{} · {}", command.category, command.label));
                            if selected && (moved || edit.changed()) {
                                response.scroll_to_me(None);
                            }
                            if response.clicked() {
                                activated = Some((command.id, command.page));
                            }
                        }
                    });
                activated
            });

        if modal.should_close() {
            self.close_command_palette(&ctx);
        } else if let Some((id, page)) = modal.inner {
            self.close_command_palette(&ctx);
            self.execute_palette_command(id, page, &ctx);
        }
    }

    fn execute_palette_command(
        &mut self,
        cmd_id: &str,
        opt_page: Option<u32>,
        ctx: &egui::Context,
    ) {
        match cmd_id {
            "__jump_page__" => {
                if let Some(p) = opt_page {
                    self.go_to_page(p, ctx);
                }
            }
            "toggle_split" => {
                self.split_view_enabled = !self.split_view_enabled;
                if self.split_view_enabled {
                    self.split_secondary_page = self.current_page;
                }
            }
            "mode_continuous" => self.set_view_mode(ViewMode::Continuous),
            "mode_book" => self.set_view_mode(ViewMode::TwoPage),
            "mode_single" => self.set_view_mode(ViewMode::SinglePage),
            "mode_toggle_cont_single" => {
                if self.view_mode == ViewMode::Continuous {
                    self.set_view_mode(ViewMode::SinglePage);
                } else if !matches!(
                    self.view_mode,
                    ViewMode::Slideshow | ViewMode::SlideshowWithNotes
                ) {
                    self.set_view_mode(ViewMode::Continuous);
                }
            }
            "mode_slideshow" => self.set_view_mode(ViewMode::Slideshow),
            "toggle_sketch" => self.toggle_sketch_tab(),
            "toggle_topbar" if self.has_pdf() => self.toggle_topbar(),
            "fit_page" => self.request_fit_notes_layout(ctx),
            "fit_pdf_page" => {
                self.zoom_mode = ZoomMode::FitPage;
                self.zoom_anchor = None;
            }
            "toggle_topbar_position" => self.toggle_topbar_position(),
            "toggle_fullscreen" => {
                if !matches!(
                    self.view_mode,
                    ViewMode::Slideshow | ViewMode::SlideshowWithNotes
                ) {
                    self.user_fullscreen = !self.user_fullscreen;
                }
            }
            "toggle_dark" => {
                self.pdf_dark_mode = !self.pdf_dark_mode;
                self.viewer.invalidate_images();
                ctx.request_repaint();
            }
            "insert_blank" => self.insert_blank_page_after(
                self.current_page,
                self.app_state.settings.default_page_style,
                ctx,
            ),
            "delete_page" => self.request_delete_page(self.current_page, ctx),
            "download_orig" => self.prompt_download_original_pdf(ctx),
            "global_search" => {
                self.open_search();
            }
            "flashcards" => {
                self.sync_and_save();
                self.toggle_flashcards_modal();
            }
            "export_anki" => {
                self.sync_and_save();
                self.export_anki_dialog(ctx);
            }
            "tool_cursor" => self.annotation_tool = AnnotationTool::Cursor,
            "tool_pen" => self.annotation_tool = AnnotationTool::Draw,
            "tool_highlight" => self.annotation_tool = AnnotationTool::Highlight,
            "tool_tape" => self.annotation_tool = AnnotationTool::Tape,
            "tool_laser" => self.annotation_tool = AnnotationTool::Laser,
            "tool_snip" => self.annotation_tool = AnnotationTool::Snip,
            "tool_eraser" => self.annotation_tool = AnnotationTool::Eraser,
            "undo" => self.undo_annotation(self.current_page),
            "redo" => self.redo_annotation(self.current_page),
            "paste_screenshot" => {
                let page = self.current_page;
                self.shots.request_clipboard(
                    ctx,
                    ShotTarget::Pdf {
                        page,
                        center: (0.5, 0.5),
                    },
                );
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::isolated_dir;

    #[test]
    fn entering_a_query_and_confirming_in_one_frame_uses_the_new_result() {
        let root = isolated_dir!(entering_a_query_and_confirming_in_one_frame_uses_the_new_result);
        let pdf = root.join("commands.pdf");
        notes::create_blank_study_notebook(&pdf).unwrap();
        let ctx = egui::Context::default();
        let mut app = Lapis::new(
            &eframe::CreationContext::_new_kittest(ctx.clone()),
            Some(pdf),
        );
        let enter = egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let draw = |app: &mut Lapis, events| {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(560.0, 400.0),
                )),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(raw, |ui| app.render_command_palette(ui));
            output.textures_delta.clear();
        };
        app.open_command_palette(&ctx);
        draw(&mut app, vec![]);
        draw(&mut app, vec![]);
        assert!(ctx.memory(|memory| memory.has_focus(egui::Id::unique(INPUT_ID))));

        draw(
            &mut app,
            vec![egui::Event::Text("book".into()), enter.clone()],
        );
        assert_eq!(app.view_mode, ViewMode::TwoPage);
        assert!(!app.split_view_enabled);
        assert!(!app.show_command_palette);

        app.open_command_palette(&ctx);
        draw(&mut app, vec![]);
        draw(
            &mut app,
            vec![egui::Event::Text("no matching command".into()), enter],
        );
        assert!(app.show_command_palette);
        assert!(!app.split_view_enabled);
        assert_eq!(app.view_mode, ViewMode::TwoPage);
        draw(
            &mut app,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(!app.show_command_palette);
    }
}

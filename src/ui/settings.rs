//! Settings modal and its page layouts.

use super::super::*;

use super::storage::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::workspace) enum SettingsTab {
    StudyAndTools,
    Workspace,
    AppearanceAndTheme,
    Shortcuts,
    SystemAndData,
    Storage,
    Dictionaries,
    Credits,
}

// ───────── Advanced Settings layout helpers ─────────
// Every settings page is built from the same pieces: a page header, titled cards (sections)
// that group related options, small sub-headings inside a card, and options that carry a short
// explanation underneath.

/// Page title with a one-line description.
fn settings_page_header(ui: &mut egui::Ui, title: &str, description: &str) {
    ui.label(egui::RichText::new(title).size(18.0).strong());
    ui.weak(description);
    ui.add_space(10.0);
}

/// A titled card that groups related options.
fn settings_section<R>(
    ui: &mut egui::Ui,
    title: &str,
    description: &str,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let out = ui
        .group(|ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(title).size(14.0).strong());
            if !description.is_empty() {
                ui.weak(description);
            }
            ui.add_space(6.0);
            add_contents(ui)
        })
        .inner;
    ui.add_space(10.0);
    out
}

/// Small heading for a group of options inside a card.
fn settings_subheading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(8.0);
    ui.label(egui::RichText::new(text).size(12.5).strong());
    ui.add_space(2.0);
}

/// Explanation under a checkbox or radio button, aligned with its label.
fn settings_hint(ui: &mut egui::Ui, text: &str) {
    let indent = ui.spacing().icon_width + ui.spacing().icon_spacing;
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: indent as i8,
            right: 0,
            top: 0,
            bottom: 3,
        })
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).size(11.5).weak());
        });
}

/// Checkbox with an explanation underneath.
fn settings_toggle(ui: &mut egui::Ui, value: &mut bool, label: &str, hint: &str) -> egui::Response {
    let response = ui.checkbox(value, label);
    settings_hint(ui, hint);
    response
}

/// Row of mutually exclusive choices drawn as buttons. Returns true when the choice changed.
fn settings_choice<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    current: &mut T,
    options: &[(T, &str)],
    min_width: f32,
) -> bool {
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        for &(value, label) in options {
            let button = crate::theme::selected_button(label, *current == value, ui.visuals())
                .min_size(egui::vec2(min_width, 28.0));
            if ui.add(button).clicked() && *current != value {
                *current = value;
                changed = true;
            }
        }
    });
    changed
}

/// Button with inline confirmation: first click -> "Sure? Yes / No". Returns true upon confirmation.
fn danger_button(
    ui: &mut egui::Ui,
    confirm: &mut String,
    key: String,
    label: &str,
    tip: &str,
    enabled: bool,
) -> bool {
    if *confirm == key {
        let mut fired = false;
        ui.label(
            egui::RichText::new("Sure?")
                .color(egui::Color32::from_rgb(251, 113, 133))
                .strong(),
        );
        if ui.small_button("Yes").clicked() {
            fired = true;
            confirm.clear();
        }
        if ui.small_button("No").clicked() {
            confirm.clear();
        }
        fired
    } else {
        if ui
            .add_enabled(enabled, egui::Button::new(label))
            .on_hover_text(tip)
            .clicked()
        {
            *confirm = key;
        }
        false
    }
}

impl Lapis {
    pub(in crate::workspace) fn render_settings_modal(&mut self, ui: &mut egui::Ui) {
        if !self.show_settings {
            return;
        }

        let screen = ui.ctx().content_rect();
        let modal_w = (screen.width() - 56.0).clamp(1.0, 1000.0);
        let modal_h = (screen.height() - 56.0).clamp(1.0, 650.0);
        let sidebar_w = (modal_w * 0.25).clamp(100.0, 185.0);
        let pane_w = (modal_w - sidebar_w - 64.0).max(1.0);
        let body_h = (modal_h - 65.0).max(1.0);

        let rebinding_at_start = self.rebinding_action.is_some();
        let response = egui::Modal::new(ui.make_persistent_id("settings_modal_window"))
            .frame(self.modal_glass_frame(242))
            .backdrop_color(egui::Color32::from_black_alpha(115))
            .show(ui.ctx(), |ui| {
                    // Keep the dialog within the available window.
                    ui.set_min_size(egui::vec2(modal_w, modal_h));
                    ui.set_max_size(egui::vec2(modal_w, modal_h));

                    // Force a vertical layout; the parent may be horizontal.
                    ui.vertical(|ui| {
                        // --- HEADER ---
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("󰒓  Advanced Settings")
                                    .size(17.0)
                                    .strong(),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.button("󰅖 Close").clicked() {
                                        self.show_settings = false;
                                    }
                                },
                            );
                        });
                        ui.add_space(6.0);
                        ui.separator();
                        ui.add_space(6.0);

                        // Main body: left sidebar and scrollable right panel.
                        ui.horizontal_top(|ui| {
                            // 1. Left column: category navigation.
                            egui::Frame::NONE
                                .inner_margin(egui::Margin::same(8))
                                .show(ui, |ui| {
                                    ui.set_min_size(egui::vec2(sidebar_w, body_h));
                                    ui.set_max_size(egui::vec2(sidebar_w, body_h));

                                    ui.vertical(|ui| {
                                        let idle = &mut ui.visuals_mut().widgets.inactive;
                                        idle.bg_fill = egui::Color32::TRANSPARENT;
                                        idle.weak_bg_fill = egui::Color32::TRANSPARENT;
                                        idle.bg_stroke = egui::Stroke::NONE;

                                        let footer_h = 32.0;
                                        let gap = ui.spacing().item_spacing.y;
                                        let nav_h = (body_h - footer_h - gap).max(0.0);
                                        egui::ScrollArea::vertical()
                                            .id_salt("settings_navigation")
                                            .max_height(nav_h)
                                            .auto_shrink([false, false])
                                            .show(ui, |ui| {
                                                let tabs = [
                                                    (SettingsTab::AppearanceAndTheme, "󰔎  Appearance"),
                                                    (SettingsTab::StudyAndTools, "󰏫  Tools & Study"),
                                                    (SettingsTab::Workspace, "󰕮  Workspace"),
                                                    (SettingsTab::Shortcuts, "󰌌  Keybindings"),
                                                    (SettingsTab::SystemAndData, "󰚥  Export & PDF"),
                                                    (SettingsTab::Storage, "󰋊  Storage & Data"),
                                                    (SettingsTab::Dictionaries, "󰗊  Dictionaries"),
                                                ];

                                                for (tab, label) in tabs {
                                                    let btn = crate::theme::selected_button(
                                                        egui::RichText::new(label).size(13.0),
                                                        self.settings_tab == tab,
                                                        ui.visuals(),
                                                    )
                                                    .min_size(egui::vec2(sidebar_w - 16.0, 32.0));

                                                    if ui.add(btn).clicked() {
                                                        self.settings_tab = tab;
                                                    }
                                                    ui.add_space(4.0);
                                                }
                                            });

                                        let btn = crate::theme::selected_button(
                                            egui::RichText::new("󰋼  Credits").size(13.0),
                                            self.settings_tab == SettingsTab::Credits,
                                            ui.visuals(),
                                        )
                                        .min_size(egui::vec2(sidebar_w - 16.0, footer_h));
                                        if ui.add(btn).clicked() {
                                            self.settings_tab = SettingsTab::Credits;
                                        }
                                    });
                                });

                            ui.add_space(10.0);

                            // 2. Right column: selected category content.
                            egui::Frame::NONE
                                .inner_margin(egui::Margin::symmetric(6, 2))
                                .show(ui, |ui| {
                                    ui.set_min_size(egui::vec2(pane_w, body_h));
                                    ui.set_max_size(egui::vec2(pane_w, body_h));

                                    egui::ScrollArea::vertical()
                                        .id_salt("settings_right_pane_scroll")
                                        .max_height(body_h - 5.0)
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| {
                                            // Use an explicit vertical layout.
                                            ui.vertical(|ui| {
                                                ui.set_max_width(pane_w - 24.0);

                                                let mut settings_dirty = false;

                                                match self.settings_tab {
                                                    // ───────────── TOOLS & STUDY ─────────────
                                                    SettingsTab::StudyAndTools => {
                                                        settings_page_header(ui, "Tools & Study", "Annotation tools, active recall and flashcards.");

                                                        settings_section(ui, "Annotation toolbar", "Which tools are shown and how you reach them.", |ui| {
                                                            if settings_toggle(
                                                                ui,
                                                                &mut self.app_state.settings.tools_button,
                                                                "Show Annotation Tools button",
                                                                "Adds the annotation tools button to the topbar.",
                                                            )
                                                            .changed()
                                                            {
                                                                settings_dirty = true;
                                                            }
                                                            if settings_toggle(
                                                                ui,
                                                                &mut self.app_state.settings.radial_menu_enabled,
                                                                "Enable Quick Radial Tools Menu on canvas",
                                                                "Right-click the page, or use its shortcut, to open a circular menu of tools.",
                                                            )
                                                            .changed()
                                                            {
                                                                settings_dirty = true;
                                                            }
                                                            if settings_toggle(
                                                                ui,
                                                                &mut self.app_state.settings.reset_tool_on_bar_close,
                                                                "Switch to Cursor when closing the annotation bar",
                                                                "Closing the toolbar returns to the Cursor / Select tool instead of keeping the active tool.",
                                                            )
                                                            .changed()
                                                            {
                                                                settings_dirty = true;
                                                            }

                                                            settings_subheading(ui, "Toolbar position");
                                                            let mut layout = self.app_state.settings.tools_layout;
                                                            if settings_choice(
                                                                ui,
                                                                &mut layout,
                                                                &[
                                                                    (state::ToolsLayoutMode::SubTopbar, "󰕮  Horizontal"),
                                                                    (state::ToolsLayoutMode::VerticalRail, "󰍜  Vertical Rail"),
                                                                ],
                                                                150.0,
                                                            ) {
                                                                self.app_state.settings.tools_layout = layout;
                                                                settings_dirty = true;
                                                            }
                                                            ui.weak("Horizontal bar or vertical rail on the right edge.");
                                                            if layout == state::ToolsLayoutMode::SubTopbar {
                                                                settings_subheading(ui, "Horizontal toolbar edge");
                                                                let mut position = self.app_state.settings.annotation_bar_position;
                                                                if settings_choice(
                                                                    ui,
                                                                    &mut position,
                                                                    &[
                                                                        (None, "Follow topbar"),
                                                                        (Some(state::TopbarPosition::Top), "Top"),
                                                                        (Some(state::TopbarPosition::Bottom), "Bottom"),
                                                                    ],
                                                                    100.0,
                                                                ) {
                                                                    self.app_state.settings.annotation_bar_position = position;
                                                                    settings_dirty = true;
                                                                }
                                                                ui.weak("Choose an edge independently of the topbar. When both share an edge, the topbar stays outermost.");
                                                            }

                                                            settings_subheading(ui, "Drawing");
                                                            ui.horizontal(|ui| {
                                                                ui.label("Stroke Thickness:");
                                                                ui.add(egui::Slider::new(&mut self.stroke_width, 1.0..=18.0).suffix(" px"));
                                                            });
                                                        });

                                                        settings_section(ui, "Active recall", "Study by hiding and revealing your own material.", |ui| {
                                                            settings_subheading(ui, "Occlusion tape");
                                                            ui.weak("Hide parts of a page with tape, then reveal them to test yourself.");
                                                            ui.add_space(4.0);
                                                            ui.horizontal_wrapped(|ui| {
                                                                if ui
                                                                    .button("󰝤  Cover All Tapes (Quiz Mode)")
                                                                    .on_hover_text("Hide all revealed occlusion tapes across the PDF")
                                                                    .clicked()
                                                                {
                                                                    self.revealed_tapes.clear();
                                                                }

                                                                if ui
                                                                    .button("󰈈  Reveal All Tapes")
                                                                    .on_hover_text("Temporarily uncover all occlusion tapes in the document")
                                                                    .clicked()
                                                                {
                                                                    for (&p, list) in &self.annotations {
                                                                        for (idx, ann) in list.iter().enumerate() {
                                                                            if matches!(ann.kind, annotations::AnnotationKind::Tape) {
                                                                                self.revealed_tapes.insert((p, idx));
                                                                            }
                                                                        }
                                                                    }
                                                                }
                                                            });

                                                            settings_subheading(ui, "Flashcards");
                                                            if settings_toggle(
                                                                ui,
                                                                &mut self.app_state.settings.flash_button,
                                                                "Show Flashcards button",
                                                                "Adds the Flashcards button to the topbar.",
                                                            )
                                                            .changed()
                                                            {
                                                                settings_dirty = true;
                                                            }
                                                            if settings_toggle(
                                                                ui,
                                                                &mut self.app_state.settings.flash_notification_dot,
                                                                "Show flashcard notification dot when document has cards",
                                                                "Marks the Flashcards button when your notes contain flashcards.",
                                                            )
                                                            .changed()
                                                            {
                                                                settings_dirty = true;
                                                            }
                                                            ui.horizontal_wrapped(|ui| {
                                                                if ui.button("󰘸 Open Flashcards Modal").clicked() {
                                                                    self.toggle_flashcards_modal();
                                                                }

                                                                if ui.button("󰈇 Export Anki Deck (.tsv)").clicked() {
                                                                    self.export_anki_dialog(ui.ctx());
                                                                }
                                                            });
                                                        });
                                                    }

                                                    // ───────────── WORKSPACE ─────────────
                                                    SettingsTab::Workspace => {
                                                        settings_page_header(ui, "Workspace", "Panels, audio, and how pages and slideshows behave.");

                                                        settings_section(ui, "Panels", "What Lapis restores when you reopen a document.", |ui| {
                                                            if settings_toggle(
                                                                ui,
                                                                &mut self.app_state.settings.restore_show_notes,
                                                                "Remember Notes panel state on reopen",
                                                                "Reopen a document with the Notes panel open or closed, as you left it.",
                                                            )
                                                            .changed()
                                                            {
                                                                settings_dirty = true;
                                                            }
                                                            if settings_toggle(
                                                                ui,
                                                                &mut self.app_state.settings.restore_show_toc,
                                                                "Remember Outline panel state on reopen",
                                                                "Reopen a document with the Outline panel open or closed, as you left it.",
                                                            )
                                                            .changed()
                                                            {
                                                                settings_dirty = true;
                                                            }
                                                        });

                                                        settings_section(ui, "Notes panel", "Where your notes live and what they offer.", |ui| {
                                                            if settings_toggle(
                                                                ui,
                                                                &mut self.app_state.settings.bottom_notes,
                                                                "Allow Notes panel to fit under the page when window is in portrait",
                                                                "In a tall window the panel can sit under the page instead of beside it.",
                                                            )
                                                            .changed()
                                                            {
                                                                settings_dirty = true;
                                                            }
                                                            if settings_toggle(
                                                                ui,
                                                                &mut self.app_state.settings.sketch_enabled,
                                                                "Enable Sketch (per-page scratchpad tab in the Notes panel)",
                                                                "Turning it off hides the Sketch tab and leaves sketches out of exports. Your sketches are kept.",
                                                            )
                                                            .changed()
                                                            {
                                                                settings_dirty = true;
                                                                if !self.app_state.settings.sketch_enabled {
                                                                    self.finish_sketch_edit();
                                                                    if self.note_mode == NoteMode::Sketch {
                                                                        self.note_mode = NoteMode::Edit;
                                                                    }
                                                                }
                                                            }
                                                        });

                                                        settings_section(ui, "Audio", "Voice memos and lesson recordings.", |ui| {
                                                            if settings_toggle(
                                                                ui,
                                                                &mut self.app_state.settings.audio_notes,
                                                                "Enable Audio notes",
                                                                "Voice memos attached to pages.",
                                                            )
                                                            .changed()
                                                            {
                                                                settings_dirty = true;
                                                            }
                                                            if settings_toggle(
                                                                ui,
                                                                &mut self.app_state.settings.audio_dock,
                                                                "Enable Audio Lessons recording and Audio dock",
                                                                "Record or load a lesson and follow it with its transcript in the audio dock.",
                                                            )
                                                            .changed()
                                                            {
                                                                settings_dirty = true;
                                                            }
                                                        });

                                                        settings_section(ui, "Page scrolling", "How fast the page moves with the mouse. The same in every view mode.", |ui| {
                                                            ui.horizontal(|ui| {
                                                                ui.label("Mouse wheel speed:");
                                                                if ui
                                                                    .add(
                                                                        egui::Slider::new(
                                                                            &mut self.app_state.settings.wheel_scroll_speed,
                                                                            0.5..=5.0,
                                                                        )
                                                                        .step_by(0.1)
                                                                        .suffix("×"),
                                                                    )
                                                                    .changed()
                                                                {
                                                                    settings_dirty = true;
                                                                }
                                                            });
                                                            ui.weak("Distance moved per wheel notch or trackpad swipe. Ctrl + wheel still zooms.");
                                                            ui.add_space(6.0);
                                                            ui.horizontal(|ui| {
                                                                ui.label("Middle-button drag speed:");
                                                                if ui
                                                                    .add(
                                                                        egui::Slider::new(
                                                                            &mut self.app_state.settings.pan_scroll_speed,
                                                                            1.0..=8.0,
                                                                        )
                                                                        .step_by(0.1)
                                                                        .suffix("×"),
                                                                    )
                                                                    .changed()
                                                                {
                                                                    settings_dirty = true;
                                                                }
                                                            });
                                                            ui.weak("How far the page moves per pixel you drag with the middle button held down. 1× follows the pointer exactly.");
                                                            ui.add_space(6.0);
                                                            if ui.button("Restore scrolling defaults").clicked() {
                                                                self.app_state.settings.wheel_scroll_speed = state::DEFAULT_WHEEL_SCROLL_SPEED;
                                                                self.app_state.settings.pan_scroll_speed = state::DEFAULT_PAN_SCROLL_SPEED;
                                                                settings_dirty = true;
                                                            }
                                                        });

                                                        settings_section(ui, "Pages & presentation", "", |ui| {
                                                            settings_subheading(ui, "Default new page style");
                                                            ui.weak("Style of the blank pages you insert into a document.");
                                                            ui.add_space(4.0);
                                                            let mut page_style = self.app_state.settings.default_page_style;
                                                            if settings_choice(
                                                                ui,
                                                                &mut page_style,
                                                                &[
                                                                    (notes::BlankPageStyle::Ruled, "Ruled"),
                                                                    (notes::BlankPageStyle::Dotted, "Dotted"),
                                                                    (notes::BlankPageStyle::Grid, "Grid"),
                                                                    (notes::BlankPageStyle::Blank, "Blank"),
                                                                ],
                                                                80.0,
                                                            ) {
                                                                self.app_state.settings.default_page_style = page_style;
                                                                settings_dirty = true;
                                                            }

                                                            settings_subheading(ui, "Slideshow + Notes");
                                                            ui.weak("What the Notes panel does on slides without annotations.");
                                                            ui.add_space(4.0);
                                                            for (mode, label, hint) in [
                                                                (
                                                                    SlideshowEmptyMode::AutoHold,
                                                                    "Auto-Hold Previous Note",
                                                                    "Keeps the previous annotated slide's note visible.",
                                                                ),
                                                                (
                                                                    SlideshowEmptyMode::ContextDashboard,
                                                                    "Study Context Dashboard",
                                                                    "Shows the audio transcript and a preview of the next note.",
                                                                ),
                                                                (
                                                                    SlideshowEmptyMode::SkipEmpty,
                                                                    "Skip Unannotated Slides",
                                                                    "Arrow navigation jumps over slides without annotations.",
                                                                ),
                                                                (
                                                                    SlideshowEmptyMode::ExpandFull,
                                                                    "Auto-Expand Fullscreen",
                                                                    "Hides the Notes panel on empty slides so the slide fills the screen.",
                                                                ),
                                                                (
                                                                    SlideshowEmptyMode::StaticEmpty,
                                                                    "Show Empty-Slide Placeholder",
                                                                    "Keeps the panel in place and shows \"No notes for this slide.\"",
                                                                ),
                                                            ] {
                                                                if ui
                                                                    .radio_value(
                                                                        &mut self.app_state.settings.slideshow_empty_mode,
                                                                        mode,
                                                                        label,
                                                                    )
                                                                    .changed()
                                                                {
                                                                    settings_dirty = true;
                                                                }
                                                                settings_hint(ui, hint);
                                                            }
                                                        });
                                                    }

                                                    // ───────────── APPEARANCE ─────────────
                                                    SettingsTab::AppearanceAndTheme => {
                                                        settings_page_header(ui, "Appearance", "Theme, interface size, and text.");

                                                        settings_section(ui, "Theme", "Choose one theme source. Switching replaces the active palette.", |ui| {
                                                            let id = ui.make_persistent_id("theme_picker_state");
                                                            let mut picker = ui.data_mut(|d| d.get_temp::<theme_picker::ThemePicker>(id))
                                                                .unwrap_or_else(theme_picker::ThemePicker::load);
                                                            let action = picker.show(ui, &mut self.theme);
                                                            ui.data_mut(|d| d.insert_temp(id, picker));
                                                            match action {
                                                                Some(theme_picker::ThemeAction::Import) => self.import_theme_dialog(ui.ctx()),
                                                                Some(theme_picker::ThemeAction::Export) => self.export_theme_dialog(ui.ctx()),
                                                                None => {}
                                                            }
                                                        });

                                                        settings_section(ui, "Interface density", "Spacing, text and bar sizes across the whole app.", |ui| {
                                                            let mut density = self.app_state.settings.ui_density;
                                                            if settings_choice(
                                                                ui,
                                                                &mut density,
                                                                &[
                                                                    (state::UiDensity::Standard, "Standard"),
                                                                    (state::UiDensity::Compact, "Compact"),
                                                                    (state::UiDensity::Ultra, "Ultra-compact"),
                                                                ],
                                                                110.0,
                                                            ) {
                                                                self.app_state.settings.ui_density = density;
                                                                settings_dirty = true;
                                                            }
                                                        });

                                                        settings_section(ui, "Topbar", "Where the topbar sits and when it shows.", |ui| {
                                                            let mut position = self.app_state.settings.topbar_position;
                                                            if settings_choice(
                                                                ui,
                                                                &mut position,
                                                                &[
                                                                    (state::TopbarPosition::Top, "Top"),
                                                                    (state::TopbarPosition::Bottom, "Bottom"),
                                                                ],
                                                                110.0,
                                                            ) {
                                                                self.app_state.settings.topbar_position = position;
                                                                self.topbar_hold_until = 0.0;
                                                                self.topbar_overlay_h = 0.0;
                                                                settings_dirty = true;
                                                            }
                                                            ui.weak("Bottom puts the topbar on the bottom edge; its menus open upwards. The annotation toolbar has its own placement setting.");
                                                            ui.add_space(6.0);
                                                            if settings_toggle(
                                                                ui,
                                                                &mut self.app_state.settings.topbar_auto_hide,
                                                                "Auto-hide topbar",
                                                                "Hover the window edge the topbar sits on to show it. The auto-hide shortcut can be changed in Keybindings.",
                                                            )
                                                            .changed()
                                                            {
                                                                settings_dirty = true;
                                                            }
                                                        });

                                                        settings_section(ui, "PDF & notes text", "How pages are drawn and the font used for notes.", |ui| {
                                                            if settings_toggle(
                                                                ui,
                                                                &mut self.pdf_dark_mode,
                                                                "PDF Dark Mode",
                                                                "Inverts the slide background to a deep slate for reading at night.",
                                                            )
                                                            .changed()
                                                            {
                                                                self.viewer.clear_textures();
                                                                ui.ctx().request_repaint();
                                                            }

                                                            settings_subheading(ui, "Notes font size");
                                                            ui.horizontal(|ui| {
                                                                ui.label("Size:");
                                                                if ui
                                                                    .add(
                                                                        egui::Slider::new(
                                                                            &mut self.app_state.settings.note_font_size,
                                                                            11.0..=24.0,
                                                                        )
                                                                        .suffix(" pt"),
                                                                    )
                                                                    .changed()
                                                                {
                                                                    settings_dirty = true;
                                                                }
                                                            });

                                                            settings_subheading(ui, "Custom notes font");
                                                            let font_label = self
                                                                .app_state
                                                                .settings
                                                                .custom_font_path
                                                                .as_ref()
                                                                .and_then(|p| p.file_name())
                                                                .and_then(|s| s.to_str())
                                                                .unwrap_or("Default font");
                                                            ui.weak(format!("Active font: {}", font_label));
                                                            ui.add_space(4.0);
                                                            ui.horizontal_wrapped(|ui| {
                                                                if ui.button("󰛖  Load Font (.ttf/.otf)...").clicked()
                                                                    && let Some(path) = rfd::FileDialog::new()
                                                                        .add_filter("Font Files", &["ttf", "otf"])
                                                                        .pick_file()
                                                                {
                                                                    self.app_state.settings.custom_font_path = Some(path);
                                                                    self.apply_custom_font(ui.ctx());
                                                                    settings_dirty = true;
                                                                }

                                                                if self.app_state.settings.custom_font_path.is_some()
                                                                    && ui.button("Reset Font").clicked()
                                                                {
                                                                    self.app_state.settings.custom_font_path = None;
                                                                    self.apply_custom_font(ui.ctx());
                                                                    settings_dirty = true;
                                                                }
                                                            });
                                                        });
                                                    }

                                                    // ───────────── KEYBINDINGS ─────────────

                                                    SettingsTab::Shortcuts => {
                                                        settings_page_header(ui, "Keybindings", "Uncheck a shortcut to disable it without losing its binding. Click a key to rebind, or press two keys in sequence for a chord. Esc cancels.");

                                                        // Defaults and snapshots for reset and conflict detection.
                                                        let mut defaults_cfg = ShortcutsConfig::default();
                                                        let defaults: Vec<(&'static str, ShortcutBinding)> = shortcut_entries(&mut defaults_cfg)
                                                            .into_iter()
                                                            .map(|(l, b)| (l, b.clone()))
                                                            .collect();

                                                        let cfg = &mut self.app_state.shortcuts;
                                                        ui.horizontal_wrapped(|ui| {
                                                            for (label, enabled) in [("Enable all", true), ("Disable all", false)] {
                                                                if ui.button(label).clicked() {
                                                                    for (_, binding) in shortcut_entries(cfg) {
                                                                        binding.enabled = enabled;
                                                                    }
                                                                    self.rebinding_action = None;
                                                                    settings_dirty = true;
                                                                }
                                                            }
                                                        });
                                                        ui.add_space(6.0);
                                                        let snapshot: Vec<(&'static str, ShortcutBinding)> = shortcut_entries(cfg)
                                                            .into_iter()
                                                            .map(|(l, b)| (l, b.clone()))
                                                            .collect();

                                                        for (group, description, labels) in [
                                                            ("Document & navigation", "Save, search and move between pages.", &[
                                                                "Save notes", "Search in notes", "Previous slide (global)", "Next slide (global)"
                                                            ][..]),
                                                            ("View & zoom", "Change PDF view, zoom and appearance.", &[
                                                                "View: Single page", "View: Continuous", "View: Book spread", "View: Slideshow", "View: Slideshow + Notes", "Toggle Continuous / Single page", "Toggle fullscreen", "Fit Page", "Fit Width", "Fit PDF and notes layout", "PDF night mode"
                                                            ][..]),
                                                            ("Workspace", "Show or switch panels and toolbars.", &[
                                                                "Toggle notes panel", "Toggle edit / preview", "Toggle outline", "Toggle Sketch tab (notes panel)", "Toggle topbar auto-hide", "Toggle annotation toolbar", "Radial tools menu"
                                                            ][..]),
                                                            ("Annotation tools", "Choose tools without opening a toolbar.", &[
                                                                "Cursor / Select PDF Text", "Pen", "Highlighter", "Textbox", "Underline", "Eraser", "Rectangle", "Circle", "Line", "Arrow", "Snip", "Occlusion tape", "Trackpad draw (hold)"
                                                            ][..]),
                                                        ] {
                                                            settings_section(ui, group, description, |ui| {
                                                                for (label, binding) in shortcut_entries(cfg) {
                                                                    if !labels.contains(&label) { continue; }
                                                                    if self.rebinding_action.as_deref() == Some(label)
                                                                        && let Some(cap) = capture_shortcut(ui) {
                                                                            if let Capture::Set(nb) = cap {
                                                                                let enabled = binding.enabled;
                                                                                *binding = nb;
                                                                                binding.enabled = enabled;
                                                                                settings_dirty = true;
                                                                            }
                                                                            self.rebinding_action = None;
                                                                        }

                                                                    let is_rebinding = self.rebinding_action.as_deref() == Some(label);
                                                                    let default = defaults.iter().find(|(l, _)| *l == label).map(|(_, b)| b.clone());

                                                                    let conflict = snapshot
                                                                        .iter()
                                                                        .find(|(l, b)| *l != label && binding.enabled && b.enabled && b.conflicts_with(binding))
                                                                        .map(|(l, _)| *l);

                                                                    let text = if is_rebinding {
                                                                        "󰌌  Press keys…  (Esc to cancel)".to_string()
                                                                    } else {
                                                                        binding.format_label()
                                                                    };
                                                                    let label_w = ui.painter().layout_no_wrap(label.to_string(), egui::TextStyle::Body.resolve(ui.style()), ui.visuals().text_color()).size().x;
                                                                    let key_w = ui.painter().layout_no_wrap(text.clone(), egui::TextStyle::Monospace.resolve(ui.style()), ui.visuals().text_color()).size().x;
                                                                    let control_w = (key_w + 70.0).max(220.0);
                                                                    let inline = ui.available_width() >= label_w + control_w + 48.0;
                                                                    let mut row = |ui: &mut egui::Ui| {
                                                                        if ui.checkbox(&mut binding.enabled, label).on_hover_text("Enable this shortcut").changed() {
                                                                            settings_dirty = true;
                                                                            if !binding.enabled && is_rebinding {
                                                                                self.rebinding_action = None;
                                                                            }
                                                                        }
                                                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                                            if let Some(def) = &default {
                                                                                if ui.add_enabled(*binding != *def, egui::Button::new("󰑐").small())
                                                                                    .on_hover_text("Reset to default and enable").clicked()
                                                                                {
                                                                                    *binding = def.clone();
                                                                                    settings_dirty = true;
                                                                                    self.rebinding_action = None;
                                                                                }
                                                                            }
                                                                            let btn = crate::theme::selected_button(
                                                                                egui::RichText::new(&text).monospace(),
                                                                                is_rebinding,
                                                                                ui.visuals(),
                                                                            )
                                                                                .min_size(egui::vec2((control_w - 32.0).min(ui.available_width()).max(0.0), 24.0))
                                                                                .wrap();
                                                                            if ui.add(btn).clicked() {
                                                                                ui.data_mut(|d| d.remove::<PendingChord>(egui::Id::unique(PENDING_ID)));
                                                                                self.rebinding_action = if is_rebinding {
                                                                                    None
                                                                                } else {
                                                                                    Some(label.to_string())
                                                                                };
                                                                            }
                                                                        });
                                                                    };
                                                                    if inline {
                                                                        ui.horizontal(&mut row);
                                                                    } else {
                                                                        ui.vertical(&mut row);
                                                                    }

                                                                    if let Some(other) = conflict {
                                                                        ui.label(
                                                                            egui::RichText::new(format!("⚠ Conflicts with “{}”", other))
                                                                                .size(11.5)
                                                                                .color(egui::Color32::from_rgb(251, 191, 36)),
                                                                        );
                                                                    }
                                                                    ui.separator();
                                                                }
                                                            });
                                                        }

                                                        ui.add_space(8.0);
                                                        if ui.button("󰑐  Reset All Shortcuts to Default").clicked() {
                                                            *cfg = ShortcutsConfig::default();
                                                            self.rebinding_action = None;
                                                            settings_dirty = true;
                                                        }

                                                        ui.add_space(12.0);
                                                        settings_section(ui, "Fixed shortcuts", "Built-in controls, independent of the configurable bindings above.", |ui| {
                                                            for (name, keys) in [
                                                                ("Command Palette", "Ctrl + P / Ctrl + K"),
                                                                ("Undo annotations", "Ctrl + Z"),
                                                                ("Redo annotations", "Ctrl + Y / Ctrl + Shift + Z"),
                                                                ("Previous / next page", "Left / Right, K / J, PageUp / PageDown"),
                                                                ("Zoom in / out / reset", "Ctrl + Plus / Minus / 0"),
                                                                ("Exit slideshow", "Esc"),
                                                            ] {
                                                                let name_w = ui.painter().layout_no_wrap(name.to_string(), egui::TextStyle::Body.resolve(ui.style()), ui.visuals().text_color()).size().x;
                                                                let keys_w = ui.painter().layout_no_wrap(keys.to_string(), egui::TextStyle::Monospace.resolve(ui.style()), ui.visuals().text_color()).size().x;
                                                                if ui.available_width() >= name_w + keys_w + 24.0 {
                                                                    ui.horizontal(|ui| {
                                                                        ui.label(name);
                                                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                                            ui.label(egui::RichText::new(keys).monospace().weak());
                                                                        });
                                                                    });
                                                                } else {
                                                                    ui.label(name);
                                                                    ui.label(egui::RichText::new(keys).monospace().weak());
                                                                }
                                                                ui.add_space(3.0);
                                                            }
                                                        });
                                                    }


                                                    // ───────────── EXPORT & PDF ─────────────
                                                    SettingsTab::SystemAndData => {
                                                        settings_page_header(ui, "Export & PDF", "Save the current document and control how exported PDFs look.");

                                                        settings_section(ui, "Current document", "Save the open PDF.", |ui| {
                                                            ui.horizontal_wrapped(|ui| {
                                                                if ui.button("󰇚  Download Original PDF").clicked() {
                                                                    self.prompt_download_original_pdf(ui.ctx());
                                                                }
                                                                let export_label = if self.is_exporting {
                                                                    "󱑚  Exporting...".to_string()
                                                                } else {
                                                                    format!(
                                                                        "󰚥  Export PDF ({})",
                                                                        self.app_state.export_settings.content.short_label()
                                                                    )
                                                                };
                                                                if ui.button(export_label).clicked() && !self.is_exporting {
                                                                    self.start_export(ui.ctx());
                                                                }
                                                            });
                                                            ui.add_space(4.0);
                                                            ui.weak("Download Original PDF saves the file as it was before Lapis. Export PDF builds a new PDF with the options below.");
                                                        });

                                                        let mut export_changed = false;
                                                        settings_section(ui, "PDF export options", "Customize note typography and slide proportions in exported PDFs.", |ui| {
                                                            let cfg = &mut self.app_state.export_settings;

                                                            egui::Grid::new("export_settings_grid")
                                                                .num_columns(2)
                                                                .spacing([12.0, 10.0])
                                                                .show(ui, |ui| {
                                                                    ui.label("Notes Base Font Size:");

                                                                    if ui
                                                                        .add(
                                                                            egui::DragValue::new(&mut cfg.base_font_size)
                                                                                .range(12.0..=32.0)
                                                                                .speed(0.2)
                                                                                .suffix(" pt"),
                                                                        )
                                                                        .changed()
                                                                    {
                                                                        export_changed = true;
                                                                    }

                                                                    ui.end_row();

                                                                    ui.label("Export content:");

                                                                    ui.vertical(|ui| {
                                                                        let current = cfg.content;

                                                                        for content in crate::state::ExportContent::ALL {
                                                                            let btn = crate::theme::selected_button(content.label(), current == content, ui.visuals());

                                                                            if ui
                                                                                .add_sized([220.0, 28.0], btn)
                                                                                .clicked()
                                                                                && current != content
                                                                            {
                                                                                cfg.content = content;
                                                                                export_changed = true;
                                                                            }
                                                                        }
                                                                    });

                                                                    ui.end_row();

                                                                    ui.label("Slide Width on Split Pages:");

                                                                    if ui
                                                                        .add(
                                                                            egui::DragValue::new(&mut cfg.slide_width_pct)
                                                                                .range(40.0..=80.0)
                                                                                .speed(0.5)
                                                                                .suffix(" %"),
                                                                        )
                                                                        .changed()
                                                                    {
                                                                        export_changed = true;
                                                                    }

                                                                    ui.end_row();
                                                                });

                                                            ui.add_space(8.0);

                                                            if ui.button("󰑐  Restore Export Defaults").clicked() {
                                                                *cfg = state::ExportSettings::default();
                                                                export_changed = true;
                                                            }

                                                            if export_changed {
                                                                cfg.clamp_valid();
                                                            }
                                                        });
                                                        if export_changed {
                                                            settings_dirty = true;
                                                        }
                                                    }
                                                    SettingsTab::Storage => {
                                                        let cache_id = ui.make_persistent_id("storage_scan_cache");
                                                        let stretch_id = ui.make_persistent_id("storage_stretch_size");
                                                        let confirm_id = ui.make_persistent_id("storage_confirm");

                                                        // Scan on refresh or action, not on every frame.
                                                        let entries: Vec<StorageEntry> =
                                                            match ui.data(|d| d.get_temp::<Vec<StorageEntry>>(cache_id)) {
                                                                Some(v) => v,
                                                                None => {
                                                                    let v = scan_storage(&self.app_state.recent_files);
                                                                    let s = dir_size(&stretch_cache_dir());
                                                                    ui.data_mut(|d| {
                                                                        d.insert_temp(cache_id, v.clone());
                                                                        d.insert_temp(stretch_id, s);
                                                                    });
                                                                    v
                                                                }
                                                            };
                                                        let stretch_size: u64 = ui.data(|d| d.get_temp::<u64>(stretch_id)).unwrap_or(0);
                                                        let mut confirm: String = ui
                                                            .data(|d| d.get_temp::<String>(confirm_id))
                                                            .unwrap_or_default();
                                                        let mut action: Option<StorageAction> = None;

                                                        let open_canon = if self.has_pdf() {
                                                            dunce::canonicalize(&self.pdf_path).ok()
                                                        } else {
                                                            None
                                                        };

                                                        settings_page_header(ui, "Storage & Data", "What Lapis keeps on disk, and how to clean it up.");

                                                        settings_section(ui, "Stored documents", "Notes, audio and original-PDF copies per document, largest first.", |ui| {
                                                        let total: u64 = entries.iter().map(|e| e.total()).sum();
                                                        let audio_total: u64 = entries.iter().map(|e| e.audio_size).sum();
                                                        let orphan_count = entries
                                                            .iter()
                                                            .filter(|e| e.pdf_path.is_some() && !e.pdf_exists)
                                                            .count();
                                                        let orphan_bytes: u64 = entries
                                                            .iter()
                                                            .filter(|e| e.pdf_path.is_some() && !e.pdf_exists)
                                                            .map(|e| e.total())
                                                            .sum();

                                                        ui.horizontal_wrapped(|ui| {
                                                            ui.label(format!(
                                                                "{} document(s) · {} total · {} audio",
                                                                entries.len(),
                                                                human_size(total),
                                                                human_size(audio_total)
                                                            ));
                                                            if ui.button("󰑐 Refresh").clicked() {
                                                                ui.data_mut(|d| d.remove::<Vec<StorageEntry>>(cache_id));
                                                            }
                                                        });

                                                        if orphan_count > 0 {
                                                            ui.add_space(4.0);
                                                            ui.horizontal_wrapped(|ui| {
                                                                if danger_button(
                                                                    ui,
                                                                    &mut confirm,
                                                                    "orphans".to_string(),
                                                                    &format!(
                                                                        "󰆴 Clean {} orphan(s) · {}",
                                                                        orphan_count,
                                                                        human_size(orphan_bytes)
                                                                    ),
                                                                    "Delete notes, audio and original copies of documents whose PDF no longer exists",
                                                                    true,
                                                                ) {
                                                                    action = Some(StorageAction::CleanOrphans);
                                                                }
                                                            });
                                                        }

                                                        ui.add_space(8.0);
                                                        ui.separator();
                                                        ui.add_space(6.0);

                                                        if entries.is_empty() {
                                                            ui.weak("No stored document data found.");
                                                        }

                                                        for (i, en) in entries.iter().enumerate() {
                                                            let is_open = en.pdf_path.is_some() && en.pdf_path == open_canon;

                                                            egui::Frame::NONE
                                                                .fill(egui::Color32::from_black_alpha(35))
                                                                .corner_radius(egui::CornerRadius::same(6))
                                                                .inner_margin(egui::Margin::same(8))
                                                                .show(ui, |ui| {
                                                                    ui.vertical(|ui| {
                                                                        ui.set_width(ui.available_width());

                                                                        // Nome + dimensione totale
                                                                        ui.horizontal(|ui| {
                                                                            ui.with_layout(
                                                                                egui::Layout::right_to_left(egui::Align::Center),
                                                                                |ui| {
                                                                                    ui.label(
                                                                                        egui::RichText::new(human_size(en.total())).strong(),
                                                                                    );
                                                                                    ui.with_layout(
                                                                                        egui::Layout::left_to_right(egui::Align::Center),
                                                                                        |ui| {
                                                                                            ui.add(
                                                                                                egui::Label::new(
                                                                                                    egui::RichText::new(&en.name).strong(),
                                                                                                )
                                                                                                .truncate(),
                                                                                            );
                                                                                        },
                                                                                    );
                                                                                },
                                                                            );
                                                                        });

                                                                        // Path and state.
                                                                        match (&en.pdf_path, en.pdf_exists) {
                                                                            (Some(p), true) => {
                                                                                ui.add(
                                                                                    egui::Label::new(
                                                                                        egui::RichText::new(p.display().to_string())
                                                                                            .size(11.0)
                                                                                            .weak(),
                                                                                    )
                                                                                    .truncate(),
                                                                                );
                                                                            }
                                                                            (Some(p), false) => {
                                                                                ui.add(
                                                                                    egui::Label::new(
                                                                                        egui::RichText::new(format!(
                                                                                            "⚠ PDF not found: {}",
                                                                                            p.display()
                                                                                        ))
                                                                                        .size(11.0)
                                                                                        .color(egui::Color32::from_rgb(251, 191, 36)),
                                                                                    )
                                                                                    .truncate(),
                                                                                );
                                                                            }
                                                                            (None, _) => {
                                                                                ui.label(
                                                                                    egui::RichText::new(
                                                                                        "⚠ Source PDF unknown (legacy data, re-open the PDF once to link it)",
                                                                                    )
                                                                                    .size(11.0)
                                                                                    .color(egui::Color32::from_rgb(251, 191, 36)),
                                                                                );
                                                                            }
                                                                        }

                                                                        // Dettagli
                                                                        let mut detail = format!(
                                                                            "{} note page(s) · {} annotation(s) · backup {}",
                                                                            en.notes_count,
                                                                            en.ann_count,
                                                                            human_size(en.backup_size)
                                                                        );
                                                                        if en.orig_size > 0 {
                                                                            detail.push_str(&format!(
                                                                                " · original copy {}",
                                                                                human_size(en.orig_size)
                                                                            ));
                                                                        }
                                                                        ui.weak(detail);
                                                                        if en.audio_size > 0 {
                                                                            ui.weak(format!(
                                                                                "audio {} · lecture {} · {} voice memo(s) {}",
                                                                                human_size(en.audio_size),
                                                                                human_size(en.lecture_size),
                                                                                en.memo_count,
                                                                                human_size(en.memos_size)
                                                                            ));
                                                                        }
                                                                        ui.add_space(4.0);

                                                                        // Azioni
                                                                        ui.horizontal_wrapped(|ui| {
                                                                            if en.pdf_exists {
                                                                                let busy = "Close this PDF first";

                                                                                if danger_button(
                                                                                    ui,
                                                                                    &mut confirm,
                                                                                    format!("clear:{}", en.id),
                                                                                    "󰆴 Notes",
                                                                                    if is_open {
                                                                                        busy
                                                                                    } else {
                                                                                        "Removes notes & annotations from the backup AND from the data appended inside the PDF. Cannot be undone."
                                                                                    },
                                                                                    en.has_notes() && !is_open,
                                                                                ) {
                                                                                    action = Some(StorageAction::ClearNotes(i));
                                                                                }

                                                                                if en.lecture_size > 0
                                                                                    && danger_button(
                                                                                        ui,
                                                                                        &mut confirm,
                                                                                        format!("lec:{}", en.id),
                                                                                        "󰆴 Lecture audio",
                                                                                        if is_open {
                                                                                            busy
                                                                                        } else {
                                                                                            "Deletes the lecture recording with its transcript, slide markers and waveform. Voice memos are kept."
                                                                                        },
                                                                                        !is_open,
                                                                                    )
                                                                                {
                                                                                    action = Some(StorageAction::DeleteLecture(i));
                                                                                }

                                                                                if en.memo_count > 0
                                                                                    && danger_button(
                                                                                        ui,
                                                                                        &mut confirm,
                                                                                        format!("memo:{}", en.id),
                                                                                        "󰆴 Voice memos",
                                                                                        if is_open {
                                                                                            busy
                                                                                        } else {
                                                                                            "Deletes every voice memo and its transcript. The lecture recording is kept."
                                                                                        },
                                                                                        !is_open,
                                                                                    )
                                                                                {
                                                                                    action = Some(StorageAction::DeleteMemos(i));
                                                                                }

                                                                                if en.orig_size > 0
                                                                                    && danger_button(
                                                                                        ui,
                                                                                        &mut confirm,
                                                                                        format!("orig:{}", en.id),
                                                                                        "󰆴 Original copy",
                                                                                        "Deletes the hidden .orig copy. 'Download Original PDF' will then return the current (modified) PDF.",
                                                                                        true,
                                                                                    )
                                                                                {
                                                                                    action = Some(StorageAction::DeleteOrig(i));
                                                                                }
                                                                            } else if danger_button(
                                                                                ui,
                                                                                &mut confirm,
                                                                                format!("left:{}", en.id),
                                                                                "󰆴 Delete leftovers",
                                                                                "Deletes the notes backup, the audio and the .orig copy of this document",
                                                                                true,
                                                                            ) {
                                                                                action = Some(StorageAction::DeleteLeftovers(i));
                                                                            }
                                                                        });
                                                                    });
                                                                });
                                                            ui.add_space(6.0);
                                                        }

                                                        });

                                                        settings_section(ui, "PDF revision history", "Rebuild the current PDF to remove historical Lapis incremental updates. Keeps a copy of the original alongside the PDF. Signed and encrypted PDFs are not supported.", |ui| {
                                                        if ui.add_enabled(self.has_pdf() && self.pdf_save_job.is_none() && self.queued_pdf_save.is_none() && self.pdf_save_failure.is_none(), egui::Button::new("󰐻  Compact Lapis PDF history..."))
                                                            .on_hover_text("Compact the current PDF (may rewrite external PDF revision history)")
                                                            .clicked()
                                                        {
                                                            self.finish_sketch_edit();
                                                            self.sync_and_save();
                                                            self.save_session_state();
                                                            let result = AudioDocIndex::recover_page_change(&self.pdf_path).and_then(|()| notes::compact_doc_data(
                                                                &self.pdf_path, &self.notes, &self.annotations,
                                                                self.blank_pages.clone(), &self.sketches,
                                                            ));
                                                            self.toast_message = Some((
                                                                match result {
                                                                    Ok(()) => "󰄬 PDF history compacted (backup preserved)".to_string(),
                                                                    Err(err) if notes::is_external_modification(&err) => {
                                                                        match self.merge_external_into_model() {
                                                                            Ok(()) => "PDF changed outside Lapis; its annotations were merged. Compact again to continue".to_string(),
                                                                            Err(err) => format!("PDF compaction failed: {err}"),
                                                                        }
                                                                    }
                                                                    Err(err) => format!("PDF compaction failed: {err}"),
                                                                },
                                                                std::time::Instant::now(),
                                                            ));
                                                        }
                                                        });

                                                        settings_section(ui, "Temporary cache", &format!("Time-stretched audio for playback speeds other than 1x: {}. Re-created on demand.", human_size(stretch_size)), |ui| {
ui.horizontal_wrapped(|ui| {
                                                            if danger_button(
                                                                ui,
                                                                &mut confirm,
                                                                "stretch".to_string(),
                                                                "󰆴 Clear playback-speed cache",
                                                                "Safe: files are regenerated the next time you play at that speed",
                                                                stretch_size > 0,
                                                            ) {
                                                                action = Some(StorageAction::ClearStretchCache);
                                                            }
                                                        });
                                                        });

                                                        settings_section(ui, "App state", "", |ui| {
let state_path = crate::state::AppState::config_path();
                                                        let state_size = std::fs::metadata(&state_path).map(|m| m.len()).unwrap_or(0);
                                                        ui.add(
                                                            egui::Label::new(
                                                                egui::RichText::new(format!(
                                                                    "{}  ·  {}",
                                                                    state_path.display(),
                                                                    human_size(state_size)
                                                                ))
                                                                .size(11.0)
                                                                .weak(),
                                                            )
                                                            .truncate(),
                                                        );
                                                        ui.weak(format!(
                                                            "{} recent file(s) with saved page, zoom and bookmarks.",
                                                            self.app_state.recent_files.len()
                                                        ));
                                                        ui.add_space(4.0);
                                                        ui.horizontal_wrapped(|ui| {
                                                            if danger_button(
                                                                ui,
                                                                &mut confirm,
                                                                "recent".to_string(),
                                                                "󰆴 Clear recent files list",
                                                                "Also forgets last page, zoom and bookmarks of every recent file. Notes and audio are not touched.",
                                                                !self.app_state.recent_files.is_empty(),
                                                            ) {
                                                                action = Some(StorageAction::ClearRecent);
                                                            }
                                                        });

                                                        ui.add_space(4.0);
                                                        ui.weak(format!(
                                                            "Notes folder: {}",
                                                            notes::get_backup_dir().display()
                                                        ));
                                                        ui.weak(format!(
                                                            "Audio folder: {}",
                                                            audio::AudioDocIndex::media_root().display()
                                                        ));
                                                        });

                                                        // ───────── Esecuzione azione ─────────
                                                        if let Some(act) = action {
                                                            let report = |what: &str, name: &str, r: std::io::Result<()>| -> String {
                                                                match r {
                                                                    Ok(_) => format!("󰄬 {} {}", what, name),
                                                                    Err(e) => format!("󰅚 Error: {}", e),
                                                                }
                                                            };

                                                            let msg = match act {
                                                                StorageAction::ClearNotes(i) => report(
                                                                    "Cleared notes of",
                                                                    &entries[i].name,
                                                                    storage_clear_notes(&entries[i]),
                                                                ),
                                                                StorageAction::DeleteOrig(i) => report(
                                                                    "Deleted original copy of",
                                                                    &entries[i].name,
                                                                    storage_delete_orig(&entries[i]),
                                                                ),
                                                                StorageAction::DeleteLecture(i) => {
                                                                    if entries[i].pdf_path.as_ref() == Some(&self.pdf_path) {
                                                                        self.audio_engine.stop_and_release();
                                                                    }
                                                                    report(
                                                                        "Deleted lecture audio of",
                                                                        &entries[i].name,
                                                                        storage_delete_lecture(&entries[i]),
                                                                    )
                                                                }
                                                                StorageAction::DeleteMemos(i) => {
                                                                    if entries[i].pdf_path.as_ref() == Some(&self.pdf_path) {
                                                                        self.memo_audio_engine.stop_and_release();
                                                                    }
                                                                    report(
                                                                        "Deleted voice memos of",
                                                                        &entries[i].name,
                                                                        storage_delete_memos(&entries[i]),
                                                                    )
                                                                }
                                                                StorageAction::DeleteLeftovers(i) => {
                                                                    if entries[i].pdf_path.as_ref() == Some(&self.pdf_path) {
                                                                        self.audio_engine.stop_and_release();
                                                                        self.memo_audio_engine.stop_and_release();
                                                                    }
                                                                    report(
                                                                        "Deleted leftovers of",
                                                                        &entries[i].name,
                                                                        storage_delete_leftovers(&entries[i]),
                                                                    )
                                                                }
                                                                StorageAction::CleanOrphans => {
                                                                    let mut ok = 0usize;
                                                                    let mut freed = 0u64;
                                                                    for en in entries
                                                                        .iter()
                                                                        .filter(|e| e.pdf_path.is_some() && !e.pdf_exists)
                                                                    {
                                                                        if storage_delete_leftovers(en).is_ok() {
                                                                            ok += 1;
                                                                            freed += en.total();
                                                                        }
                                                                    }
                                                                    format!("󰄬 Cleaned {} orphan(s), freed {}", ok, human_size(freed))
                                                                }
                                                                StorageAction::ClearStretchCache => {
                                                                    let freed = stretch_size;
                                                                    match std::fs::remove_dir_all(stretch_cache_dir()) {
                                                                        Ok(_) => format!("󰄬 Cleared cache, freed {}", human_size(freed)),
                                                                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                                                                            "󰄬 Cache already empty".to_string()
                                                                        }
                                                                        Err(e) => format!("󰅚 Error: {}", e),
                                                                    }
                                                                }
                                                                StorageAction::ClearRecent => {
                                                                    self.app_state.recent_files.clear();
                                                                    settings_dirty = true;
                                                                    "󰄬 Recent files list cleared".to_string()
                                                                }
                                                            };
                                                            self.toast_message = Some((msg, std::time::Instant::now()));
                                                            ui.data_mut(|d| d.remove::<Vec<StorageEntry>>(cache_id)); // forza nuova scansione
                                                        }

                                                        ui.data_mut(|d| d.insert_temp(confirm_id, confirm));
                                                    }
                                                    SettingsTab::Credits => {
                                                        settings_page_header(ui, "Credits", "About Lapis, project links and acknowledgements.");
                                                        settings_section(ui, "Lapis", "Advanced PDF Study & Lecture Workspace", |ui| {
                                                            ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                                                            ui.label(format!("License: {}", env!("CARGO_PKG_LICENSE")));
                                                            ui.weak("Copyright © 2026 Enrico Di Maria");
                                                        });
                                                        settings_section(ui, "Project links", "", |ui| {
                                                            ui.hyperlink_to("Source code on GitHub", "https://github.com/EnricoDiMaria/lapis");
                                                            ui.hyperlink_to("Releases", "https://github.com/EnricoDiMaria/lapis/releases");
                                                            ui.hyperlink_to("Report an issue", "https://github.com/EnricoDiMaria/lapis/issues");
                                                            ui.hyperlink_to("User Guide", "https://github.com/EnricoDiMaria/lapis/blob/HEAD/docs/USER_GUIDE.md");
                                                            ui.hyperlink_to("Full license text", "https://github.com/EnricoDiMaria/lapis/blob/HEAD/LICENSE");
                                                        });
                                                        settings_section(ui, "Built with", "Selected open-source technologies used by Lapis.", |ui| {
                                                            for (name, url, description) in [
                                                                ("Rust", "https://www.rust-lang.org/", "Programming language"),
                                                                ("egui / eframe", "https://github.com/emilk/egui", "Application interface"),
                                                                ("MuPDF", "https://mupdf.com/", "PDF rendering"),
                                                                ("lopdf", "https://crates.io/crates/lopdf", "PDF editing"),
                                                                ("egui_commonmark", "https://crates.io/crates/egui_commonmark", "Markdown preview"),
                                                                ("CPAL / Rodio", "https://github.com/RustAudio", "Audio capture and playback"),
                                                                ("parakeet-rs", "https://crates.io/crates/parakeet-rs", "Offline transcription"),
                                                                ("spellbook", "https://crates.io/crates/spellbook", "Spellchecking"),
                                                            ] {
                                                                ui.horizontal_wrapped(|ui| {
                                                                    ui.hyperlink_to(name, url);
                                                                    ui.weak(description);
                                                                });
                                                            }
                                                            ui.hyperlink_to("Full dependency manifest", "https://github.com/EnricoDiMaria/lapis/blob/HEAD/Cargo.toml");
                                                            ui.weak("Third-party components retain their respective licenses.");
                                                        });
                                                    }
                                                    SettingsTab::Dictionaries => {
                                                        let confirm_id = ui.make_persistent_id("dict_confirm");
                                                        let words_cache_id = ui.make_persistent_id("dict_user_words_cache");
                                                        let input_id = ui.make_persistent_id("dict_new_word_input");
                                                        let mut confirm: String = ui
                                                            .data(|d| d.get_temp::<String>(confirm_id))
                                                            .unwrap_or_default();

                                                        settings_page_header(ui, "Dictionaries", "Enable any combination of installed Hunspell language pairs. Disabling a language keeps its files.");

                                                        settings_section(ui, "Spellcheck", "", |ui| {
                                                        if settings_toggle(
                                                                ui,
                                                                &mut self.app_state.settings.spellcheck_enabled,
                                                                "Enable offline spellcheck in Markdown notes",
                                                                "Underlines unknown words in the notes editor; right-click one for suggestions.",
                                                            )
                                                            .changed()
                                                        {
                                                            self.spellchecker.enabled = self.app_state.settings.spellcheck_enabled;
                                                            settings_dirty = true;
                                                        }

                                                        ui.add_space(6.0);
                                                        ui.weak(format!(
                                                            "{} active dictionaries · {}",
                                                            self.spellchecker.loaded_languages.len(),
                                                            self.spellchecker.loaded_languages.join(", ")
                                                        ));
                                                        });

                                                        settings_section(ui, "Installed dictionaries", "Languages used to check and suggest words in your notes.", |ui| {
                                                        egui::Frame::NONE
                                                            .fill(egui::Color32::from_black_alpha(35))
                                                            .corner_radius(egui::CornerRadius::same(6))
                                                            .inner_margin(egui::Margin::same(8))
                                                            .show(ui, |ui| {
                                                                ui.set_width(ui.available_width());
                                                                if ui.checkbox(
                                                                    &mut self.app_state.settings.english_dictionary_enabled,
                                                                    "English (built-in) · EN",
                                                                ).changed() {
                                                                    self.rebuild_spellchecker();
                                                                    settings_dirty = true;
                                                                }
                                                                ui.weak("Bundled en_US.aff + en_US.dic · retained when disabled");
                                                            });
                                                        ui.add_space(4.0);

                                                        let extras = self.app_state.settings.extra_dictionaries.clone();
                                                        let mut remove_idx: Option<usize> = None;

                                                        for (i, p) in extras.iter().enumerate() {
                                                            let is_pair = p.extension().and_then(|e| e.to_str()) == Some("aff");
                                                            let dic_path = p.with_extension("dic");
                                                            let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
                                                                + if is_pair { std::fs::metadata(&dic_path).map(|m| m.len()).unwrap_or(0) } else { 0 };
                                                            let missing = !p.is_file() || (is_pair && !dic_path.is_file());
                                                            let name = p
                                                                .file_stem()
                                                                .map(|s| s.to_string_lossy().to_string())
                                                                .unwrap_or_else(|| p.display().to_string());

                                                            egui::Frame::NONE
                                                                .fill(egui::Color32::from_black_alpha(35))
                                                                .corner_radius(egui::CornerRadius::same(6))
                                                                .inner_margin(egui::Margin::same(8))
                                                                .show(ui, |ui| {
                                                                    ui.vertical(|ui| {
                                                                        ui.set_width(ui.available_width());
                                                                        ui.horizontal(|ui| {
                                                                            ui.with_layout(
                                                                                egui::Layout::right_to_left(egui::Align::Center),
                                                                                |ui| {
                                                                                    ui.label(if missing {
                                                                                        "missing".to_string()
                                                                                    } else {
                                                                                        human_size(size)
                                                                                    });
                                                                                    ui.with_layout(
                                                                                        egui::Layout::left_to_right(egui::Align::Center),
                                                                                        |ui| {
                                                                                            ui.add(
                                                                                                egui::Label::new(egui::RichText::new(&name).strong())
                                                                                                    .truncate(),
                                                                                            );
                                                                                        },
                                                                                    );
                                                                                },
                                                                            );
                                                                        });
                                                                        let mut active = !self.app_state.settings.disabled_dictionaries.contains(p);
                                                                        if ui.checkbox(&mut active, "Enable").changed() {
                                                                            if active {
                                                                                self.app_state.settings.disabled_dictionaries.retain(|item| item != p);
                                                                            } else {
                                                                                self.app_state.settings.disabled_dictionaries.push(p.clone());
                                                                            }
                                                                            self.rebuild_spellchecker();
                                                                            settings_dirty = true;
                                                                        }
                                                                        ui.label(egui::RichText::new(format!(".aff / dictionary: {}", p.display())).size(11.0).weak());
                                                                        if is_pair {
                                                                            ui.label(egui::RichText::new(format!(".dic / word list: {}", dic_path.display())).size(11.0).weak());
                                                                        }
                                                                        if missing {
                                                                            ui.label(
                                                                                egui::RichText::new("⚠ Missing .dic or .aff file — use Add dictionary to complete this pair")
                                                                                    .size(11.0)
                                                                                    .color(egui::Color32::from_rgb(251, 191, 36)),
                                                                            );
                                                                        }
                                                                        ui.add_space(2.0);
                                                                        ui.horizontal_wrapped(|ui| {
                                                                            if danger_button(
                                                                                ui,
                                                                                &mut confirm,
                                                                                format!("dict:{}", p.display()),
                                                                                "󰆴 Remove",
                                                                                "Removes it from the list. If the copy lives in the app's dictionaries folder it is deleted too.",
                                                                                true,
                                                                            ) {
                                                                                remove_idx = Some(i);
                                                                            }
                                                                        });
                                                                    });
                                                                });
                                                            ui.add_space(4.0);
                                                        }

                                                        if extras.is_empty() {
                                                            ui.weak("No extra dictionaries yet.");
                                                        }

                                                        ui.add_space(4.0);
                                                        if ui.button("󰐕  Add dictionary...").clicked() {
                                                            self.dialogs.open(
                                                                ui.ctx(),
                                                                DialogKind::AddDictionary,
                                                                DialogSpec::open_files()
                                                                    .title("Select both .dic and .aff (or add a missing partner)")
                                                                    .filter("Hunspell dictionaries", &["dic", "aff", "txt"]),
                                                            );
                                                        }

                                                        if let Some(i) = remove_idx {
                                                            let p = self.app_state.settings.extra_dictionaries.remove(i);
                                                            self.app_state.settings.disabled_dictionaries.retain(|item| item != &p);
                                                            // Delete only app-owned copies, never external files.
                                                            if p.starts_with(spellcheck::dict_dir()) {
                                                                let _ = std::fs::remove_file(&p);
                                                                if p.extension().and_then(|e| e.to_str()) == Some("aff") {
                                                                    let _ = std::fs::remove_file(p.with_extension("dic"));
                                                                }
                                                            }
                                                            self.rebuild_spellchecker();
                                                            settings_dirty = true;
                                                            self.toast_message = Some((
                                                                "󰄬 Dictionary removed".to_string(),
                                                                std::time::Instant::now(),
                                                            ));
                                                        }

                                                        });

                                                        settings_section(ui, "Custom words", "Words you added with “Add to dictionary” in the notes editor.", |ui| {
                                                        let user_words: Vec<String> = match ui.data(|d| d.get_temp::<Vec<String>>(words_cache_id)) {
                                                            Some(v) => v,
                                                            None => {
                                                                let v = spellcheck::read_user_words();
                                                                ui.data_mut(|d| d.insert_temp(words_cache_id, v.clone()));
                                                                v
                                                            }
                                                        };

                                                        let mut new_word: String = ui
                                                            .data(|d| d.get_temp::<String>(input_id))
                                                            .unwrap_or_default();
                                                        ui.horizontal(|ui| {
                                                            let te = ui.add(
                                                                egui::TextEdit::singleline(&mut new_word)
                                                                    .hint_text("Add a word...")
                                                                    .desired_width(200.0),
                                                            );
                                                            let enter = te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                                                            if (ui.button("Add").clicked() || enter) && !new_word.trim().is_empty() {
                                                                self.spellchecker.add_user_word(&new_word);
                                                                new_word.clear();
                                                                ui.data_mut(|d| d.remove::<Vec<String>>(words_cache_id));
                                                            }
                                                        });
                                                        ui.data_mut(|d| d.insert_temp(input_id, new_word));

                                                        ui.add_space(4.0);
                                                        if user_words.is_empty() {
                                                            ui.weak("No custom words.");
                                                        } else {
                                                            ui.weak(format!("{} word(s) — click one to remove it", user_words.len()));
                                                            ui.add_space(2.0);

                                                            let mut remove_word: Option<String> = None;
                                                            ui.horizontal_wrapped(|ui| {
                                                                for w in user_words.iter().take(300) {
                                                                    if ui
                                                                        .small_button(format!("{} ×", w))
                                                                        .on_hover_text("Remove from dictionary")
                                                                        .clicked()
                                                                    {
                                                                        remove_word = Some(w.clone());
                                                                    }
                                                                }
                                                            });
                                                            if user_words.len() > 300 {
                                                                ui.weak(format!("…and {} more", user_words.len() - 300));
                                                            }

                                                            if let Some(w) = remove_word {
                                                                let rest: Vec<String> = user_words.iter().filter(|x| **x != w).cloned().collect();
                                                                let _ = spellcheck::write_user_words(&rest);
                                                                self.rebuild_spellchecker();
                                                                ui.data_mut(|d| d.remove::<Vec<String>>(words_cache_id));
                                                            }

                                                            ui.add_space(4.0);
                                                            ui.horizontal_wrapped(|ui| {
                                                                if danger_button(
                                                                    ui,
                                                                    &mut confirm,
                                                                    "user_words".to_string(),
                                                                    "󰆴 Clear all custom words",
                                                                    "Deletes your personal word list",
                                                                    true,
                                                                ) {
                                                                    let _ = spellcheck::write_user_words(&[]);
                                                                    self.rebuild_spellchecker();
                                                                    ui.data_mut(|d| d.remove::<Vec<String>>(words_cache_id));
                                                                }
                                                            });
                                                        }

                                                        });

                                                        ui.data_mut(|d| d.insert_temp(confirm_id, confirm));
                                                    }

                                                }

                                                if settings_dirty {
                                                    self.app_state.save();
                                                }
                                            }); // End right-panel vertical layout.
                                        });
                                });
                        });
                    }); // End modal vertical layout.
            });
        if !rebinding_at_start && self.rebinding_action.is_none() && response.should_close() {
            self.show_settings = false;
        }
    }
}

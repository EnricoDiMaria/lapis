//! Topbar, annotation controls, and tool popups.

use super::super::*;

/// Where the topbar is drawn this frame.
pub(in crate::workspace) enum TopbarSlot {
    /// In the layout, pushing the page down.
    Docked,
    /// Auto-hide: floating over the page while revealed.
    Overlay,
    Hidden,
}

impl Lapis {
    /// Decides where the topbar goes. With auto-hide on it floats over the page (never pushing
    /// it down) while the pointer is at the top edge, or over the bar, a menu of the bar is
    /// open or one of its fields is edited; it hides 0.4 s later. A pinned bar stays docked.
    pub(in crate::workspace) fn topbar_slot(&mut self, ui: &egui::Ui) -> TopbarSlot {
        if !self.app_state.settings.topbar_auto_hide {
            return TopbarSlot::Docked;
        }
        let ctx = ui.ctx();
        let screen = ctx.content_rect();
        let now = ctx.input(|i| i.time);
        let (pointer, button_down) = ctx.input(|i| (i.pointer.hover_pos(), i.pointer.any_down()));
        let was_shown = self.topbar_overlay_h > 0.0;
        let at_bottom = self.app_state.settings.topbar_position == state::TopbarPosition::Bottom;
        // Distance from the window edge the bar sits on.
        let near_edge = |reach: f32| {
            pointer.is_some_and(|p| {
                if at_bottom {
                    p.y >= screen.bottom() - reach
                } else {
                    p.y <= screen.top() + reach
                }
            })
        };
        // A press elsewhere (drawing, dragging) must not pop the bar open.
        let at_edge = near_edge(5.0) && !button_down;
        // Keep the revealed bar open while using a same-edge annotation toolbar.
        let settings = &self.app_state.settings;
        let shared_edge = self.show_tools_rail
            && settings.tools_layout == state::ToolsLayoutMode::SubTopbar
            && settings.annotation_bar_edge() == settings.topbar_position;
        let reach = self.topbar_overlay_h + if shared_edge { self.subbar_h } else { 0.0 } + 4.0;
        let over_bar = was_shown && near_edge(reach);
        // Menus and popups opened from the bar keep it visible while they are open: egui menus
        // (zoom, overflow), the file and bookmark popups, and the search popup.
        // `Popup::is_any_open` reads egui's memory, so it already works before the bar is drawn
        // (`Context::any_popup_open` only sees popups shown earlier in the same frame).
        let menu_open = was_shown
            && (egui::Popup::is_any_open(ctx) || self.topbar_popup_open || self.show_search);
        let editing = was_shown
            && ctx
                .memory(|m| m.focused())
                .and_then(|id| ctx.read_response(id))
                .is_some_and(|r| {
                    if at_bottom {
                        r.rect.top() >= screen.bottom() - reach + 2.0
                    } else {
                        r.rect.bottom() <= screen.top() + reach - 2.0
                    }
                });
        if at_edge || over_bar || menu_open || editing {
            self.topbar_hold_until = now + 0.4;
        }
        if now >= self.topbar_hold_until {
            return TopbarSlot::Hidden;
        }
        // Wake up again to hide the bar once the grace period ends.
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        TopbarSlot::Overlay
    }

    /// Shortcut: turns topbar auto-hide on or off (saved, like the setting).
    pub(in crate::workspace) fn toggle_topbar(&mut self) {
        let settings = &mut self.app_state.settings;
        settings.topbar_auto_hide = !settings.topbar_auto_hide;
        self.app_state.save();
        // Start from a clean state: hidden when auto-hide was just turned on.
        self.topbar_hold_until = 0.0;
        self.topbar_overlay_h = 0.0;
    }

    /// Placement of a popup opened from a topbar button: under it, or above it when the bar is
    /// at the bottom of the window. `centered` centres it on the button, otherwise it starts at
    /// the button's left edge.
    pub(in crate::workspace) fn topbar_popup_placement(
        &self,
        button: egui::Rect,
        centered: bool,
    ) -> (egui::Pos2, egui::Align2) {
        let x = if centered {
            button.center().x
        } else {
            button.left()
        };
        let at_bottom = self.app_state.settings.topbar_position == state::TopbarPosition::Bottom;
        match (at_bottom, centered) {
            (false, true) => (
                egui::pos2(x, button.bottom() + 6.0),
                egui::Align2::CENTER_TOP,
            ),
            (false, false) => (egui::pos2(x, button.bottom() + 6.0), egui::Align2::LEFT_TOP),
            (true, true) => (
                egui::pos2(x, button.top() - 6.0),
                egui::Align2::CENTER_BOTTOM,
            ),
            (true, false) => (egui::pos2(x, button.top() - 6.0), egui::Align2::LEFT_BOTTOM),
        }
    }

    /// Space occupied by visible bars at the bottom of the window.
    pub(in crate::workspace) fn bottom_bars_h(&self) -> f32 {
        let settings = &self.app_state.settings;
        let topbar = if settings.topbar_position == state::TopbarPosition::Bottom {
            self.topbar_dock_h + self.topbar_overlay_h
        } else {
            0.0
        };
        let annotation = if settings.annotation_bar_edge() == state::TopbarPosition::Bottom {
            self.subbar_h
        } else {
            0.0
        };
        topbar + annotation
    }

    /// Moves the topbar between the top and the bottom edge (saved).
    pub(in crate::workspace) fn toggle_topbar_position(&mut self) {
        let settings = &mut self.app_state.settings;
        settings.topbar_position = match settings.topbar_position {
            state::TopbarPosition::Top => state::TopbarPosition::Bottom,
            state::TopbarPosition::Bottom => state::TopbarPosition::Top,
        };
        self.app_state.save();
        self.topbar_hold_until = 0.0;
        self.topbar_overlay_h = 0.0;
    }

    pub(in crate::workspace) fn topbar_tools_button(
        &mut self,
        ui: &mut egui::Ui,
        sq_btn: egui::Vec2,
        labeled: bool,
    ) {
        if !self.app_state.settings.tools_button {
            return;
        }
        let btn_size = if labeled {
            egui::vec2(200.0, sq_btn.y)
        } else {
            sq_btn
        };
        if ui
            .add(
                crate::theme::selected_button(
                    if labeled {
                        format!("{}  Annotation tools", self.annotation_cursor_icon())
                    } else {
                        self.annotation_cursor_icon().to_string()
                    },
                    self.show_tools_rail,
                    ui.visuals(),
                )
                .min_size(btn_size),
            )
            .on_hover_text(
                self.app_state
                    .shortcuts
                    .toggle_tools
                    .hint("Toggle Annotation Tools Bar"),
            )
            .clicked()
            && self.has_pdf()
        {
            if labeled {
                ui.close();
            }
            self.show_tools_rail = !self.show_tools_rail;
            if !self.show_tools_rail && self.app_state.settings.reset_tool_on_bar_close {
                self.annotation_tool = AnnotationTool::Cursor;
            }
        }
    }

    pub(in crate::workspace) fn topbar_view_mode_menu(&mut self, ui: &mut egui::Ui) {
        // View Mode Dropdown
        let view_label = match self.view_mode {
            ViewMode::SinglePage => "󰈙 Single",
            ViewMode::Continuous => "󰡏 Continuous",
            ViewMode::TwoPage => "󰂺 Book",
            ViewMode::Slideshow => "󰐨 Slideshow",
            ViewMode::SlideshowWithNotes => "󰠮 Present+Notes",
        };

        Self::glass_menu_button(ui, self.menu_glass(), view_label, |ui| {
            ui.set_min_width(215.0);
            if ui
                .selectable_label(
                    self.view_mode == ViewMode::SinglePage,
                    self.app_state.shortcuts.view_single.hint("󰈙  Single Page"),
                )
                .clicked()
            {
                self.set_view_mode(ViewMode::SinglePage);
                ui.close();
            }
            if ui
                .selectable_label(
                    self.view_mode == ViewMode::Continuous,
                    self.app_state
                        .shortcuts
                        .view_continuous
                        .hint("󰡏  Continuous Scroll"),
                )
                .clicked()
            {
                self.set_view_mode(ViewMode::Continuous);
                ui.close();
            }

            let book_menu_label = if self.book_cover_mode {
                self.app_state
                    .shortcuts
                    .view_twopage
                    .hint("󰂻  Book (2,3 Cover)")
            } else {
                self.app_state
                    .shortcuts
                    .view_twopage
                    .hint("󰂺  Book (1,2 Pair)")
            };

            if ui
                .selectable_label(self.view_mode == ViewMode::TwoPage, book_menu_label)
                .on_hover_text(
                    "Click again while in Book mode to toggle between Cover (2,3) and Pair (1,2)",
                )
                .clicked()
            {
                if self.view_mode == ViewMode::TwoPage {
                    self.book_cover_mode = !self.book_cover_mode;
                } else {
                    self.view_mode = ViewMode::TwoPage;
                }
                ui.close();
            }

            ui.separator();
            if ui
                .selectable_label(
                    self.view_mode == ViewMode::Slideshow,
                    self.app_state.shortcuts.view_slideshow.hint("󰐨  Slideshow"),
                )
                .clicked()
            {
                if self.view_mode != ViewMode::Slideshow
                    && self.view_mode != ViewMode::SlideshowWithNotes
                {
                    self.prev_view_mode = self.view_mode;
                }
                self.view_mode = ViewMode::Slideshow;
                ui.close();
            }
            if ui
                .selectable_label(
                    self.view_mode == ViewMode::SlideshowWithNotes,
                    self.app_state
                        .shortcuts
                        .view_slideshow_notes
                        .hint("󰠮  Slideshow + Notes"),
                )
                .clicked()
            {
                if self.view_mode != ViewMode::Slideshow
                    && self.view_mode != ViewMode::SlideshowWithNotes
                {
                    self.prev_view_mode = self.view_mode;
                }
                self.view_mode = ViewMode::SlideshowWithNotes;
                ui.close();
            }
        })
        .response
        .on_hover_text("Change View Mode");
    }

    pub(in crate::workspace) fn topbar_split_button(
        &mut self,
        ui: &mut egui::Ui,
        sq_btn: egui::Vec2,
        labeled: bool,
    ) {
        let btn_size = if labeled {
            egui::vec2(200.0, sq_btn.y)
        } else {
            sq_btn
        };
        if ui
            .add(
                crate::theme::selected_button(
                    if labeled {
                        format!("{}  Split view", "󰯌")
                    } else {
                        "󰯌".to_string()
                    },
                    self.split_view_enabled,
                    ui.visuals(),
                )
                .min_size(btn_size),
            )
            .on_hover_text("Toggle PDF Split-View (view & annotate two pages side-by-side)")
            .clicked()
            && self.has_pdf()
        {
            if labeled {
                ui.close();
            }
            self.split_view_enabled = !self.split_view_enabled;
            if self.split_view_enabled {
                self.split_secondary_page = (self.current_page + 1).min(self.total_pages.max(1));
            }
        }
    }

    pub(in crate::workspace) fn topbar_dark_button(
        &mut self,
        ui: &mut egui::Ui,
        sq_btn: egui::Vec2,
        labeled: bool,
    ) {
        let btn_size = if labeled {
            egui::vec2(200.0, sq_btn.y)
        } else {
            sq_btn
        };
        let dark_icon = if self.pdf_dark_mode { "󰖔" } else { "󰖙" };
        if ui
            .add(
                crate::theme::selected_button(
                    if labeled {
                        format!("{}  PDF night mode", dark_icon)
                    } else {
                        dark_icon.to_string()
                    },
                    self.pdf_dark_mode,
                    ui.visuals(),
                )
                .min_size(btn_size),
            )
            .on_hover_text(
                self.app_state
                    .shortcuts
                    .toggle_night
                    .hint("Toggle PDF Dark / Night Mode"),
            )
            .clicked()
        {
            if labeled {
                ui.close();
            }
            self.pdf_dark_mode = !self.pdf_dark_mode;
            self.viewer.invalidate_images();
            self.save_session_state();
        }
    }

    pub(in crate::workspace) fn topbar_search_button(
        &mut self,
        ui: &mut egui::Ui,
        sq_btn: egui::Vec2,
        labeled: bool,
    ) {
        let btn_size = if labeled {
            egui::vec2(200.0, sq_btn.y)
        } else {
            sq_btn
        };
        if ui
            .add(
                crate::theme::selected_button(
                    if labeled {
                        format!("{} Search in notes", "")
                    } else {
                        " ".to_string()
                    },
                    self.show_search,
                    ui.visuals(),
                )
                .min_size(btn_size),
            )
            .on_hover_text(self.app_state.shortcuts.search_notes.hint("Global Search"))
            .clicked()
            && self.has_pdf()
        {
            if labeled {
                ui.close();
            }
            if self.show_search {
                self.close_search();
            } else {
                self.open_search();
            }
        }
    }

    /// Allow corner badges to extend into the topbar margin.
    pub(in crate::workspace) fn paint_topbar_dot(&self, ui: &egui::Ui, button: egui::Rect) {
        let m = self.app_state.settings.ui_density.bar();
        let extent = m.dot_radius + 0.5;
        let mut clip = ui.clip_rect();
        clip.min.y -= f32::from(m.margin_y);
        clip = clip.intersect(ui.ctx().content_rect());
        if clip.width() < 2.0 * extent || clip.height() < 2.0 * extent {
            return;
        }
        let center = egui::pos2(
            (button.right() - m.dot_inset).clamp(clip.left() + extent, clip.right() - extent),
            (button.top() + m.dot_inset).clamp(clip.top() + extent, clip.bottom() - extent),
        );
        ui.ctx()
            .layer_painter(ui.layer_id())
            .with_clip_rect(clip)
            .circle(
                center,
                m.dot_radius,
                rgb(self.theme.accent),
                egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 255, 255)),
            );
    }

    pub(in crate::workspace) fn topbar_flash_button(
        &mut self,
        ui: &mut egui::Ui,
        sq_btn: egui::Vec2,
        labeled: bool,
    ) {
        if !self.app_state.settings.flash_button {
            return;
        }
        let btn_size = if labeled {
            egui::vec2(200.0, sq_btn.y)
        } else {
            sq_btn
        };
        // Flashcards of the page on screen: the live text of the editor when it is that page's note.
        let has_flashcards = self.document_has_flashcards();
        let resp = ui.add(
            crate::theme::selected_button(
                if labeled {
                    format!("{}  Flashcards", "󰘸")
                } else {
                    "󰘸 ".to_string()
                },
                self.show_flashcards_modal,
                ui.visuals(),
            )
            .min_size(btn_size),
        );
        if has_flashcards && self.app_state.settings.flash_notification_dot {
            // Same dot as the Notes button.
            self.paint_topbar_dot(ui, resp.rect);
        }
        let has_cards_hover = if has_flashcards {
            "\n(This document contains flashcards)"
        } else {
            ""
        };
        if resp
            .on_hover_text(format!(
                "Flashcards (parses '::' and 'Q:/A:' from notes) {}",
                has_cards_hover
            ))
            .clicked()
            && self.has_pdf()
        {
            if labeled {
                ui.close();
            }
            if self.show_flashcards_modal {
                self.show_flashcards_modal = false;
            } else {
                self.toggle_flashcards_modal();
            }
        }
    }

    pub(in crate::workspace) fn topbar_newpage_button(
        &mut self,
        ui: &mut egui::Ui,
        sq_btn: egui::Vec2,
        labeled: bool,
    ) {
        let btn_size = if labeled {
            egui::vec2(200.0, sq_btn.y)
        } else {
            sq_btn
        };
        let response = ui
            .add(egui::Button::new(if labeled { "  New Page" } else { "" }).min_size(btn_size))
            .on_hover_text(
                "Click: add a page with the default style · Right-click: choose page style",
            );

        let mut chosen_style = None;
        let glass_style = self.menu_glass().modifier();
        egui::Popup::context_menu(&response)
            .style(glass_style)
            .show(|ui| {
                ui.set_min_width(190.0);
                ui.label(egui::RichText::new("New page style").strong());
                ui.add_space(4.0);
                for (style, label, description) in [
                    (
                        notes::BlankPageStyle::Ruled,
                        "Ruled",
                        "Traditional horizontal lines",
                    ),
                    (
                        notes::BlankPageStyle::Dotted,
                        "Dotted",
                        "Subtle writing dots",
                    ),
                    (notes::BlankPageStyle::Grid, "Grid", "Even square grid"),
                    (
                        notes::BlankPageStyle::Blank,
                        "Blank",
                        "Completely empty page",
                    ),
                ] {
                    let selected = self.app_state.settings.default_page_style == style;
                    let row_label = if selected {
                        format!("●  {label}")
                    } else {
                        format!("    {label}")
                    };
                    let row = ui.add(
                        crate::theme::selected_button(
                            egui::RichText::new(row_label).strong(),
                            selected,
                            ui.visuals(),
                        )
                        .frame_when_inactive(selected),
                    );
                    let row = row.on_hover_text(description);
                    if row.clicked() {
                        chosen_style = Some(style);
                        ui.close();
                    }
                }
                ui.separator();
                ui.small("Current default is marked with a dot.");
            });

        if response.clicked() {
            chosen_style = Some(self.app_state.settings.default_page_style);
        }
        if let Some(style) = chosen_style {
            if labeled {
                ui.close();
            }
            self.insert_blank_page_after(self.current_page, style, ui.ctx());
        }
    }

    /// Render a bookmark button and return its popup anchor.
    pub(in crate::workspace) fn topbar_bookmark_button(
        &mut self,
        ui: &mut egui::Ui,
        sq_btn: egui::Vec2,
        labeled: bool,
    ) -> egui::Rect {
        let btn_size = if labeled {
            egui::vec2(200.0, sq_btn.y)
        } else {
            sq_btn
        };
        let is_bookmarked = self
            .user_bookmarks
            .iter()
            .any(|b| b.page == self.current_page);
        let bm_icon = if is_bookmarked { "󰃀" } else { "󰃃" };
        let bm_id = ui.make_persistent_id("bookmark_dropdown_open");
        let mut bm_open = ui.data_mut(|d| *d.get_temp_mut_or_default::<bool>(bm_id));

        let bm_resp = ui
            .add(
                crate::theme::selected_button(
                    egui::RichText::new(if labeled {
                        format!("{}  Bookmark", bm_icon)
                    } else {
                        bm_icon.to_string()
                    })
                    .color(if bm_open {
                        self.theme.on_accent()
                    } else if is_bookmarked {
                        rgb(self.theme.warning)
                    } else {
                        rgb(self.theme.fg_primary)
                    }),
                    bm_open,
                    ui.visuals(),
                )
                .min_size(btn_size),
            )
            .on_hover_text("Add / Edit Bookmark for this page");

        if bm_resp.clicked() && self.has_pdf() {
            bm_open = !bm_open;
            if bm_open {
                self.bookmark_input = self
                    .user_bookmarks
                    .iter()
                    .find(|b| b.page == self.current_page)
                    .map(|b| b.title.clone())
                    .unwrap_or_else(|| format!("Page {}", self.current_page));
            }
        }

        if labeled && bm_resp.clicked() {
            ui.data_mut(|d| d.insert_temp(ui.make_persistent_id("bookmark_skip_outside"), true));
            ui.close();
        }
        ui.data_mut(|d| *d.get_temp_mut_or_default::<bool>(bm_id) = bm_open);
        bm_resp.rect
    }

    /// Render the bookmark popup at `anchor`.
    pub(in crate::workspace) fn render_bookmark_popup(
        &mut self,
        ui: &mut egui::Ui,
        anchor: egui::Rect,
    ) {
        let is_bookmarked = self
            .user_bookmarks
            .iter()
            .any(|b| b.page == self.current_page);
        let bm_id = ui.make_persistent_id("bookmark_dropdown_open");
        let mut bm_open = ui.data_mut(|d| *d.get_temp_mut_or_default::<bool>(bm_id));
        if bm_open {
            let mut close_bm = false;
            let (popup_pos, popup_pivot) = self.topbar_popup_placement(anchor, true);

            let area_resp = egui::Area::new(ui.make_persistent_id("bookmark_popup_area"))
                .fixed_pos(popup_pos)
                .pivot(popup_pivot)
                .order(egui::Order::Foreground)
                .show(ui.ctx(), |ui| {
                    self.modal_glass_frame(MENU_GLASS_ALPHA)
                        .inner_margin(egui::Margin::same(10))
                        .show(ui, |ui| {
                            ui.set_width(220.0);
                            ui.label(
                                egui::RichText::new(format!(
                                    "󰃀  Bookmark Page {}",
                                    self.current_page
                                ))
                                .strong()
                                .size(13.0),
                            );
                            ui.add_space(4.0);

                            let input_resp = ui.add(
                                egui::TextEdit::singleline(&mut self.bookmark_input)
                                    .hint_text("Bookmark name...")
                                    .desired_width(f32::INFINITY),
                            );

                            // Auto-focus the input box when opened
                            if !input_resp.has_focus() {
                                input_resp.request_focus();
                            }

                            let enter_pressed = input_resp.lost_focus()
                                && ui.input(|i| i.key_pressed(egui::Key::Enter));

                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                if ui
                                    .add_sized([100.0, 26.0], egui::Button::new("󰄬  Save"))
                                    .clicked()
                                    || enter_pressed
                                {
                                    let name = self.bookmark_input.clone();
                                    self.add_or_update_bookmark(self.current_page, name);
                                    self.toast_message = Some((
                                        format!("󰃀 Bookmark saved for page {}", self.current_page),
                                        std::time::Instant::now(),
                                    ));
                                    close_bm = true;
                                }

                                if is_bookmarked
                                    && ui
                                        .add_sized([90.0, 26.0], egui::Button::new("󰆴  Remove"))
                                        .clicked()
                                {
                                    self.remove_bookmark(self.current_page);
                                    close_bm = true;
                                }
                            });
                        });
                });

            // Ignore the click that opened the popup.
            let skip_outside = ui
                .data_mut(|d| d.remove_temp::<bool>(ui.make_persistent_id("bookmark_skip_outside")))
                .unwrap_or(false);
            let clicked_out = !skip_outside
                && ui.input(|i| i.pointer.any_click())
                && ui
                    .input(|i| i.pointer.interact_pos())
                    .map(|p| !anchor.contains(p) && !area_resp.response.rect.contains(p))
                    .unwrap_or(false);

            if close_bm || clicked_out || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                bm_open = false;
            }
        }
        ui.data_mut(|d| *d.get_temp_mut_or_default::<bool>(bm_id) = bm_open);
        if bm_open {
            self.topbar_popup_open = true;
        }
    }

    pub(in crate::workspace) fn render_sub_topbar_tools(&mut self, ui: &mut egui::Ui) {
        let settings = &self.app_state.settings;
        let bottom = settings.annotation_bar_edge() == state::TopbarPosition::Bottom;
        let offset = if settings.annotation_bar_edge() == settings.topbar_position {
            self.topbar_overlay_h
        } else {
            0.0
        };
        if offset > 0.0 {
            // Reserve the revealed topbar before docking the annotation toolbar.
            let panel = if bottom {
                egui::Panel::bottom("subbar_topbar_space")
            } else {
                egui::Panel::top("subbar_topbar_space")
            };
            panel
                .exact_size(offset)
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    ui.allocate_rect(ui.max_rect(), egui::Sense::hover());
                });
        }
        self.draw_sub_topbar_tools(ui, bottom);
    }

    pub(in crate::workspace) fn render_radial_tools_menu(&mut self, ui: &mut egui::Ui) {
        let Some(raw_center) = self.radial_menu_pos else {
            return;
        };
        if !self.app_state.settings.radial_menu_enabled {
            self.radial_menu_pos = None;
            return;
        }
        // Escape closes the menu unless a text field is being edited.
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) && !ui.ctx().egui_wants_keyboard_input() {
            self.radial_menu_pos = None;
            return;
        }

        // Clamp center so the 105px radius circle never goes off-screen
        let screen = ui.ctx().content_rect();
        let margin = 110.0;
        let center = egui::pos2(
            raw_center
                .x
                .clamp(screen.left() + margin, screen.right() - margin),
            raw_center
                .y
                .clamp(screen.top() + margin, screen.bottom() - margin),
        );

        let mut close_menu = false;

        // 1. Radial circle
        let pick_id = ui.make_persistent_id("radial_shape_pick");
        let shape_pick: bool = ui.data(|d| d.get_temp::<egui::Pos2>(pick_id)) == Some(raw_center);

        egui::Area::new(ui.make_persistent_id("radial_pie_tools_menu"))
            .fixed_pos(center - egui::vec2(105.0, 105.0))
            .order(egui::Order::Tooltip)
            .show(ui.ctx(), |ui| {
                // 210 × 210 circle plus 30 px for the hover label.
                let (box_rect, bg_resp) =
                    ui.allocate_exact_size(egui::vec2(210.0, 240.0), egui::Sense::click());
                let c = box_rect.min + egui::vec2(105.0, 105.0);
                let painter = ui.painter();

                // Tight, subtle shadow and a translucent surface.
                for (radius, alpha) in [(109.0, 3), (107.0, 5), (105.0, 7), (103.0, 9)] {
                    painter.circle_filled(
                        c + egui::vec2(0.0, 2.0),
                        radius,
                        egui::Color32::from_black_alpha(alpha),
                    );
                }
                let [gr, gg, gb] = self.theme.bg_elevated;
                painter.circle_filled(
                    c,
                    102.0,
                    egui::Color32::from_rgba_unmultiplied(gr, gg, gb, 232),
                );
                let accent = rgb(self.theme.accent);
                painter.circle_stroke(
                    c,
                    102.0,
                    egui::Stroke::new(1.0, accent.gamma_multiply(0.65)),
                );
                painter.circle_stroke(
                    c,
                    52.0,
                    egui::Stroke::new(0.75, accent.gamma_multiply(0.40)),
                );

                let hover_pos = ui.input(|i| i.pointer.hover_pos());
                let left_clicked = bg_resp.clicked_by(egui::PointerButton::Primary);
                let right_clicked = bg_resp.clicked_by(egui::PointerButton::Secondary);
                let clicked = left_clicked || right_clicked;
                let mut label_hover: Option<String> = None;

                // Central hub: Undo
                let hub_r = 18.0;
                let can_undo = self.can_undo_annotation(self.current_page);

                let in_hub = hover_pos.map(|p| p.distance(c) <= hub_r).unwrap_or(false);

                painter.circle_filled(
                    c,
                    hub_r,
                    rgb(if in_hub && can_undo {
                        self.theme.bg_hover
                    } else {
                        self.theme.bg_widget
                    }),
                );
                painter.circle_stroke(c, hub_r, egui::Stroke::new(1.0, rgb(self.theme.border)));
                let undo_pos = c;

                let fg = rgb(self.theme.fg_primary);
                let fg_dim = fg.gamma_multiply(0.35);
                painter.text(
                    undo_pos,
                    egui::Align2::CENTER_CENTER,
                    "󰕌",
                    egui::FontId::proportional(16.0),
                    if can_undo { fg } else { fg_dim },
                );

                if in_hub {
                    label_hover = Some("Undo PDF Annotation".into());
                    if clicked && can_undo {
                        self.undo_annotation(self.current_page);
                    }
                }

                // Inner colors ring
                if shape_pick {
                    // Choose a shape instead of a color.
                    let kinds = [
                        (ShapeKind::Rect, "󰝣", "Rectangle"),
                        (ShapeKind::Circle, "󰝥", "Circle"),
                        (ShapeKind::Line, "󰕞", "Line"),
                        (ShapeKind::Arrow, "󰁔", "Arrow"),
                    ];
                    for (i, (sk, icon, lbl)) in kinds.iter().enumerate() {
                        let angle =
                            (i as f32) * std::f32::consts::TAU / 4.0 - std::f32::consts::FRAC_PI_2;
                        let pt = c + egui::vec2(angle.cos(), angle.sin()) * 36.0;
                        let is_sel = self.shape_kind == *sk;
                        let is_hov = hover_pos.map(|p| p.distance(pt) <= 13.0).unwrap_or(false);

                        painter.circle_filled(
                            pt,
                            13.0,
                            if is_sel {
                                rgb(self.theme.accent)
                            } else if is_hov {
                                rgb(self.theme.bg_hover)
                            } else {
                                rgb(self.theme.bg_widget)
                            },
                        );
                        painter.circle_stroke(
                            pt,
                            13.0,
                            egui::Stroke::new(1.2, rgb(self.theme.border)),
                        );
                        painter.text(
                            pt,
                            egui::Align2::CENTER_CENTER,
                            icon,
                            egui::FontId::proportional(14.0),
                            if is_sel {
                                self.theme.on_accent()
                            } else {
                                rgb(self.theme.fg_primary)
                            },
                        );
                        if is_hov {
                            label_hover =
                                Some(shape_shortcut_hint(&self.app_state.shortcuts, *sk, lbl));
                        }
                        if clicked && is_hov {
                            self.shape_kind = *sk;
                            self.annotation_tool = AnnotationTool::Shape;
                            close_menu = left_clicked;
                        }
                    }
                } else {
                    let n_cols = ANNOTATION_PALETTE.len();
                    let col_ring_r = 36.0;
                    for (i, (col, name)) in ANNOTATION_PALETTE.iter().copied().enumerate() {
                        let angle = (i as f32) * std::f32::consts::TAU / (n_cols as f32)
                            - std::f32::consts::FRAC_PI_2;
                        let pt = c + egui::vec2(angle.cos(), angle.sin()) * col_ring_r;
                        let is_sel = self.annotation_color() == col;
                        let is_hov = hover_pos.map(|p| p.distance(pt) <= 10.0).unwrap_or(false);

                        painter.circle_filled(pt, 8.0, rgb(col));
                        if is_sel || is_hov {
                            painter.circle_stroke(
                                pt,
                                10.0,
                                egui::Stroke::new(
                                    if is_sel { 1.5 } else { 1.0 },
                                    rgb(self.theme.fg_primary),
                                ),
                            );
                        }
                        if is_hov {
                            label_hover = Some(name.to_string());
                        }
                        if clicked && is_hov {
                            self.pending_annotation_color = Some(col);
                            close_menu = left_clicked;
                        }
                    }
                }
                // Outer ring: tools (77 px radius).
                let tools: &[(AnnotationTool, &str, &str)] = &[
                    (AnnotationTool::Cursor, "󰇀", "Cursor"),
                    (AnnotationTool::Highlight, " ", "Highlight"),
                    (AnnotationTool::Underline, "󰊇", "Underline"),
                    (AnnotationTool::Draw, "󱦹", "Pen"),
                    (AnnotationTool::Shape, "󰠱 ", "Shapes"),
                    (AnnotationTool::TextBox, "󰊄", "Text"),
                    (AnnotationTool::Tape, "󰝤 ", "Tape"),
                    (AnnotationTool::Snip, "󰆞 ", "Snip"),
                    (AnnotationTool::Laser, "󰐾 ", "Laser"),
                    (AnnotationTool::Eraser, "󰇾 ", "Eraser"),
                ];
                let outer_r = 77.0;
                let node_r = 18.0;

                for (i, &(tool, icon, label)) in tools.iter().enumerate() {
                    let angle = (i as f32) * std::f32::consts::TAU / (tools.len() as f32)
                        - std::f32::consts::FRAC_PI_2;
                    let pt = c + egui::vec2(angle.cos(), angle.sin()) * outer_r;
                    let is_active = self.annotation_tool == tool;
                    let is_hov = hover_pos.map(|p| p.distance(pt) <= node_r).unwrap_or(false);

                    // The shape icon reflects the current selection.
                    let icon = if tool == AnnotationTool::Shape {
                        match self.shape_kind {
                            ShapeKind::Rect => "󰝣",
                            ShapeKind::Circle => "󰝥",
                            ShapeKind::Line => "󰕞",
                            ShapeKind::Arrow => "󰁔",
                        }
                    } else {
                        icon
                    };

                    let fill = if is_active {
                        rgb(self.theme.accent)
                    } else if is_hov {
                        rgb(self.theme.bg_hover)
                    } else {
                        rgb(self.theme.bg_widget)
                    };
                    painter.circle_filled(pt, node_r, fill);
                    if !is_active {
                        painter.circle_stroke(
                            pt,
                            node_r,
                            egui::Stroke::new(
                                1.0,
                                if is_hov {
                                    rgb(self.theme.accent)
                                } else {
                                    rgb(self.theme.border).gamma_multiply(0.65)
                                },
                            ),
                        );
                    }
                    painter.text(
                        pt,
                        egui::Align2::CENTER_CENTER,
                        icon,
                        egui::FontId::proportional(15.0),
                        if is_active {
                            self.theme.on_accent()
                        } else {
                            rgb(self.theme.fg_primary)
                        },
                    );

                    if is_hov {
                        label_hover = Some(annotation_shortcut_hint(
                            &self.app_state.shortcuts,
                            tool,
                            label,
                        ));
                    }

                    if clicked && is_hov {
                        self.annotation_tool = tool;
                        if tool == AnnotationTool::Shape {
                            // Shape picker stays open so its shape can be chosen next.
                            ui.data_mut(|d| d.insert_temp(pick_id, raw_center));
                        } else {
                            close_menu = left_clicked;
                        }
                    }
                }

                // A small satellite action, not another annotation tool.
                // Only display it if an image was available when the menu opened.
                if self.radial_clipboard_image_available {
                    let paste_pos = c + egui::vec2(79.0, -75.0);
                    let paste_hovered = hover_pos
                        .map(|p| p.distance(paste_pos) <= 12.0)
                        .unwrap_or(false);
                    painter.circle_filled(
                        paste_pos,
                        12.0,
                        if paste_hovered {
                            rgb(self.theme.bg_hover)
                        } else {
                            rgb(self.theme.bg_widget)
                        },
                    );
                    painter.circle_stroke(
                        paste_pos,
                        12.0,
                        egui::Stroke::new(1.2, rgb(self.theme.accent)),
                    );
                    painter.text(
                        paste_pos,
                        egui::Align2::CENTER_CENTER,
                        "󰆒 ",
                        egui::FontId::proportional(12.0),
                        rgb(self.theme.fg_primary),
                    );
                    if paste_hovered {
                        label_hover = Some("Paste image".to_string());
                        if clicked {
                            let (page, center) = self.page_at_pos(Some(raw_center));
                            self.shots
                                .request_clipboard(ui.ctx(), ShotTarget::Pdf { page, center });
                            close_menu = left_clicked;
                        }
                    }
                }

                // Label for the node under the pointer.
                if let Some(txt) = &label_hover {
                    let pos = c + egui::vec2(0.0, 122.0);
                    let galley = painter.layout_no_wrap(
                        txt.clone(),
                        egui::FontId::proportional(12.0),
                        rgb(self.theme.fg_primary),
                    );
                    let r =
                        egui::Rect::from_center_size(pos, galley.size() + egui::vec2(18.0, 8.0));
                    let [gr, gg, gb] = self.theme.bg_elevated;
                    painter.rect_filled(
                        r,
                        egui::CornerRadius::same(8),
                        egui::Color32::from_rgba_unmultiplied(gr, gg, gb, 232),
                    );
                    painter.rect_stroke(
                        r,
                        egui::CornerRadius::same(8),
                        egui::Stroke::new(1.0, rgb(self.theme.border)),
                        egui::StrokeKind::Inside,
                    );
                    painter.galley(
                        pos - galley.size() * 0.5,
                        galley,
                        rgb(self.theme.fg_primary),
                    );
                }
            });

        // ====================================================================
        // 3. Close when the pointer is pressed outside the circle and panel.
        // Use press rather than click so drawing drags also close the menu.
        // ====================================================================

        let press_pos = ui.input(|i| {
            if i.pointer.any_pressed() {
                i.pointer.interact_pos()
            } else {
                None
            }
        });
        if let Some(p) = press_pos
            && p.distance(center)
                > if self.radial_clipboard_image_available {
                    122.0
                } else {
                    106.0
                }
        {
            close_menu = true;
        }
        if close_menu {
            ui.data_mut(|d| d.remove::<egui::Pos2>(pick_id));
            self.radial_menu_pos = None;
        }
    }
}

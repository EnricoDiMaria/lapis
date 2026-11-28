use super::*;

impl Lapis {
    pub(super) fn render_markdown_preview_with_arrow(
        &mut self,
        ui: &mut egui::Ui,
        scroll_id_str: &'static str,
        md_source: &str,
        header_ui: impl FnOnce(&mut egui::Ui),
    ) {
        let scroll_id = ui.make_persistent_id(scroll_id_str);
        let anim_scroll_id = scroll_id.with("anim_target");
        let current_offset_y = ui.data(|d| {
            d.get_temp::<f32>(scroll_id.with("last_offset"))
                .unwrap_or(0.0)
        });

        let mut scroll_area = egui::ScrollArea::vertical()
            .id_salt(scroll_id_str)
            .auto_shrink([false, false]);

        if let Some(target) = ui.data(|d| d.get_temp::<f32>(anim_scroll_id)) {
            let diff = target - current_offset_y;
            if diff.abs() > 1.0 {
                scroll_area = scroll_area.vertical_scroll_offset(current_offset_y + diff * 0.2);
                ui.ctx().request_repaint();
            } else {
                ui.data_mut(|d| d.remove::<f32>(anim_scroll_id));
            }
        }

        let note_font_sz = self.app_state.settings.note_font_size.clamp(10.0, 28.0);

        let scroll_out = scroll_area.show(ui, |ui| {
            // Keep the Markdown viewport and any embedded images within the panel width
            // on every frame while the side-panel splitter is moving.
            let preview_width = ui.available_width().max(1.0);
            ui.take_available_width();
            header_ui(ui);
            ui.scope(|ui| {
                ui.take_available_width();
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

                let rendered_md = flashcards::preprocess_notes_preview(md_source);

                egui_commonmark::CommonMarkViewer::new()
                    .max_image_width(Some(preview_width.ceil() as usize))
                    .show(ui, &mut self.commonmark_cache, &rendered_md);
                ui.add_space(28.0);
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
            for text in copied_texts {
                self.clipboard.set_text(&text);
            }
        });

        ui.data_mut(|d| {
            d.insert_temp(
                scroll_id.with("last_offset"),
                scroll_out.state.clamped_offset().y,
            );
        });

        // Draw the fade gradient and arrow badge.
        let viewport_rect = scroll_out.inner_rect;
        let content_h = scroll_out.content_size.y;
        let view_h = viewport_rect.height();
        let offset_y = scroll_out.state.clamped_offset().y;
        let remaining_px = content_h - (offset_y + view_h);

        if remaining_px > 8.0 {
            let alpha = (remaining_px / 35.0).clamp(0.25, 0.95);
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
            let bot_col = egui::Color32::from_rgba_unmultiplied(
                elevated.r(),
                elevated.g(),
                elevated.b(),
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

            let pill_rect = egui::Rect::from_center_size(
                egui::pos2(viewport_rect.center().x, viewport_rect.bottom() - 15.0),
                egui::vec2(34.0, 22.0),
            );
            let resp = ui.interact(
                pill_rect,
                scroll_id.with("more_below_btn"),
                egui::Sense::click(),
            );
            let pill_bg = if resp.hovered() {
                let active = rgb(self.theme.bg_active);
                egui::Color32::from_rgba_unmultiplied(active.r(), active.g(), active.b(), 245)
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
    }
}

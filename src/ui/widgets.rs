//! Shared menu, spacing, and display helpers.

use super::super::*;

pub(in crate::workspace) fn truncate_filename<P: AsRef<Path>>(
    path_ref: P,
    max_len: usize,
) -> String {
    let path = path_ref.as_ref();

    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let file_stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");

    let truncated_stem: String = {
        // Counting total characters
        let total_chars = file_stem.chars().count();

        if total_chars > max_len {
            let available_chars = max_len.saturating_sub(2);
            let keep_start = (available_chars as f32 * 0.7).round() as usize;
            let keep_end = available_chars.saturating_sub(keep_start);

            // Finding index to where execute cut
            let start_byte_idx = file_stem
                .char_indices()
                .nth(keep_start)
                .map_or(file_stem.len(), |(idx, _)| idx);

            let skip_from_end = total_chars.saturating_sub(keep_end);
            let end_byte_idx = file_stem
                .char_indices()
                .nth(skip_from_end)
                .map_or(file_stem.len(), |(idx, _)| idx);

            let start_part = &file_stem[..start_byte_idx];
            let end_part = &file_stem[end_byte_idx..];

            format!("{}…{}", start_part, end_part)
        } else {
            file_stem.to_string()
        }
    };

    if extension.is_empty() {
        truncated_stem
    } else {
        format!("{}.{}", truncated_stem, extension)
    }
}

/// Background alpha of the menus and popups opened from the main page (0 = invisible).
pub(in crate::workspace) const MENU_GLASS_ALPHA: u8 = 200;

/// Frame look of the built-in egui menus, taken from `modal_glass_frame`.
#[derive(Clone, Copy)]
pub(in crate::workspace) struct MenuGlass {
    fill: egui::Color32,
    stroke: egui::Stroke,
    radius: egui::CornerRadius,
    shadow: egui::Shadow,
}

impl MenuGlass {
    /// egui draws the frame of a menu from the style the menu is opened with, not from the
    /// style of the button, so the look has to be passed as a style modifier.
    pub(in crate::workspace) fn modifier(self) -> egui::style::StyleModifier {
        egui::style::StyleModifier::new(move |style| {
            egui::containers::menu::menu_style(style);
            let v = &mut style.visuals;
            v.window_fill = self.fill;
            v.window_stroke = self.stroke;
            v.menu_corner_radius = self.radius;
            v.popup_shadow = self.shadow;
        })
    }
}

impl Lapis {
    /// Scales a fixed panel size (button height, margin) by the interface density.
    pub(in crate::workspace) fn sz(&self, base: f32) -> f32 {
        (base * self.app_state.settings.ui_density.scale()).round()
    }

    /// Glass look of the menus and popups opened from the main page.
    pub(in crate::workspace) fn menu_glass(&self) -> MenuGlass {
        let f = self.modal_glass_frame(MENU_GLASS_ALPHA);
        MenuGlass {
            fill: f.fill,
            stroke: f.stroke,
            radius: f.corner_radius,
            shadow: f.shadow,
        }
    }

    /// Like `Ui::menu_button`, with the glass look. Inside another menu it stays a submenu,
    /// which inherits the look of its parent.
    pub(in crate::workspace) fn glass_menu_button<'a, R>(
        ui: &mut egui::Ui,
        glass: MenuGlass,
        atoms: impl egui::IntoAtoms<'a>,
        add_contents: impl FnOnce(&mut egui::Ui) -> R,
    ) -> egui::InnerResponse<Option<R>> {
        if egui::containers::menu::is_in_menu(ui) {
            return ui.menu_button(atoms, add_contents);
        }
        let (response, inner) = egui::containers::menu::MenuButton::new(atoms)
            .config(egui::containers::menu::MenuConfig::new().style(glass.modifier()))
            .ui(ui, add_contents);
        egui::InnerResponse::new(inner.map(|i| i.inner), response)
    }

    pub(in crate::workspace) fn modal_glass_frame(&self, alpha: u8) -> egui::Frame {
        let [r, g, b] = self.theme.bg_elevated;
        egui::Frame::NONE
            .fill(egui::Color32::from_rgba_unmultiplied(r, g, b, alpha))
            .stroke(egui::Stroke::new(1.0, rgb(self.theme.border)))
            .corner_radius(egui::CornerRadius::same(12))
            .inner_margin(egui::Margin::same(16))
            .shadow(egui::Shadow {
                offset: [0, 8],
                blur: 24,
                spread: 2,
                color: egui::Color32::from_black_alpha(140),
            })
    }
}

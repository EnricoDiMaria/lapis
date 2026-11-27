use crate::theme::{CATPPUCCIN_ACCENTS, CATPPUCCIN_FLAVORS, SuiteTheme, rgb};
use eframe::egui;
use std::{fs, io, path::Path};

pub enum ThemeAction {
    Import,
    Export,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
enum ThemeSource {
    #[default]
    Presets,
    Catppuccin,
    Custom,
}

#[derive(Clone, Default)]
pub struct ThemePicker {
    saved: Vec<SuiteTheme>,
    source: ThemeSource,
    observed: Option<SuiteTheme>,
    last_preset: Option<SuiteTheme>,
    last_catppuccin: Option<(usize, usize)>,
    last_custom: Option<SuiteTheme>,
    name: String,
    notice: String,
    load_failed: bool,
}

fn load_saved(path: &Path) -> io::Result<Vec<SuiteTheme>> {
    match fs::read(path) {
        Ok(bytes) => {
            let mut themes: Vec<SuiteTheme> = serde_json::from_slice(&bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            for theme in &mut themes {
                theme.refresh_catppuccin();
            }
            Ok(themes)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

fn save_named(path: &Path, theme: &SuiteTheme, name: &str) -> io::Result<Vec<SuiteTheme>> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 60 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Use a name of 1–60 characters.",
        ));
    }
    // Re-read so another Lapis window's saved themes are retained.
    let mut saved = load_saved(path)?;
    let mut theme = theme.clone();
    theme.name = name.to_owned();
    if let Some(old) = saved.iter_mut().find(|t| t.name == name) {
        *old = theme;
    } else {
        saved.push(theme);
    }
    saved.sort_by(|a, b| a.name.cmp(&b.name));
    let bytes = serde_json::to_vec_pretty(&saved).map_err(io::Error::other)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    // Preserve the previous collection if a write is interrupted.
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(&temporary, bytes)?;
    fs::rename(&temporary, path)?;
    Ok(saved)
}

impl ThemePicker {
    pub fn load() -> Self {
        match load_saved(&crate::paths::config_dir().join("custom_themes.json")) {
            Ok(saved) => Self {
                saved,
                ..Self::default()
            },
            Err(e) => Self {
                notice: format!("Could not load saved themes: {e}"),
                load_failed: true,
                ..Self::default()
            },
        }
    }

    fn source_for(&self, theme: &SuiteTheme) -> ThemeSource {
        if self.saved.iter().any(|saved| saved == theme) {
            ThemeSource::Custom
        } else if theme.catppuccin_choice().is_some() {
            ThemeSource::Catppuccin
        } else if SuiteTheme::dark_presets()
            .into_iter()
            .chain(SuiteTheme::light_presets())
            .any(|preset| preset == *theme)
        {
            ThemeSource::Presets
        } else {
            ThemeSource::Custom
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, theme: &mut SuiteTheme) -> Option<ThemeAction> {
        let before = theme.clone();
        let mut action = None;
        if self.observed.as_ref() != Some(theme) {
            self.source = self.source_for(theme);
        }
        match self.source {
            ThemeSource::Presets => self.last_preset = Some(theme.clone()),
            ThemeSource::Catppuccin => self.last_catppuccin = theme.catppuccin_choice(),
            ThemeSource::Custom => self.last_custom = Some(theme.clone()),
        }
        let old_source = self.source;
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.source, ThemeSource::Presets, "Presets");
            ui.selectable_value(&mut self.source, ThemeSource::Catppuccin, "Catppuccin");
            ui.selectable_value(&mut self.source, ThemeSource::Custom, "Custom");
        });
        if self.source != old_source {
            *theme = match self.source {
                ThemeSource::Presets => self.last_preset.clone().unwrap_or_default(),
                ThemeSource::Catppuccin => {
                    let (flavor, accent) = self.last_catppuccin.unwrap_or((3, 3));
                    SuiteTheme::catppuccin(flavor, accent)
                }
                ThemeSource::Custom => self.last_custom.clone().unwrap_or_else(|| {
                    let mut custom = theme.clone();
                    custom.name = "Custom (unsaved)".into();
                    custom
                }),
            };
        }
        ui.add_space(8.0);
        match self.source {
            ThemeSource::Presets => {
                egui::ComboBox::from_id_salt("theme_preset")
                    .selected_text(
                        egui::RichText::new(format!("{}  {}", theme.symbol(), theme.name))
                            .color(rgb(theme.accent)),
                    )
                    .width(280.0)
                    .show_ui(ui, |ui| {
                        for (label, presets) in [
                            ("Dark", SuiteTheme::dark_presets()),
                            ("Light", SuiteTheme::light_presets()),
                        ] {
                            ui.weak(label);
                            for preset in presets {
                                let selected = *theme == preset;
                                let button = egui::Button::new(
                                    egui::RichText::new(format!(
                                        "{}  {}",
                                        preset.symbol(),
                                        preset.name
                                    ))
                                    .color(rgb(preset.accent))
                                    .strong(),
                                )
                                .fill(rgb(preset.bg_widget))
                                .stroke(egui::Stroke::new(
                                    if selected { 2.0 } else { 1.0 },
                                    rgb(if selected {
                                        preset.accent
                                    } else {
                                        preset.border
                                    }),
                                ))
                                .min_size(egui::vec2(260.0, 28.0));
                                if ui.add(button).clicked() {
                                    *theme = preset;
                                    ui.close();
                                }
                            }
                            ui.separator();
                        }
                    });
            }
            ThemeSource::Catppuccin => {
                let (mut flavor, mut accent) = theme.catppuccin_choice().unwrap_or((3, 3));
                let mut selected = false;
                ui.horizontal(|ui| {
                    ui.label("Flavor");
                    egui::ComboBox::from_id_salt("catppuccin_flavor")
                        .width(115.0)
                        .selected_text(CATPPUCCIN_FLAVORS[flavor])
                        .show_ui(ui, |ui| {
                            for (i, label) in CATPPUCCIN_FLAVORS.iter().enumerate() {
                                selected |= ui.selectable_value(&mut flavor, i, *label).clicked();
                            }
                        });
                    ui.label("Accent");
                    egui::ComboBox::from_id_salt("catppuccin_accent")
                        .width(120.0)
                        .selected_text(CATPPUCCIN_ACCENTS[accent])
                        .show_ui(ui, |ui| {
                            for (i, label) in CATPPUCCIN_ACCENTS.iter().enumerate() {
                                ui.horizontal(|ui| {
                                    let color = SuiteTheme::catppuccin(flavor, i).accent;
                                    let (rect, _) = ui.allocate_exact_size(
                                        egui::vec2(12.0, 12.0),
                                        egui::Sense::hover(),
                                    );
                                    ui.painter().rect_filled(rect, 3.0, rgb(color));
                                    selected |=
                                        ui.selectable_value(&mut accent, i, *label).clicked();
                                });
                            }
                        });
                });
                if selected {
                    *theme = SuiteTheme::catppuccin(flavor, accent);
                }
            }
            ThemeSource::Custom => {
                ui.weak(format!("Active: {}", theme.name));
                if self.saved.is_empty() {
                    ui.weak("Customize the current palette below, then save a name.");
                }
            }
        }

        if self.source == ThemeSource::Custom && !self.saved.is_empty() {
            ui.add_space(8.0);
            egui::ComboBox::from_id_salt("saved_themes")
                .width(260.0)
                .selected_text("Saved custom themes…")
                .show_ui(ui, |ui| {
                    for saved in &self.saved {
                        if ui.selectable_label(*theme == *saved, &saved.name).clicked() {
                            *theme = saved.clone();
                            self.name = saved.name.clone();
                        }
                    }
                });
        }

        ui.add_space(8.0);
        let before_edit = theme.clone();
        egui::CollapsingHeader::new("Customize palette")
            .id_salt("customize_theme")
            .show(ui, |ui| {
                ui.weak("Editing switches to Custom. Save a named copy to keep it.");
                egui::Grid::new("theme_colors")
                    .num_columns(4)
                    .spacing([10.0, 7.0])
                    .show(ui, |ui| {
                        for (i, (label, color)) in [
                            ("Canvas", &mut theme.bg_canvas),
                            ("Panel", &mut theme.bg_panel),
                            ("Elevated", &mut theme.bg_elevated),
                            ("Widget", &mut theme.bg_widget),
                            ("Hover", &mut theme.bg_hover),
                            ("Pressed", &mut theme.bg_active),
                            ("Text", &mut theme.fg_primary),
                            ("Muted text", &mut theme.fg_muted),
                            ("Accent", &mut theme.accent),
                            ("Border", &mut theme.border),
                            ("Success", &mut theme.success),
                            ("Warning", &mut theme.warning),
                            ("Error", &mut theme.error),
                        ]
                        .into_iter()
                        .enumerate()
                        {
                            ui.label(label);
                            ui.color_edit_button_srgb(color);
                            if i % 2 == 1 {
                                ui.end_row();
                            }
                        }
                        ui.end_row();
                    });
                ui.add(
                    egui::Slider::new(&mut theme.corner_radius, 0..=14)
                        .text("Corner radius")
                        .suffix(" px"),
                );
            });

        if *theme != before_edit {
            theme.name = "Custom (unsaved)".to_owned();
            self.source = ThemeSource::Custom;
        }

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.name)
                    .hint_text("Custom theme name")
                    .desired_width(210.0)
                    .char_limit(60),
            );
            let exists = self.saved.iter().any(|t| t.name == self.name.trim());
            let label = if exists {
                "Update saved theme"
            } else {
                "Save custom theme"
            };
            if ui
                .add_enabled(
                    !self.load_failed && !self.name.trim().is_empty(),
                    egui::Button::new(label),
                )
                .clicked()
            {
                match save_named(
                    &crate::paths::config_dir().join("custom_themes.json"),
                    theme,
                    &self.name,
                ) {
                    Ok(saved) => {
                        self.saved = saved;
                        self.source = ThemeSource::Custom;
                        theme.name = self.name.trim().to_owned();
                        self.notice = format!("Saved theme: {}", theme.name);
                    }
                    Err(e) => self.notice = format!("Could not save theme: {e}"),
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            if ui.small_button("Import…").clicked() {
                action = Some(ThemeAction::Import);
            }
            if ui.small_button("Export…").clicked() {
                action = Some(ThemeAction::Export);
            }
            if ui.small_button("Reset default").clicked() {
                *theme = SuiteTheme::default();
                self.source = ThemeSource::Presets;
            }
        });

        if *theme != before {
            theme.apply(ui.ctx());
            if let Err(e) = theme.save() {
                self.notice = format!("Could not persist active theme: {e}");
            }
        }
        self.observed = Some(theme.clone());
        if !self.notice.is_empty() {
            ui.small(&self.notice);
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_sources_are_exclusive_and_saved_names_remain_custom() {
        let mut picker = ThemePicker::default();
        assert_eq!(
            picker.source_for(&SuiteTheme::default()),
            ThemeSource::Presets
        );
        let mut theme = SuiteTheme::catppuccin(3, 3);
        assert_eq!(picker.source_for(&theme), ThemeSource::Catppuccin);
        theme.name = "My Mocha".into();
        picker.saved.push(theme.clone());
        assert_eq!(picker.source_for(&theme), ThemeSource::Custom);
        theme.bg_canvas = [1, 2, 3];
        assert_eq!(picker.source_for(&theme), ThemeSource::Custom);
    }

    #[test]
    fn save_reload_update_and_reject_corrupt_collections() {
        let dir = std::env::temp_dir().join(format!(
            "lapis-theme-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("custom_themes.json");
        let first = SuiteTheme::catppuccin(3, 3);
        let second = SuiteTheme::catppuccin(0, 12);
        save_named(&path, &first, " My theme ").unwrap();
        save_named(&path, &second, "Another theme").unwrap();
        let loaded = load_saved(&path).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[1].accent, first.accent);
        save_named(&path, &second, "My theme").unwrap();
        let loaded = load_saved(&path).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[1].accent, second.accent);
        assert!(save_named(&path, &first, " ").is_err());
        fs::write(&path, b"invalid JSON").unwrap();
        assert!(save_named(&path, &first, "New").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"invalid JSON");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn color_popup_sits_above_modal_and_escape_closes_popup_first() {
        use egui::*;
        let ctx = Context::default();
        let mut color = [120, 150, 200];
        let mut button = Rect::NOTHING;
        let mut closed = false;
        let mut frame = |events: Vec<Event>| {
            let mut output = ctx.run_ui(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 800.0))),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let ctx = ui.ctx();
                    let response = Modal::new(Id::unique("settings")).show(ctx, |ui| {
                        button = ui.color_edit_button_srgb(&mut color).rect;
                    });
                    closed = response.should_close();
                },
            );
            output.textures_delta.clear();
            (button, closed)
        };
        frame(vec![]);
        let (button, _) = frame(vec![]);
        let pos = button.center();
        for pressed in [true, false] {
            frame(vec![
                Event::PointerMoved(pos),
                Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                },
            ]);
        }
        assert!(Popup::is_any_open(&ctx));
        // The picker opens below the swatch, on a child popup layer.
        frame(vec![]);
        let layer = ctx.layer_id_at(pos + vec2(10.0, 35.0)).unwrap();
        assert_ne!(layer.id, Id::unique("settings"));
        assert_eq!(layer.order, Order::Foreground);
        let (_, closed) = frame(vec![Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }]);
        assert!(!closed);
        assert!(!Popup::is_any_open(&ctx));
        frame(vec![Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: Modifiers::NONE,
        }]);
        let (_, closed) = frame(vec![Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }]);
        assert!(closed);
    }
}

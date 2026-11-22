use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use eframe::egui;
use serde::{Deserialize, Serialize};

/// Shared theme configuration across the entire application suite (PDF viewer, Text Editor, etc.).
///
/// All colors are stored as RGB triples so themes remain portable in `theme.json`.
/// `#[serde(default)]` keeps older theme files loadable when new fields are added.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SuiteTheme {
    pub name: String,
    pub bg_canvas: [u8; 3],
    pub bg_panel: [u8; 3],
    pub bg_elevated: [u8; 3],
    pub bg_widget: [u8; 3],
    pub bg_hover: [u8; 3],
    pub bg_active: [u8; 3],
    pub fg_primary: [u8; 3],
    pub fg_muted: [u8; 3],
    pub accent: [u8; 3],
    pub border: [u8; 3],
    pub success: [u8; 3],
    pub warning: [u8; 3],
    pub error: [u8; 3],
    pub corner_radius: u8,
    pub button_padding: [f32; 2],
}

impl Default for SuiteTheme {
    fn default() -> Self {
        Self::suite_dark()
    }
}

#[inline]
pub fn rgb(c: [u8; 3]) -> egui::Color32 {
    egui::Color32::from_rgb(c[0], c[1], c[2])
}

/// Selected text must override emphasis and explicit label colors.
pub fn selected_button(
    label: impl Into<egui::RichText>,
    selected: bool,
    visuals: &egui::Visuals,
) -> egui::Button<'static> {
    let label = label.into();
    let label = if selected {
        label.color(visuals.selection.stroke.color)
    } else {
        label
    };
    egui::Button::new(label).selected(selected)
}

impl SuiteTheme {
    fn palette(
        name: &str,
        bg_canvas: [u8; 3],
        bg_panel: [u8; 3],
        bg_elevated: [u8; 3],
        bg_widget: [u8; 3],
        bg_hover: [u8; 3],
        bg_active: [u8; 3],
        fg_primary: [u8; 3],
        fg_muted: [u8; 3],
        accent: [u8; 3],
        border: [u8; 3],
        success: [u8; 3],
        warning: [u8; 3],
        error: [u8; 3],
    ) -> Self {
        Self {
            name: name.to_string(),
            bg_canvas,
            bg_panel,
            bg_elevated,
            bg_widget,
            bg_hover: readable_color(bg_hover, fg_primary, 4.5),
            bg_active: readable_color(bg_active, fg_primary, 4.5),
            fg_primary,
            fg_muted: readable_color(fg_muted, bg_panel, 4.5),
            accent,
            border,
            success,
            warning,
            error,
            corner_radius: 7,
            button_padding: [8.0, 4.0],
        }
    }

    pub fn suite_dark() -> Self {
        Self::palette(
            "Suite Dark",
            [17, 18, 22],
            [24, 25, 32],
            [30, 32, 41],
            [36, 39, 50],
            [48, 53, 69],
            [61, 69, 91],
            [231, 234, 242],
            [151, 158, 176],
            [125, 156, 255],
            [58, 63, 81],
            [111, 214, 177],
            [245, 190, 105],
            [247, 118, 142],
        )
    }

    pub fn indigo() -> Self {
        Self::palette(
            "Indigo",
            [14, 17, 30],
            [21, 26, 45],
            [29, 36, 61],
            [37, 46, 76],
            [49, 62, 101],
            [61, 78, 128],
            [232, 237, 255],
            [153, 166, 202],
            [129, 140, 248],
            [55, 66, 105],
            [110, 231, 183],
            [251, 191, 36],
            [251, 113, 133],
        )
    }

    pub fn emerald() -> Self {
        Self::palette(
            "Emerald",
            [12, 23, 22],
            [18, 34, 32],
            [24, 45, 41],
            [31, 57, 51],
            [40, 76, 65],
            [49, 94, 78],
            [225, 248, 240],
            [147, 181, 166],
            [52, 211, 153],
            [45, 78, 67],
            [74, 222, 128],
            [250, 204, 21],
            [251, 113, 133],
        )
    }

    pub fn cyber_violet() -> Self {
        Self::palette(
            "Cyber Violet",
            [22, 15, 31],
            [32, 22, 45],
            [43, 29, 60],
            [55, 36, 76],
            [74, 47, 102],
            [93, 57, 128],
            [246, 236, 255],
            [184, 160, 204],
            [192, 132, 252],
            [76, 53, 98],
            [110, 231, 183],
            [250, 204, 21],
            [251, 113, 133],
        )
    }

    pub fn sunset_amber() -> Self {
        Self::palette(
            "Sunset Amber",
            [29, 20, 17],
            [42, 28, 23],
            [56, 36, 28],
            [70, 45, 34],
            [91, 56, 39],
            [112, 68, 43],
            [255, 241, 224],
            [201, 166, 139],
            [251, 191, 36],
            [94, 62, 43],
            [134, 239, 172],
            [253, 186, 116],
            [251, 113, 133],
        )
    }

    pub fn blood_moon() -> Self {
        Self::palette(
            "Blood Moon",
            [25, 13, 18],
            [37, 19, 26],
            [50, 25, 34],
            [64, 31, 42],
            [83, 39, 52],
            [101, 46, 62],
            [255, 235, 237],
            [201, 155, 164],
            [255, 79, 105],
            [91, 43, 57],
            [112, 218, 153],
            [255, 190, 92],
            [255, 91, 105],
        )
    }

    pub fn cyanic_teal() -> Self {
        Self::palette(
            "Cyanic Teal",
            [12, 24, 29],
            [18, 35, 42],
            [24, 47, 55],
            [30, 59, 68],
            [38, 79, 90],
            [45, 98, 111],
            [225, 248, 252],
            [145, 181, 190],
            [45, 212, 191],
            [43, 78, 88],
            [134, 239, 172],
            [250, 204, 21],
            [251, 113, 133],
        )
    }

    pub fn graphite() -> Self {
        Self::palette(
            "Graphite",
            [17, 18, 19],    // canvas
            [25, 26, 27],    // panel
            [33, 34, 35],    // elevated
            [42, 43, 44],    // widget
            [53, 54, 55],    // hover
            [64, 65, 66],    // active
            [245, 246, 247], // primary text
            [169, 172, 175], // muted text
            [190, 198, 207], // silver accent
            [72, 74, 77],    // border
            [112, 205, 153], // success
            [235, 190, 105], // warning
            [240, 105, 115], // error
        )
    }

    pub fn forest() -> Self {
        Self::palette(
            "Forest",
            [15, 24, 19],
            [22, 34, 27],
            [29, 44, 34],
            [36, 54, 41],
            [46, 69, 51],
            [56, 82, 60],
            [231, 242, 226],
            [157, 180, 151],
            [150, 190, 105],
            [51, 72, 53],
            [117, 204, 133],
            [231, 190, 95],
            [235, 112, 112],
        )
    }

    pub fn plum() -> Self {
        Self::palette(
            "Plum",
            [27, 18, 25],
            [39, 26, 36],
            [51, 34, 47],
            [63, 42, 58],
            [79, 53, 73],
            [94, 63, 87],
            [248, 234, 243],
            [191, 161, 181],
            [221, 125, 177],
            [83, 53, 74],
            [116, 211, 163],
            [239, 190, 100],
            [245, 112, 127],
        )
    }

    pub fn paper_light() -> Self {
        Self::palette(
            "Paper",
            [248, 247, 243],
            [238, 236, 229],
            [255, 255, 252],
            [230, 228, 219],
            [219, 216, 204],
            [207, 203, 188],
            [42, 43, 38],
            [105, 104, 94],
            [133, 105, 62],
            [205, 201, 187],
            [38, 132, 86],
            [173, 112, 35],
            [184, 57, 65],
        )
    }

    pub fn warm_light() -> Self {
        Self::palette(
            "Warm Sand",
            [250, 244, 233],
            [240, 230, 214],
            [255, 250, 241],
            [234, 220, 199],
            [224, 205, 177],
            [211, 187, 153],
            [63, 46, 34],
            [124, 99, 77],
            [171, 100, 48],
            [213, 194, 168],
            [48, 132, 88],
            [179, 112, 38],
            [184, 58, 58],
        )
    }

    pub fn sage_light() -> Self {
        Self::palette(
            "Sage",
            [242, 246, 239],
            [229, 236, 224],
            [253, 255, 250],
            [218, 229, 213],
            [203, 219, 195],
            [187, 207, 177],
            [39, 53, 40],
            [96, 117, 96],
            [75, 126, 79],
            [194, 208, 187],
            [39, 130, 83],
            [170, 115, 35],
            [182, 55, 63],
        )
    }

    pub fn molten() -> Self {
        Self::palette(
            "Molten",
            [29, 14, 12],
            [43, 20, 16],
            [57, 27, 20],
            [72, 34, 23],
            [94, 43, 25],
            [116, 51, 26],
            [255, 239, 226],
            [207, 159, 139],
            [255, 105, 55],
            [100, 48, 31],
            [116, 218, 145],
            [255, 193, 79],
            [255, 104, 104],
        )
    }

    pub fn royal_gold() -> Self {
        Self::palette(
            "Royal Gold",
            [25, 21, 14],
            [37, 31, 19],
            [49, 41, 24],
            [62, 51, 29],
            [80, 65, 34],
            [98, 79, 39],
            [255, 247, 220],
            [195, 177, 132],
            [255, 205, 72],
            [83, 68, 39],
            [124, 219, 150],
            [255, 211, 99],
            [244, 112, 112],
        )
    }

    /// Twelve dark palettes, ordered roughly along a chromatic arc.
    pub fn dark_presets() -> Vec<Self> {
        vec![
            Self::suite_dark(),
            Self::graphite(),
            Self::blood_moon(),
            Self::sunset_amber(),
            Self::molten(),
            Self::royal_gold(),
            Self::forest(),
            Self::emerald(),
            Self::cyanic_teal(),
            Self::indigo(),
            Self::cyber_violet(),
            Self::plum(),
        ]
    }

    pub fn porcelain_light() -> Self {
        Self::palette(
            "Porcelain",
            [246, 247, 250],
            [235, 238, 244],
            [255, 255, 255],
            [225, 230, 239],
            [211, 219, 232],
            [196, 207, 224],
            [35, 42, 56],
            [100, 111, 130],
            [92, 112, 174],
            [196, 207, 222],
            [35, 145, 94],
            [177, 117, 35],
            [190, 55, 68],
        )
    }

    pub fn blush_light() -> Self {
        Self::palette(
            "Blush",
            [252, 242, 243],
            [244, 226, 229],
            [255, 250, 250],
            [239, 211, 216],
            [229, 191, 199],
            [217, 170, 181],
            [69, 38, 47],
            [133, 91, 101],
            [190, 76, 112],
            [222, 190, 197],
            [40, 133, 88],
            [173, 111, 35],
            [183, 53, 66],
        )
    }

    pub fn lavender_light() -> Self {
        Self::palette(
            "Lavender",
            [247, 243, 252],
            [235, 226, 246],
            [255, 252, 255],
            [226, 212, 240],
            [212, 194, 231],
            [198, 176, 220],
            [53, 39, 69],
            [115, 96, 139],
            [133, 88, 190],
            [207, 190, 224],
            [39, 135, 89],
            [174, 113, 37],
            [188, 57, 72],
        )
    }

    /// Six light palettes, ordered by hue.
    pub fn light_presets() -> Vec<Self> {
        vec![
            Self::paper_light(),
            Self::warm_light(),
            Self::blush_light(),
            Self::sage_light(),
            Self::porcelain_light(),
            Self::lavender_light(),
        ]
    }

    /*pub fn presets() -> Vec<Self> {
        let mut themes = Self::dark_presets();
        themes.extend(Self::light_presets());
        themes
    }*/

    /// Compact decorative symbol for the theme-picker buttons.
    pub fn symbol(&self) -> &'static str {
        match self.name.as_str() {
            // DARK THEMES
            "Suite Dark" => "󰇀",
            "Graphite" => "󰏪",
            "Blood Moon" => "󰧻",
            "Sunset Amber" => "󰖨",
            "Molten" => "",
            "Royal Gold" => "",
            "Forest" => "󱢗",
            "Emerald" => "󰣏",
            "Cyanic Teal" => "",
            "Indigo" => "󰊠",
            "Cyber Violet" => "󰊖",
            "Plum" => "󰉊",

            // LIGHT THEMES
            "Paper" => "",
            "Warm Sand" => "󰓠",
            "Blush" => "󰋑",
            "Birch" => "󰔱",
            "Porcelain" => "󰋥",
            "Lavender" => "󰴈",

            _ => "◆",
        }
    }

    /// Returns `~/.config/lapis/theme.json` (respecting `XDG_CONFIG_HOME`).
    pub fn default_config_path() -> PathBuf {
        crate::paths::config_dir().join("theme.json")
    }

    pub fn save(&self) -> io::Result<()> {
        let path = Self::default_config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        self.save_to_file(&path)
    }

    /// Loads a theme from `theme.json`, or creates the default one.
    pub fn load_or_init() -> Self {
        let path = Self::default_config_path();
        if let Ok(content) = fs::read_to_string(&path)
            && let Ok(mut theme) = serde_json::from_str::<Self>(&content)
        {
            theme.refresh_catppuccin();
            return theme;
        }

        let theme = Self::default();
        let _ = theme.save_to_file(&path);
        theme
    }

    pub fn load_from_file(path: &Path) -> io::Result<Self> {
        let content = fs::read_to_string(path)?;
        let mut theme = serde_json::from_str::<Self>(&content)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        theme.refresh_catppuccin();
        Ok(theme)
    }

    pub fn save_to_file(&self, path: &Path) -> io::Result<()> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        fs::write(path, json)
    }

    fn is_light(&self) -> bool {
        // Perceived luminance: enough to choose egui's light/dark widget conventions.
        let [r, g, b] = self.bg_canvas;
        (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32) > 145.0
    }

    /// Applies the full palette to egui. Both egui theme slots receive the same
    /// selected palette so the OS theme cannot unexpectedly override the user's choice.
    pub fn apply(&self, ctx: &egui::Context) {
        let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
        let radius = egui::CornerRadius::same(self.corner_radius);
        let is_light = self.is_light();

        // Keep controls compact across the app while respecting themes that request
        // even tighter spacing.
        style.spacing.button_padding = egui::vec2(
            self.button_padding[0].min(6.0),
            self.button_padding[1].min(3.0),
        );
        style.spacing.item_spacing = egui::vec2(5.0, 4.0);

        let accent = rgb(self.accent);
        let fg_primary = rgb(self.fg_primary);
        let fg_muted = rgb(self.fg_muted);
        let border = rgb(self.border);
        let bg_panel = rgb(self.bg_panel);
        let bg_widget = rgb(self.bg_widget);
        let bg_hover = rgb(self.bg_hover);
        let bg_active = rgb(self.bg_active);

        let v = &mut style.visuals;
        v.dark_mode = !is_light;
        v.override_text_color = None;
        v.weak_text_color = Some(fg_muted);
        v.panel_fill = bg_panel;
        v.window_fill = rgb(self.bg_elevated);
        v.extreme_bg_color = rgb(self.bg_canvas);
        v.faint_bg_color = bg_widget;
        v.code_bg_color = rgb(self.bg_elevated);
        v.hyperlink_color = rgb(readable_color(self.accent, self.bg_panel, 4.5));
        v.window_corner_radius = egui::CornerRadius::same(self.corner_radius.saturating_add(2));
        v.menu_corner_radius = egui::CornerRadius::same(self.corner_radius.saturating_add(1));
        v.window_stroke = egui::Stroke::new(1.0, border);

        v.selection.bg_fill = rgb(self.selection_fill());
        v.selection.stroke = egui::Stroke::new(1.5, self.on_accent());

        v.widgets.noninteractive.bg_fill = bg_panel;
        v.widgets.noninteractive.weak_bg_fill = bg_panel;
        v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, border);
        v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, fg_primary);
        v.widgets.noninteractive.corner_radius = radius;
        v.widgets.noninteractive.expansion = 0.0;

        // Middle ground between frameless controls and heavy button boxes: a light
        // surface tint and hairline edge at rest, with a clear filled hover state.
        let idle_button_fill =
            egui::Color32::from_rgba_unmultiplied(bg_widget.r(), bg_widget.g(), bg_widget.b(), 112);
        let idle_button_border =
            egui::Color32::from_rgba_unmultiplied(border.r(), border.g(), border.b(), 128);
        v.widgets.inactive.bg_fill = idle_button_fill;
        v.widgets.inactive.weak_bg_fill = idle_button_fill;
        v.widgets.inactive.bg_stroke = egui::Stroke::new(0.8, idle_button_border);
        v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, fg_primary);
        v.widgets.inactive.corner_radius = radius;
        v.widgets.inactive.expansion = 0.0;

        v.widgets.hovered.bg_fill = bg_hover;
        v.widgets.hovered.weak_bg_fill = bg_hover;
        v.widgets.hovered.bg_stroke = egui::Stroke::new(0.8, accent);
        v.widgets.hovered.fg_stroke = egui::Stroke::new(
            1.0,
            rgb(readable_color(self.fg_primary, self.bg_hover, 4.5)),
        );
        v.widgets.hovered.corner_radius = radius;
        v.widgets.hovered.expansion = 0.0;

        v.widgets.active.bg_fill = bg_active;
        v.widgets.active.weak_bg_fill = bg_active;
        v.widgets.active.bg_stroke = egui::Stroke::new(0.8, accent);
        v.widgets.active.fg_stroke = egui::Stroke::new(
            1.0,
            rgb(readable_color(self.fg_primary, self.bg_active, 4.5)),
        );
        v.widgets.active.corner_radius = radius;
        v.widgets.active.expansion = 0.0;

        v.widgets.open.bg_fill = bg_active;
        v.widgets.open.weak_bg_fill = bg_active;
        v.widgets.open.bg_stroke = egui::Stroke::new(0.8, border);
        v.widgets.open.fg_stroke = egui::Stroke::new(
            1.0,
            rgb(readable_color(self.fg_primary, self.bg_active, 4.5)),
        );
        v.widgets.open.corner_radius = radius;
        v.widgets.open.expansion = 0.0;

        v.text_cursor.stroke = egui::Stroke::new(2.0, accent); // color and thickness

        ctx.set_style_of(egui::Theme::Dark, style.clone());
        ctx.set_style_of(egui::Theme::Light, style);
    }
}

impl SuiteTheme {
    /// Applies the interface density on top of [`SuiteTheme::apply`]: spacing, margins, widget
    /// sizes and text sizes. Standard keeps the theme's own spacing and egui's default sizes.
    pub fn apply_density(&self, ctx: &egui::Context, density: crate::state::UiDensity) {
        let m = density.global();
        let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
        if density != crate::state::UiDensity::Standard {
            style.spacing.item_spacing = egui::vec2(m.item_spacing[0], m.item_spacing[1]);
            style.spacing.button_padding = egui::vec2(
                self.button_padding[0].min(m.pad[0]),
                self.button_padding[1].min(m.pad[1]),
            );
        }
        style.spacing.interact_size.y = m.interact_y;
        style.spacing.window_margin = egui::Margin::same(m.window_margin);
        style.spacing.menu_margin = egui::Margin::same(m.menu_margin);
        style.spacing.indent = m.indent;
        style.spacing.icon_width = m.icon_width;
        style.spacing.icon_width_inner = m.icon_inner;
        // Scale from egui's defaults so repeated calls never compound.
        for (text_style, base) in egui::Style::default().text_styles {
            if let Some(font) = style.text_styles.get_mut(&text_style) {
                font.size = base.size * m.font_scale;
            }
        }
        ctx.set_style_of(egui::Theme::Dark, style.clone());
        ctx.set_style_of(egui::Theme::Light, style);
    }

    /// Whether the current style already carries `density` (a theme change resets the spacing).
    pub fn density_applied(&self, ctx: &egui::Context, density: crate::state::UiDensity) -> bool {
        let m = density.global();
        let style = ctx.style_of(egui::Theme::Dark);
        let item = if density == crate::state::UiDensity::Standard {
            egui::vec2(5.0, 4.0)
        } else {
            egui::vec2(m.item_spacing[0], m.item_spacing[1])
        };
        style.spacing.item_spacing == item && style.spacing.indent == m.indent
    }
}

fn luminance(color: [u8; 3]) -> f32 {
    let linear = color.map(|c| {
        let v = c as f32 / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    });
    linear[0] * 0.2126 + linear[1] * 0.7152 + linear[2] * 0.0722
}

fn contrast(a: [u8; 3], b: [u8; 3]) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn mix(a: [u8; 3], b: [u8; 3], amount: f32) -> [u8; 3] {
    std::array::from_fn(|i| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * amount).round() as u8)
}

/// Keep the hue where possible, moving toward the higher-contrast endpoint.
fn readable_color(color: [u8; 3], against: [u8; 3], minimum: f32) -> [u8; 3] {
    let target = if contrast([0; 3], against) > contrast([255; 3], against) {
        [0; 3]
    } else {
        [255; 3]
    };
    for step in 0..=100 {
        let candidate = mix(color, target, step as f32 / 100.0);
        if contrast(candidate, against) >= minimum {
            return candidate;
        }
    }
    target
}

impl SuiteTheme {
    pub fn selection_fill(&self) -> [u8; 3] {
        self.accent
    }

    /// Text for controls painted with the unmodified accent.
    pub fn on_accent(&self) -> egui::Color32 {
        if self.bg_canvas != [0; 3]
            && self.bg_canvas != [255; 3]
            && contrast(self.bg_canvas, self.accent) >= 4.5
        {
            return rgb(self.bg_canvas);
        }
        let candidates = [
            self.bg_canvas,
            self.bg_panel,
            self.bg_elevated,
            self.bg_widget,
            self.fg_primary,
            self.fg_muted,
        ];
        let best = candidates
            .into_iter()
            .filter(|color| *color != [0; 3] && *color != [255; 3])
            .max_by(|a, b| contrast(*a, self.accent).total_cmp(&contrast(*b, self.accent)))
            .unwrap_or(self.fg_primary);
        // Preserve a palette tone, adjusting its brightness only if necessary.
        rgb(readable_color(best, self.accent, 4.5))
    }
}

pub const CATPPUCCIN_FLAVORS: [&str; 4] = ["Latte", "Frappe", "Macchiato", "Mocha"];
pub const CATPPUCCIN_ACCENTS: [&str; 14] = [
    "Rosewater",
    "Flamingo",
    "Pink",
    "Mauve",
    "Red",
    "Maroon",
    "Peach",
    "Yellow",
    "Green",
    "Teal",
    "Sky",
    "Sapphire",
    "Blue",
    "Lavender",
];

impl SuiteTheme {
    /// Official Catppuccin palette: https://github.com/catppuccin/palette
    pub fn catppuccin(flavor: usize, accent: usize) -> Self {
        let flavor = flavor.min(3);
        let accent = accent.min(13);
        let accents = [
            [
                [220, 138, 120],
                [221, 120, 120],
                [234, 118, 203],
                [136, 57, 239],
                [210, 15, 57],
                [230, 69, 83],
                [254, 100, 11],
                [223, 142, 29],
                [64, 160, 43],
                [23, 146, 153],
                [4, 165, 229],
                [32, 159, 181],
                [30, 102, 245],
                [114, 135, 253],
            ],
            [
                [242, 213, 207],
                [238, 190, 190],
                [244, 184, 228],
                [202, 158, 230],
                [231, 130, 132],
                [234, 153, 156],
                [239, 159, 118],
                [229, 200, 144],
                [166, 209, 137],
                [129, 200, 190],
                [153, 209, 219],
                [133, 193, 220],
                [140, 170, 238],
                [186, 187, 241],
            ],
            [
                [244, 219, 214],
                [240, 198, 198],
                [245, 189, 230],
                [198, 160, 246],
                [237, 135, 150],
                [238, 153, 160],
                [245, 169, 127],
                [238, 212, 159],
                [166, 218, 149],
                [139, 213, 202],
                [145, 215, 227],
                [125, 196, 228],
                [138, 173, 244],
                [183, 189, 248],
            ],
            [
                [245, 224, 220],
                [242, 205, 205],
                [245, 194, 231],
                [203, 166, 247],
                [243, 139, 168],
                [235, 160, 172],
                [250, 179, 135],
                [249, 226, 175],
                [166, 227, 161],
                [148, 226, 213],
                [137, 220, 235],
                [116, 199, 236],
                [137, 180, 250],
                [180, 190, 254],
            ],
        ];
        let surfaces = [
            [
                [239, 241, 245],
                [230, 233, 239],
                [204, 208, 218],
                [188, 192, 204],
                [172, 176, 190],
                [76, 79, 105],
                [92, 95, 119],
                [64, 160, 43],
                [223, 142, 29],
                [210, 15, 57],
            ],
            [
                [48, 52, 70],
                [41, 44, 60],
                [65, 69, 89],
                [81, 87, 109],
                [98, 104, 128],
                [198, 208, 245],
                [181, 191, 226],
                [166, 209, 137],
                [229, 200, 144],
                [231, 130, 132],
            ],
            [
                [36, 39, 58],
                [30, 32, 48],
                [54, 58, 79],
                [73, 77, 100],
                [91, 96, 120],
                [202, 211, 245],
                [184, 192, 224],
                [166, 218, 149],
                [238, 212, 159],
                [237, 135, 150],
            ],
            [
                [30, 30, 46],
                [24, 24, 37],
                [49, 50, 68],
                [69, 71, 90],
                [88, 91, 112],
                [205, 214, 244],
                [186, 194, 222],
                [166, 227, 161],
                [249, 226, 175],
                [243, 139, 168],
            ],
        ];
        let [
            base,
            mantle,
            surface0,
            surface1,
            surface2,
            text,
            muted,
            green,
            yellow,
            red,
        ] = surfaces[flavor];
        let mut theme = Self::palette(
            &format!(
                "Catppuccin {} · {}",
                CATPPUCCIN_FLAVORS[flavor], CATPPUCCIN_ACCENTS[accent]
            ),
            base,
            mantle,
            base,
            surface0,
            surface1,
            surface2,
            text,
            muted,
            accents[flavor][accent],
            surface1,
            green,
            yellow,
            red,
        );
        // Keep Catppuccin surfaces exact; adjust widget text instead.
        theme.bg_hover = surface1;
        theme.bg_active = surface2;
        theme.fg_muted = muted;
        theme
    }

    /// Refresh only exact built-ins, including the earlier adjusted surfaces.
    pub fn refresh_catppuccin(&mut self) {
        if let Some((flavor, accent)) = self.catppuccin_choice() {
            let name = self.name.clone();
            *self = Self::catppuccin(flavor, accent);
            self.name = name;
        }
    }

    pub fn catppuccin_choice(&self) -> Option<(usize, usize)> {
        for flavor in 0..4 {
            for accent in 0..14 {
                let mut candidate = Self::catppuccin(flavor, accent);
                candidate.name = self.name.clone();
                if *self == candidate {
                    return Some((flavor, accent));
                }
                // Recognize palettes saved by the first theme-settings patch.
                candidate.bg_hover = readable_color(candidate.bg_hover, candidate.fg_primary, 4.5);
                candidate.bg_active =
                    readable_color(candidate.bg_active, candidate.fg_primary, 4.5);
                candidate.fg_muted = readable_color(candidate.fg_muted, candidate.bg_panel, 4.5);
                if *self == candidate {
                    return Some((flavor, accent));
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palettes_keep_text_readable_in_widget_states() {
        let mut themes = SuiteTheme::dark_presets();
        themes.extend(SuiteTheme::light_presets());
        for flavor in 0..4 {
            for accent in 0..14 {
                themes.push(SuiteTheme::catppuccin(flavor, accent));
            }
        }
        for theme in themes {
            for surface in [theme.bg_panel, theme.bg_widget] {
                assert!(
                    contrast(theme.fg_primary, surface) >= 4.5,
                    "{}: {:?}",
                    theme.name,
                    surface
                );
            }
            let ctx = egui::Context::default();
            theme.apply(&ctx);
            let style = ctx.style_of(egui::Theme::Dark);
            for widget in [
                &style.visuals.widgets.hovered,
                &style.visuals.widgets.active,
            ] {
                let text = widget.fg_stroke.color.to_array();
                let bg = widget.bg_fill.to_array();
                assert!(contrast([text[0], text[1], text[2]], [bg[0], bg[1], bg[2]]) >= 4.5);
            }
            assert_eq!(theme.selection_fill(), theme.accent);
            assert!(
                contrast(theme.fg_muted, theme.bg_panel) >= 4.5,
                "{} muted",
                theme.name
            );
            let text = theme.on_accent().to_array();
            assert!(
                contrast([text[0], text[1], text[2]], theme.accent) >= 4.5,
                "{} accent",
                theme.name
            );
        }
    }

    #[test]
    fn catppuccin_choices_round_trip_even_with_custom_names() {
        for flavor in 0..4 {
            for accent in 0..14 {
                let mut theme = SuiteTheme::catppuccin(flavor, accent);
                assert_eq!(theme.catppuccin_choice(), Some((flavor, accent)));
                theme.name = "My theme".into();
                let decoded: SuiteTheme =
                    serde_json::from_str(&serde_json::to_string(&theme).unwrap()).unwrap();
                assert_eq!(decoded.catppuccin_choice(), Some((flavor, accent)));
            }
        }
        assert_eq!(SuiteTheme::catppuccin(0, 12).accent, [30, 102, 245]);
        assert_eq!(SuiteTheme::catppuccin(3, 3).accent, [203, 166, 247]);
    }

    #[test]
    fn selected_buttons_paint_the_full_accent_with_contrasting_text() {
        for flavor in 0..4 {
            for accent in 0..14 {
                let theme = SuiteTheme::catppuccin(flavor, accent);
                let ctx = egui::Context::default();
                theme.apply(&ctx);
                let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                    ui.visuals_mut().override_text_color = Some(egui::Color32::WHITE);
                    for label in [
                        egui::RichText::new("Selected"),
                        egui::RichText::new("Edit").strong(),
                        egui::RichText::new("Preview")
                            .strong()
                            .color(egui::Color32::WHITE),
                        egui::RichText::new("Sketch").size(11.0).strong(),
                        egui::RichText::new("Bold").strong(),
                        egui::RichText::new("Italic").italics(),
                    ] {
                        ui.add(selected_button(label, true, ui.visuals()));
                    }
                });
                output.textures_delta.clear();
                for label in ["Selected", "Edit", "Preview", "Sketch", "Bold", "Italic"] {
                    let text = output
                        .shapes
                        .iter()
                        .find_map(|s| match &s.shape {
                            egui::epaint::Shape::Text(t) if t.galley.job.text == label => Some(t),
                            _ => None,
                        })
                        .expect("selected button text");
                    let color = text.galley.job.sections[0].format.color;
                    let color = text.override_text_color.unwrap_or(
                        if color == egui::Color32::PLACEHOLDER {
                            text.fallback_color
                        } else {
                            color
                        },
                    );
                    assert_eq!(color, theme.on_accent(), "{label}: {}", theme.name);
                }
                assert!(output.shapes.iter().any(|s| matches!(&s.shape,
                    egui::epaint::Shape::Rect(r) if r.fill == rgb(theme.accent))));
                output.textures_delta.clear();
            }
        }
    }

    #[test]
    fn accent_text_uses_theme_tones_instead_of_black_or_white() {
        let mut themes = SuiteTheme::dark_presets();
        themes.extend(SuiteTheme::light_presets());
        for flavor in 0..4 {
            for accent in 0..14 {
                themes.push(SuiteTheme::catppuccin(flavor, accent));
            }
        }
        for theme in themes {
            let color = theme.on_accent();
            assert_ne!(color, egui::Color32::BLACK, "{}", theme.name);
            assert_ne!(color, egui::Color32::WHITE, "{}", theme.name);
            if contrast(theme.bg_canvas, theme.accent) >= 4.5 {
                assert_eq!(color, rgb(theme.bg_canvas));
            }
        }
        assert_eq!(SuiteTheme::catppuccin(3, 3).on_accent(), rgb([30, 30, 46]));
        assert_eq!(
            SuiteTheme::catppuccin(0, 3).on_accent(),
            rgb([239, 241, 245])
        );
    }

    #[test]
    fn old_catppuccin_surfaces_refresh_without_overwriting_custom_colors() {
        let canonical = SuiteTheme::catppuccin(1, 3);
        let mut old = canonical.clone();
        old.bg_active = readable_color(old.bg_active, old.fg_primary, 4.5);
        old.refresh_catppuccin();
        assert_eq!(old, canonical);
        let mut custom = canonical;
        custom.bg_canvas = [12, 14, 16];
        let original = custom.clone();
        custom.refresh_catppuccin();
        assert_eq!(custom, original);
    }

    #[test]
    fn old_theme_json_loads_and_custom_selection_stays_readable() {
        let theme: SuiteTheme =
            serde_json::from_str(r#"{"name":"Legacy","accent":[250,240,100]}"#).unwrap();
        assert_eq!(theme.name, "Legacy");
        assert_eq!(theme.selection_fill(), theme.accent);
        let ctx = egui::Context::default();
        theme.apply(&ctx);
        for mode in [egui::Theme::Light, egui::Theme::Dark] {
            assert_eq!(
                ctx.style_of(mode).visuals.selection.bg_fill,
                rgb(theme.selection_fill())
            );
        }
    }
}

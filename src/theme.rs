use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use eframe::egui;
use serde::{Deserialize, Serialize};

/// Shared theme configuration across the entire application suite (PDF viewer, Text Editor, etc.).
///
/// All colors are stored as RGB triples so themes remain portable in `theme.json`.
/// `#[serde(default)]` keeps older theme files loadable when new fields are added.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
            bg_hover,
            bg_active,
            fg_primary,
            fg_muted,
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
            [17, 18, 22], [24, 25, 32], [30, 32, 41],
            [36, 39, 50], [48, 53, 69], [61, 69, 91],
            [231, 234, 242], [151, 158, 176], [125, 156, 255],
            [58, 63, 81], [111, 214, 177], [245, 190, 105], [247, 118, 142],
        )
    }

    pub fn indigo() -> Self {
        Self::palette(
            "Indigo",
            [14, 17, 30], [21, 26, 45], [29, 36, 61],
            [37, 46, 76], [49, 62, 101], [61, 78, 128],
            [232, 237, 255], [153, 166, 202], [129, 140, 248],
            [55, 66, 105], [110, 231, 183], [251, 191, 36], [251, 113, 133],
        )
    }

    pub fn emerald() -> Self {
        Self::palette(
            "Emerald",
            [12, 23, 22], [18, 34, 32], [24, 45, 41],
            [31, 57, 51], [40, 76, 65], [49, 94, 78],
            [225, 248, 240], [147, 181, 166], [52, 211, 153],
            [45, 78, 67], [74, 222, 128], [250, 204, 21], [251, 113, 133],
        )
    }

    pub fn cyber_violet() -> Self {
        Self::palette(
            "Cyber Violet",
            [22, 15, 31], [32, 22, 45], [43, 29, 60],
            [55, 36, 76], [74, 47, 102], [93, 57, 128],
            [246, 236, 255], [184, 160, 204], [192, 132, 252],
            [76, 53, 98], [110, 231, 183], [250, 204, 21], [251, 113, 133],
        )
    }

    pub fn sunset_amber() -> Self {
        Self::palette(
            "Sunset Amber",
            [29, 20, 17], [42, 28, 23], [56, 36, 28],
            [70, 45, 34], [91, 56, 39], [112, 68, 43],
            [255, 241, 224], [201, 166, 139], [251, 191, 36],
            [94, 62, 43], [134, 239, 172], [253, 186, 116], [251, 113, 133],
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
            [12, 24, 29], [18, 35, 42], [24, 47, 55],
            [30, 59, 68], [38, 79, 90], [45, 98, 111],
            [225, 248, 252], [145, 181, 190], [45, 212, 191],
            [43, 78, 88], [134, 239, 172], [250, 204, 21], [251, 113, 133],
        )
    }

pub fn graphite() -> Self {
    Self::palette(
        "Graphite",
        [17, 18, 19],       // canvas
        [25, 26, 27],       // panel
        [33, 34, 35],       // elevated
        [42, 43, 44],       // widget
        [53, 54, 55],       // hover
        [64, 65, 66],       // active
        [245, 246, 247],    // primary text
        [169, 172, 175],    // muted text
        [190, 198, 207],    // silver accent
        [72, 74, 77],       // border
        [112, 205, 153],    // success
        [235, 190, 105],    // warning
        [240, 105, 115],    // error
    )
}

    pub fn forest() -> Self {
        Self::palette("Forest", [15, 24, 19], [22, 34, 27], [29, 44, 34], [36, 54, 41], [46, 69, 51], [56, 82, 60], [231, 242, 226], [157, 180, 151], [150, 190, 105], [51, 72, 53], [117, 204, 133], [231, 190, 95], [235, 112, 112])
    }

    pub fn plum() -> Self {
        Self::palette("Plum", [27, 18, 25], [39, 26, 36], [51, 34, 47], [63, 42, 58], [79, 53, 73], [94, 63, 87], [248, 234, 243], [191, 161, 181], [221, 125, 177], [83, 53, 74], [116, 211, 163], [239, 190, 100], [245, 112, 127])
    }

    pub fn paper_light() -> Self {
        Self::palette("Paper", [248, 247, 243], [238, 236, 229], [255, 255, 252], [230, 228, 219], [219, 216, 204], [207, 203, 188], [42, 43, 38], [105, 104, 94], [133, 105, 62], [205, 201, 187], [38, 132, 86], [173, 112, 35], [184, 57, 65])
    }

    pub fn warm_light() -> Self {
        Self::palette("Warm Sand", [250, 244, 233], [240, 230, 214], [255, 250, 241], [234, 220, 199], [224, 205, 177], [211, 187, 153], [63, 46, 34], [124, 99, 77], [171, 100, 48], [213, 194, 168], [48, 132, 88], [179, 112, 38], [184, 58, 58])
    }

    pub fn sage_light() -> Self {
        Self::palette("Sage", [242, 246, 239], [229, 236, 224], [253, 255, 250], [218, 229, 213], [203, 219, 195], [187, 207, 177], [39, 53, 40], [96, 117, 96], [75, 126, 79], [194, 208, 187], [39, 130, 83], [170, 115, 35], [182, 55, 63])
    }

    pub fn molten() -> Self {
        Self::palette("Molten", [29, 14, 12], [43, 20, 16], [57, 27, 20], [72, 34, 23], [94, 43, 25], [116, 51, 26], [255, 239, 226], [207, 159, 139], [255, 105, 55], [100, 48, 31], [116, 218, 145], [255, 193, 79], [255, 104, 104])
    }

    pub fn royal_gold() -> Self {
        Self::palette("Royal Gold", [25, 21, 14], [37, 31, 19], [49, 41, 24], [62, 51, 29], [80, 65, 34], [98, 79, 39], [255, 247, 220], [195, 177, 132], [255, 205, 72], [83, 68, 39], [124, 219, 150], [255, 211, 99], [244, 112, 112])
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
        Self::palette("Porcelain", [246, 247, 250], [235, 238, 244], [255, 255, 255], [225, 230, 239], [211, 219, 232], [196, 207, 224], [35, 42, 56], [100, 111, 130], [92, 112, 174], [196, 207, 222], [35, 145, 94], [177, 117, 35], [190, 55, 68])
    }

    pub fn blush_light() -> Self {
        Self::palette("Blush", [252, 242, 243], [244, 226, 229], [255, 250, 250], [239, 211, 216], [229, 191, 199], [217, 170, 181], [69, 38, 47], [133, 91, 101], [190, 76, 112], [222, 190, 197], [40, 133, 88], [173, 111, 35], [183, 53, 66])
    }

    pub fn lavender_light() -> Self {
        Self::palette("Lavender", [247, 243, 252], [235, 226, 246], [255, 252, 255], [226, 212, 240], [212, 194, 231], [198, 176, 220], [53, 39, 69], [115, 96, 139], [133, 88, 190], [207, 190, 224], [39, 135, 89], [174, 113, 37], [188, 57, 72])
    }

    /// Six light palettes, ordered by hue.
    pub fn light_presets() -> Vec<Self> {
        vec![Self::paper_light(), Self::warm_light(), Self::blush_light(), Self::sage_light(), Self::porcelain_light(), Self::lavender_light()]
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
            "Suite Dark"   => "󰇀",
            "Graphite"  => "󰏪",
            "Blood Moon"   => "󰧻",
            "Sunset Amber" => "󰖨",
            "Molten"       => "",
            "Royal Gold"   => "",
            "Forest"       => "󱢗",
            "Emerald"      => "󰣏",
            "Cyanic Teal"  => "",
            "Indigo"       => "󰊠",
            "Cyber Violet" => "󰊖",
            "Plum"         => "󰉊",

            // LIGHT THEMES
            "Paper"     => "",
            "Warm Sand"    => "󰓠",
            "Blush"        => "󰋑",
            "Birch"         => "󰔱",
            "Porcelain"    => "󰋥",
            "Lavender"     => "󰴈",

            _              => "◆",
        }
    }

    /// Returns `~/.config/lapis/theme.json` (respecting `XDG_CONFIG_HOME`).
    pub fn default_config_path() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."));

        let dir = base.join("lapis");
        let _ = fs::create_dir_all(&dir);
        dir.join("theme.json")
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
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(theme) = serde_json::from_str::<Self>(&content) {
                return theme;
            }
        }

        let theme = Self::default();
        let _ = theme.save_to_file(&path);
        theme
    }

    pub fn load_from_file(path: &Path) -> io::Result<Self> {
        let content = fs::read_to_string(path)?;
        serde_json::from_str::<Self>(&content)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
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

        style.spacing.button_padding =
            egui::vec2(self.button_padding[0], self.button_padding[1]);
        style.spacing.item_spacing = egui::vec2(6.0, 6.0);

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
        v.override_text_color = Some(fg_primary);
        v.weak_text_color = Some(fg_muted);
        v.panel_fill = bg_panel;
        v.window_fill = rgb(self.bg_elevated);
        v.extreme_bg_color = rgb(self.bg_canvas);
        v.faint_bg_color = bg_widget;
        v.code_bg_color = rgb(self.bg_elevated);
        v.hyperlink_color = accent;
        v.window_corner_radius = egui::CornerRadius::same(self.corner_radius.saturating_add(2));
        v.menu_corner_radius = egui::CornerRadius::same(self.corner_radius.saturating_add(1));
        v.window_stroke = egui::Stroke::new(1.0, border);

        v.selection.bg_fill = accent;
        v.selection.stroke = egui::Stroke::new(
            1.5,
            if is_light { egui::Color32::WHITE } else { fg_primary },
        );

        v.widgets.noninteractive.bg_fill = bg_panel;
        v.widgets.noninteractive.weak_bg_fill = bg_panel;
        v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, border);
        v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, fg_primary);
        v.widgets.noninteractive.corner_radius = radius;

        v.widgets.inactive.bg_fill = bg_widget;
        v.widgets.inactive.weak_bg_fill = bg_widget;
        v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, border);
        v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, fg_primary);
        v.widgets.inactive.corner_radius = radius;

        v.widgets.hovered.bg_fill = bg_hover;
        v.widgets.hovered.weak_bg_fill = bg_hover;
        v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, accent);
        v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, fg_primary);
        v.widgets.hovered.corner_radius = radius;
        v.widgets.hovered.expansion = 0.0;

        v.widgets.active.bg_fill = bg_active;
        v.widgets.active.weak_bg_fill = bg_active;
        v.widgets.active.bg_stroke = egui::Stroke::new(1.0, accent);
        v.widgets.active.fg_stroke = egui::Stroke::new(1.0, fg_primary);
        v.widgets.active.corner_radius = radius;
        v.widgets.active.expansion = 0.0;

        v.widgets.open.bg_fill = bg_active;
        v.widgets.open.weak_bg_fill = bg_active;
        v.widgets.open.bg_stroke = egui::Stroke::new(1.0, border);
        v.widgets.open.fg_stroke = egui::Stroke::new(1.0, fg_primary);
        v.widgets.open.corner_radius = radius;

        ctx.set_style_of(egui::Theme::Dark, style.clone());
        ctx.set_style_of(egui::Theme::Light, style);
    }
}

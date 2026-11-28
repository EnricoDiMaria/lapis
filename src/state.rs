use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use crate::shortcuts::ShortcutsConfig;
use crate::workspace::{ViewMode, ZoomMode};

const MAX_RECENT_FILES: usize = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SlideshowEmptyMode {
    /// 1. Keeps the previous annotated slide's note visible (zero layout shifts)
    AutoHold,
    /// 2. Shows Whisper audio transcript & preview of the next upcoming note (zero layout shifts)
    ContextDashboard,
    /// 3. Automatically skips unannotated slides when navigating with arrows in Slideshow
    SkipEmpty,
    /// 4. Hides the right panel on empty slides so the slide expands to 100% fullscreen
    ExpandFull,
    /// 5. Keeps the right panel fixed and displays "No notes for this slide."
    #[default]
    StaticEmpty,
}

/// What the exported PDF contains.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ExportContent {
    /// The original pages with the drawn annotations on top (no notes).
    Annotations,
    /// Pages with notes get the "slide | notes" layout; no annotations are drawn.
    Notes,
    /// Annotations and notes.
    #[default]
    AnnotationsAndNotes,
}

impl ExportContent {
    pub const ALL: [ExportContent; 3] = [
        ExportContent::AnnotationsAndNotes,
        ExportContent::Annotations,
        ExportContent::Notes,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ExportContent::Annotations => "Annotations only",
            ExportContent::Notes => "Notes only",
            ExportContent::AnnotationsAndNotes => "Annotations + notes",
        }
    }

    /// Short text for the FILE menu button.
    pub fn short_label(self) -> &'static str {
        match self {
            ExportContent::Annotations => "annot.",
            ExportContent::Notes => "notes",
            ExportContent::AnnotationsAndNotes => "annot. + notes",
        }
    }

    pub fn file_suffix(self) -> &'static str {
        match self {
            ExportContent::Annotations => "_annotated",
            ExportContent::Notes => "_notes",
            ExportContent::AnnotationsAndNotes => "_annotated_notes",
        }
    }

    pub fn includes_annotations(self) -> bool {
        matches!(
            self,
            ExportContent::Annotations | ExportContent::AnnotationsAndNotes
        )
    }

    pub fn includes_notes(self) -> bool {
        matches!(
            self,
            ExportContent::Notes | ExportContent::AnnotationsAndNotes
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportSettings {
    /// Base font size in points for exported notes (valid range: 12.0..=32.0)
    pub base_font_size: f32,
    /// Percentage of the page width occupied by the slide when notes are present (valid range: 40.0..=80.0)
    pub slide_width_pct: f32,
    /// What to export: annotations, notes or both.
    pub content: ExportContent,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            base_font_size: 20.0,
            slide_width_pct: 75.0,
            content: ExportContent::AnnotationsAndNotes,
        }
    }
}

impl ExportSettings {
    pub fn clamp_valid(&mut self) {
        self.base_font_size = self.base_font_size.clamp(12.0, 32.0);
        self.slide_width_pct = self.slide_width_pct.clamp(40.0, 80.0);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookmarkItem {
    pub title: String,
    pub page: u32,
}

/// Notes panels whose size is remembered per document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelSlot {
    Right,
    Bottom,
    Slideshow,
}

/// Saved panel sizes in points. `None` keeps the default size.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelSizes {
    pub right: Option<f32>,
    pub bottom: Option<f32>,
    pub slideshow: Option<f32>,
}

impl PanelSizes {
    pub fn get(&self, slot: PanelSlot) -> Option<f32> {
        match slot {
            PanelSlot::Right => self.right,
            PanelSlot::Bottom => self.bottom,
            PanelSlot::Slideshow => self.slideshow,
        }
    }

    pub fn set(&mut self, slot: PanelSlot, size: f32) {
        match slot {
            PanelSlot::Right => self.right = Some(size),
            PanelSlot::Bottom => self.bottom = Some(size),
            PanelSlot::Slideshow => self.slideshow = Some(size),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentFileEntry {
    pub path: PathBuf,
    pub name: String,
    pub last_page: u32,
    pub total_pages: u32,
    pub zoom_mode: ZoomMode,
    pub zoom_scale: f32,
    pub view_mode: ViewMode,
    pub show_notes: bool,
    pub show_toc: bool,
    #[serde(default)]
    pub bookmarks: Vec<BookmarkItem>,
    pub hidden_pages: HashSet<u32>,
    #[serde(default)]
    pub pdf_dark_mode: bool,
    #[serde(default)]
    pub panel_sizes: PanelSizes,
}

/// How tightly the whole interface is packed: bars, panels, spacing and text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub enum UiDensity {
    #[default]
    Standard,
    Compact,
    Ultra,
}

impl<'de> Deserialize<'de> for UiDensity {
    /// Also reads the former `compact_topbar` boolean (`true` meant Compact).
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Flag(bool),
            Name(String),
        }
        Ok(match Repr::deserialize(deserializer)? {
            Repr::Flag(true) => Self::Compact,
            Repr::Flag(false) => Self::Standard,
            Repr::Name(name) => match name.as_str() {
                "Compact" => Self::Compact,
                "Ultra" => Self::Ultra,
                _ => Self::Standard,
            },
        })
    }
}

/// Where the topbar is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TopbarPosition {
    #[default]
    Top,
    Bottom,
}

/// Main topbar sizes.
pub struct BarMetrics {
    pub row_h: f32,
    pub margin_y: i8,
    pub pad_y: f32,
    pub dot_radius: f32,
    pub dot_inset: f32,
}

/// Annotation sub-topbar sizes.
pub struct SubBarMetrics {
    pub height: f32,
    pub margin_y: i8,
    pub spacing: f32,
    pub pad_y: f32,
    pub btn: [f32; 2],
    pub icon: f32,
    pub swatch_box: f32,
    pub swatch_fill: f32,
    pub swatch_ring: f32,
    pub shape_btn_h: f32,
}

/// Annotation vertical rail sizes.
pub struct RailMetrics {
    pub width: f32,
    pub margin: [i8; 2],
    pub spacing: f32,
    pub pad_y: f32,
    pub btn: [f32; 2],
    pub icon: f32,
}

/// egui-wide spacing and text sizes. Standard matches egui's own defaults.
pub struct GlobalMetrics {
    pub item_spacing: [f32; 2],
    /// Upper bounds for the theme's button padding.
    pub pad: [f32; 2],
    pub interact_y: f32,
    pub window_margin: i8,
    pub menu_margin: i8,
    pub indent: f32,
    pub icon_width: f32,
    pub icon_inner: f32,
    pub font_scale: f32,
}

impl UiDensity {
    /// Factor for fixed sizes in panels (button heights, margins).
    pub fn scale(self) -> f32 {
        match self {
            Self::Standard => 1.0,
            Self::Compact => 0.88,
            Self::Ultra => 0.78,
        }
    }

    pub fn global(self) -> GlobalMetrics {
        let (item_spacing, pad, interact_y, window_margin, menu_margin) = match self {
            Self::Standard => ([5.0, 4.0], [6.0, 3.0], 18.0, 6, 6),
            Self::Compact => ([4.0, 3.0], [5.0, 2.0], 17.0, 5, 4),
            Self::Ultra => ([3.0, 2.0], [4.0, 1.0], 15.0, 4, 3),
        };
        let (indent, icon_width, icon_inner, font_scale) = match self {
            Self::Standard => (18.0, 14.0, 8.0, 1.0),
            Self::Compact => (15.0, 13.0, 7.0, 0.93),
            Self::Ultra => (12.0, 12.0, 6.5, 0.86),
        };
        GlobalMetrics {
            item_spacing,
            pad,
            interact_y,
            window_margin,
            menu_margin,
            indent,
            icon_width,
            icon_inner,
            font_scale,
        }
    }

    pub fn bar(self) -> BarMetrics {
        let (row_h, margin_y, pad_y, dot_radius, dot_inset) = match self {
            Self::Standard => (27.0, 3, 3.0, 3.5, 2.0),
            Self::Compact => (22.0, 2, 1.0, 3.0, 2.5),
            Self::Ultra => (20.0, 1, 0.0, 2.5, 2.5),
        };
        BarMetrics { row_h, margin_y, pad_y, dot_radius, dot_inset }
    }

    pub fn sub_bar(self) -> SubBarMetrics {
        match self {
            Self::Standard => SubBarMetrics {
                height: 36.0, margin_y: 4, spacing: 4.0, pad_y: 3.0, btn: [28.0, 26.0], icon: 14.5,
                swatch_box: 20.0, swatch_fill: 7.5, swatch_ring: 9.0, shape_btn_h: 24.0,
            },
            Self::Compact => SubBarMetrics {
                height: 28.0, margin_y: 2, spacing: 3.0, pad_y: 1.0, btn: [26.0, 22.0], icon: 13.5,
                swatch_box: 18.0, swatch_fill: 6.5, swatch_ring: 8.0, shape_btn_h: 20.0,
            },
            Self::Ultra => SubBarMetrics {
                height: 24.0, margin_y: 1, spacing: 2.0, pad_y: 0.0, btn: [24.0, 20.0], icon: 12.5,
                swatch_box: 16.0, swatch_fill: 5.5, swatch_ring: 7.0, shape_btn_h: 18.0,
            },
        }
    }

    pub fn rail(self) -> RailMetrics {
        match self {
            Self::Standard => RailMetrics {
                width: 44.0, margin: [5, 8], spacing: 5.0, pad_y: 3.0, btn: [32.0, 30.0], icon: 15.5,
            },
            Self::Compact => RailMetrics {
                width: 40.0, margin: [4, 5], spacing: 3.0, pad_y: 1.0, btn: [30.0, 25.0], icon: 14.5,
            },
            Self::Ultra => RailMetrics {
                width: 36.0, margin: [3, 4], spacing: 2.0, pad_y: 0.0, btn: [28.0, 22.0], icon: 13.5,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ToolsLayoutMode {
    VerticalRail, // Floating vertical pill on the right edge
    SubTopbar,    // Horizontal annotation toolbar
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    /// Font size in the Notes Editor & Preview (default: 14.0 pt)
    pub note_font_size: f32,
    /// Optional path to a user-loaded .ttf / .otf font file
    pub custom_font_path: Option<PathBuf>,
    /// Installed Hunspell dictionary pairs (stored by .aff path), or legacy .txt lists
    pub extra_dictionaries: Vec<PathBuf>,
    pub spellcheck_enabled: bool,
    /// The bundled dictionary can be disabled without uninstalling it.
    pub english_dictionary_enabled: bool,
    /// Installed dictionaries omitted from spellchecking; files are retained.
    pub disabled_dictionaries: Vec<PathBuf>,
    pub slideshow_empty_mode: SlideshowEmptyMode,
    pub tools_layout: ToolsLayoutMode,
    /// None keeps the horizontal toolbar on the topbar edge.
    pub annotation_bar_position: Option<TopbarPosition>,
    pub tools_button: bool,
    /// Return to Cursor / Select when the annotation bar is closed.
    pub reset_tool_on_bar_close: bool,
    pub radial_menu_enabled: bool,
    pub flash_button: bool,
    /// Show the document-wide flashcard indicator dot on the toolbar.
    pub flash_notification_dot: bool,
    pub restore_show_notes: bool,
    pub restore_show_toc: bool,
    pub audio_dock: bool,
    pub audio_notes: bool,
    pub bottom_notes: bool,
    pub default_page_style: crate::notes::BlankPageStyle,
    /// Show the per-page Sketch tab in the notes panel (and include sketches in exports).
    pub sketch_enabled: bool,
    /// Density of the whole interface (`compact_topbar` was the former boolean key).
    #[serde(alias = "compact_topbar", alias = "topbar_density")]
    pub ui_density: UiDensity,
    /// Hide the topbar; reveal it by hovering the top edge or with the shortcut.
    pub topbar_auto_hide: bool,
    /// Top or bottom edge of the window.
    pub topbar_position: TopbarPosition,
    /// Multiplier for mouse-wheel and trackpad scrolling, the same in every page view.
    pub wheel_scroll_speed: f32,
    /// Page movement per pixel of middle-button dragging, the same in every page view.
    pub pan_scroll_speed: f32,
}

/// Default mouse-wheel scroll multiplier (the former Continuous Scroll value).
pub const DEFAULT_WHEEL_SCROLL_SPEED: f32 = 2.2;
/// Default middle-button drag multiplier (between the former 3.5 and 4.8).
pub const DEFAULT_PAN_SCROLL_SPEED: f32 = 4.0;

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            note_font_size: 14.0,
            custom_font_path: None,
            extra_dictionaries: Vec::new(),
            spellcheck_enabled: true,
            english_dictionary_enabled: true,
            disabled_dictionaries: Vec::new(),
            slideshow_empty_mode: SlideshowEmptyMode::StaticEmpty,
            tools_layout: ToolsLayoutMode::SubTopbar,
            annotation_bar_position: None,
            tools_button: true,
            reset_tool_on_bar_close: true,
            radial_menu_enabled: true,
            flash_button: true,
            flash_notification_dot: true,
            restore_show_notes: true,
            restore_show_toc: true,
            audio_dock: true,
            audio_notes: true,
            bottom_notes: true,
            default_page_style: crate::notes::BlankPageStyle::Ruled,
            sketch_enabled: true,
            ui_density: UiDensity::Standard,
            topbar_auto_hide: false,
            topbar_position: TopbarPosition::Top,
            wheel_scroll_speed: DEFAULT_WHEEL_SCROLL_SPEED,
            pan_scroll_speed: DEFAULT_PAN_SCROLL_SPEED,
        }
    }
}

impl AppSettings {
    pub fn annotation_bar_edge(&self) -> TopbarPosition {
        self.annotation_bar_position.unwrap_or(self.topbar_position)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppState {
    pub recent_files: VecDeque<RecentFileEntry>,
    #[serde(default = "default_home_grid_view")]
    pub home_grid_view: bool,
    pub export_settings: ExportSettings,
    pub shortcuts: ShortcutsConfig,
    pub settings: AppSettings,
    #[serde(default = "default_annotation_colors")]
    pub annotation_colors: [[u8; 3]; 6],
}

fn default_annotation_colors() -> [[u8; 3]; 6] {
    [
        [37, 99, 235], // Pen
        [250, 204, 21], // Highlighter
        [17, 24, 39], // Textbox
        [17, 24, 39], // Underline
        [17, 24, 39], // Shapes
        [17, 24, 39], // Tape
    ]
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            recent_files: VecDeque::new(),
            home_grid_view: false,
            export_settings: ExportSettings::default(),
            shortcuts: ShortcutsConfig::default(),
            settings: AppSettings::default(),
            annotation_colors: default_annotation_colors(),
        }
    }
}

fn default_home_grid_view() -> bool {
    true
}

impl AppState {
    pub fn config_path() -> PathBuf {
        crate::paths::config_dir().join("state.json")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        if let Ok(content) = fs::read_to_string(&path) {
            match serde_json::from_str::<Self>(&content) {
                Ok(mut state) => {
                    state.shortcuts.migrate_layout();
                    // Filter out files that were deleted or moved from disk
                    state.recent_files.retain(|entry| entry.path.exists());
                    return state;
                }
                // Keep the unreadable file: the next save would otherwise overwrite it.
                Err(_) => { crate::paths::quarantine_corrupt(&path); }
            }
        }
        Self {
            home_grid_view: true,
            ..Self::default()
        }
    }

    pub fn save(&self) {
        let path = Self::config_path();
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = crate::paths::write_atomic(&path, json.as_bytes());
        }
    }

    pub fn find_entry(&self, pdf_path: &Path) -> Option<&RecentFileEntry> {
        let canonical = dunce::canonicalize(pdf_path).unwrap_or_else(|_| pdf_path.to_path_buf());
        self.recent_files
            .iter()
            .find(|e| e.path == canonical || e.path == pdf_path)
    }

    pub fn upsert_recent(&mut self, mut entry: RecentFileEntry) {
        if entry.path.as_os_str().is_empty() {
            return;
        }
        if let Ok(canon) = dunce::canonicalize(&entry.path) {
            entry.path = canon;
        }
        self.recent_files.retain(|e| e.path != entry.path);
        self.recent_files.push_front(entry);
        self.recent_files.truncate(MAX_RECENT_FILES);
        self.save();
    }

    pub fn remove_recent(&mut self, path: &Path) {
        self.recent_files.retain(|e| e.path != path);
        self.save();
    }
}

#[cfg(test)]
mod ui_density_tests {
    use super::*;

    #[test]
    fn reads_the_former_keys_and_new_names() {
        let read = |json: &str| serde_json::from_str::<AppSettings>(json).unwrap().ui_density;
        assert_eq!(read(r#"{"compact_topbar": true}"#), UiDensity::Compact);
        assert_eq!(read(r#"{"compact_topbar": false}"#), UiDensity::Standard);
        assert_eq!(read(r#"{"topbar_density": "Ultra"}"#), UiDensity::Ultra);
        assert_eq!(read(r#"{"ui_density": "Compact"}"#), UiDensity::Compact);
        assert_eq!(read("{}"), UiDensity::Standard);
    }

    #[test]
    fn density_round_trips() {
        let settings = AppSettings {
            ui_density: UiDensity::Ultra,
            ..AppSettings::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        let back: AppSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.ui_density, UiDensity::Ultra);
    }

    #[test]
    fn denser_levels_are_strictly_smaller() {
        use UiDensity::*;
        assert!(Standard.bar().row_h > Compact.bar().row_h);
        assert!(Compact.bar().row_h > Ultra.bar().row_h);
        assert!(Standard.sub_bar().height > Compact.sub_bar().height);
        assert!(Compact.sub_bar().height > Ultra.sub_bar().height);
        assert!(Standard.rail().btn[1] > Compact.rail().btn[1]);
        assert!(Compact.rail().btn[1] > Ultra.rail().btn[1]);
        assert!(Standard.scale() > Compact.scale() && Compact.scale() > Ultra.scale());
        assert!(Standard.global().item_spacing[1] > Compact.global().item_spacing[1]);
        assert!(Compact.global().item_spacing[1] > Ultra.global().item_spacing[1]);
        assert!(Standard.global().font_scale > Compact.global().font_scale);
        assert!(Compact.global().font_scale > Ultra.global().font_scale);
        // Each button fits inside its bar after the margins.
        for d in [Standard, Compact, Ultra] {
            let s = d.sub_bar();
            assert!(s.btn[1] + 2.0 * f32::from(s.margin_y) <= s.height);
        }
    }
}

#[cfg(test)]
mod scroll_speed_tests {
    use super::*;

    #[test]
    fn older_settings_get_the_default_speeds() {
        let settings: AppSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings.wheel_scroll_speed, DEFAULT_WHEEL_SCROLL_SPEED);
        assert_eq!(settings.pan_scroll_speed, DEFAULT_PAN_SCROLL_SPEED);
    }

    #[test]
    fn saved_speeds_round_trip() {
        let settings = AppSettings {
            wheel_scroll_speed: 3.5,
            pan_scroll_speed: 1.0,
            ..AppSettings::default()
        };
        let back: AppSettings = serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert_eq!((back.wheel_scroll_speed, back.pan_scroll_speed), (3.5, 1.0));
    }
}

#[cfg(test)]
mod topbar_position_tests {
    use super::*;

    #[test]
    fn annotation_bar_position_is_independent_and_saved() {
        let mut settings: AppSettings = serde_json::from_str(r#"{"topbar_position":"Bottom"}"#).unwrap();
        assert_eq!(settings.annotation_bar_edge(), TopbarPosition::Bottom);
        settings.annotation_bar_position = Some(TopbarPosition::Top);
        let mut back: AppSettings = serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert_eq!(back.annotation_bar_edge(), TopbarPosition::Top);
        back.topbar_position = TopbarPosition::Top;
        back.annotation_bar_position = Some(TopbarPosition::Bottom);
        assert_eq!(back.annotation_bar_edge(), TopbarPosition::Bottom);
    }

    #[test]
    fn the_topbar_defaults_to_the_top_and_the_choice_is_saved() {
        let settings: AppSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings.topbar_position, TopbarPosition::Top);
        let moved = AppSettings {
            topbar_position: TopbarPosition::Bottom,
            ..AppSettings::default()
        };
        let back: AppSettings = serde_json::from_str(&serde_json::to_string(&moved).unwrap()).unwrap();
        assert_eq!(back.topbar_position, TopbarPosition::Bottom);
    }
}

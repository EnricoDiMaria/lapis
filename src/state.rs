use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};


use crate::ZoomMode;
use crate::ViewMode;

const MAX_RECENT_FILES: usize = 12;

 
/// The time window within which the second button of a combo must follow the first.
/// Increase it if the combo feels too "tight"; decrease it if it triggers accidentally.
pub const CHORD_WINDOW_SECS: f64 = 0.45;
 
struct KeyTracker {
    frame_time: f64,
    last_press: HashMap<egui::Key, f64>,
    last_ctrl: f64,
    last_alt: f64,
    last_shift: f64,
}
 
static KEY_TRACKER: OnceLock<Mutex<KeyTracker>> = OnceLock::new();
 
fn key_tracker() -> &'static Mutex<KeyTracker> {
    KEY_TRACKER.get_or_init(|| {
        Mutex::new(KeyTracker {
            frame_time: -1.0,
            last_press: HashMap::new(),
            last_ctrl: f64::NEG_INFINITY,
            last_alt: f64::NEG_INFINITY,
            last_shift: f64::NEG_INFINITY,
        })
    })
}
 
/// Records the keys pressed during this frame. It is idempotent (can be called only once per frame),
/// so you may call it multiple times.
///
/// IMPORTANT: It must be executed BEFORE any keyboard events are consumed within the frame.
/// `ShortcutBinding::consume` calls it automatically; however, if you have code that consumes
/// key presses prior to any `consume` call, you should invoke it yourself at the start of your
/// input handling logic:
///
///      ctx.input(|i| state::track_key_presses(i));
pub fn track_key_presses(i: &egui::InputState) {
    let Ok(mut t) = key_tracker().lock() else {
        return;
    };
    if t.frame_time == i.time {
        return;
    }
    t.frame_time = i.time;
 
    for ev in &i.events {
        if let egui::Event::Key {
            key,
            pressed: true,
            repeat: false,
            ..
        } = ev
        {
            t.last_press.insert(*key, i.time);
        }
    }
    if i.modifiers.ctrl || i.modifiers.command {
        t.last_ctrl = i.time;
    }
    if i.modifiers.alt {
        t.last_alt = i.time;
    }
    if i.modifiers.shift {
        t.last_shift = i.time;
    }
 
    let cutoff = i.time - 5.0;
    t.last_press.retain(|_, ts| *ts >= cutoff);
}


#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
    StaticEmpty,
}

impl Default for SlideshowEmptyMode {
    fn default() -> Self {
        Self::StaticEmpty
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportSettings {
    /// Base font size in points for exported notes (valid range: 12.0..=32.0)
    pub base_font_size: f32,
    /// Percentage of the 16:9 canvas width occupied by the slide when notes are present (valid range: 40.0..=80.0)
    pub slide_width_pct: f32,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            base_font_size: 20.0,
            slide_width_pct: 63.0,
        }
    }
}

impl ExportSettings {
    pub fn clamp_valid(&mut self) {
        self.base_font_size = self.base_font_size.clamp(12.0, 32.0);
        self.slide_width_pct = self.slide_width_pct.clamp(40.0, 80.0);
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShortcutBinding {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: String,
    #[serde(default)]
    pub extra_key: Option<String>,
}

impl ShortcutBinding {
    pub fn new(ctrl: bool, alt: bool, shift: bool, key: &str) -> Self {
        Self {
            ctrl,
            alt,
            shift,
            key: key.to_string(),
            extra_key: None,
        }
    }

    pub fn format_label(&self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.shift {
            parts.push("Shift");
        }
        if let Some(ek) = &self.extra_key {
            parts.push(ek.as_str());
        }
        parts.push(&self.key);
        parts.join(" + ")
    }

    pub fn to_egui_key(&self) -> Option<egui::Key> {
        egui::Key::from_name(&self.key)
    }

    
pub fn consume(&self, i: &mut egui::InputState) -> bool {
    track_key_presses(i);
 
    let Some(k) = self.to_egui_key() else {
        return false;
    };
 
    if let Some(ek_name) = &self.extra_key {
        return self.consume_chord(i, k, ek_name);
    }
 
    let mut mods = egui::Modifiers::NONE;
    if self.ctrl {
        mods |= egui::Modifiers::COMMAND;
    }
    if self.alt {
        mods |= egui::Modifiers::ALT;
    }
    if self.shift {
        mods |= egui::Modifiers::SHIFT;
    }
    i.consume_shortcut(&egui::KeyboardShortcut::new(mods, k))
    }

    fn consume_chord(&self, i: &mut egui::InputState, k: egui::Key, ek_name: &str) -> bool {
    let Some(ek) = egui::Key::from_name(ek_name) else {
        return false;
    };
    if ek == k {
        return false;
    }
 
    let pressed_now = i.events.iter().any(|ev| {
        matches!(
            ev,
            egui::Event::Key { key, pressed: true, repeat: false, .. } if *key == k
        )
    });
    if !pressed_now {
        return false;
    }
 
    let now = i.time;
    let held_extra = i.key_down(ek);
    let (cur_ctrl, cur_alt, cur_shift) = (
        i.modifiers.ctrl || i.modifiers.command,
        i.modifiers.alt,
        i.modifiers.shift,
    );
 
    let ok = {
        let Ok(t) = key_tracker().lock() else {
            return false;
        };
        let recent = |ts: f64| now - ts <= CHORD_WINDOW_SECS;
 
        let extra_ok = held_extra || t.last_press.get(&ek).map(|&ts| recent(ts)).unwrap_or(false);
        // I modificatori valgono se tenuti adesso OPPURE se lo erano da poco
        let ctrl_ok = !self.ctrl || cur_ctrl || recent(t.last_ctrl);
        let alt_ok = !self.alt || cur_alt || recent(t.last_alt);
        let shift_ok = !self.shift || cur_shift || recent(t.last_shift);
        extra_ok && ctrl_ok && alt_ok && shift_ok
    };
    if !ok {
        return false;
    }
 
    i.events.retain(|ev| {
        !matches!(
            ev,
            egui::Event::Key { key, pressed: true, .. } if *key == k
        )
    });
    if let Ok(mut t) = key_tracker().lock() {
        t.last_press.remove(&ek);
        t.last_press.remove(&k);
    }
    true
    }


    pub fn is_held(&self, i: &egui::InputState) -> bool {
    let Some(k) = self.to_egui_key() else {
        return false;
    };
    let ctrl_ok = !self.ctrl || i.modifiers.ctrl || i.modifiers.command;
    let alt_ok = !self.alt || i.modifiers.alt;
    let shift_ok = !self.shift || i.modifiers.shift;
    ctrl_ok && alt_ok && shift_ok && i.key_down(k)
}

}

pub fn shortcut_entries(cfg: &mut ShortcutsConfig) -> [(&'static str, &mut ShortcutBinding); 14] {
    [
        ("Save notes", &mut cfg.save_notes),
        ("Toggle notes panel", &mut cfg.toggle_notes),
        ("Toggle edit / preview", &mut cfg.toggle_edit_preview),
        ("Toggle outline", &mut cfg.toggle_outline),
        ("Previous slide (global)", &mut cfg.prev_slide_global),
        ("Next slide (global)", &mut cfg.next_slide_global),
        ("View: Single page", &mut cfg.view_single),
        ("View: Continuous", &mut cfg.view_continuous),
        ("View: Book spread", &mut cfg.view_twopage),
        ("View: Slideshow", &mut cfg.view_slideshow),
        ("View: Slideshow + Notes", &mut cfg.view_slideshow_notes),
        ("Search in notes", &mut cfg.search_notes),
        ("Trackpad draw (hold)", &mut cfg.trackpad_draw_hold),
        ("Radial tools menu", &mut cfg.radial_tools_menu),
    ]
}
 
pub enum Capture {
    Cancel,
    Set(ShortcutBinding),
}

pub type PendingChord = (Instant, Vec<String>, bool, bool, bool);
const CHORD_WINDOW_MS: u64 = 300;
pub const PENDING_ID: &str = "shortcut_capture_pending";

fn is_modifier_key_name(name: &str) -> bool {
    let l = name.to_lowercase();
    ["control", "ctrl", "alt", "shift", "super", "meta", "command", "option", "win"]
        .iter()
        .any(|m| l.contains(m))
}
 
pub fn capture_shortcut(ui: &egui::Ui) -> Option<Capture> {
    let id = ui.make_persistent_id(egui::IdSalt::new(PENDING_ID));
    let mut pending: Option<PendingChord> = ui.data(|d| d.get_temp(id));
 
    let cancel = ui.input(|i| {
        i.events.iter().any(|ev| {
            matches!(
                ev,
                egui::Event::Key { key: egui::Key::Escape, pressed: true, modifiers, .. }
                    if !modifiers.any()
            )
        })
    });
    if cancel {
        ui.data_mut(|d| d.remove::<PendingChord>(id));
        return Some(Capture::Cancel);
    }
 
    ui.input(|i| {
        for ev in &i.events {
            if let egui::Event::Key {
                key,
                pressed: true,
                repeat: false,
                modifiers,
                ..
            } = ev
            {
                let name = key.name();
                if is_modifier_key_name(name) {
                    continue;
                }
                let ctrl = modifiers.ctrl || modifiers.command;
                match pending.as_mut() {
                    Some(p) => {
                        if !p.1.iter().any(|k| k == name) {
                            p.1.push(name.to_string());
                        }
                        p.2 |= ctrl;
                        p.3 |= modifiers.alt;
                        p.4 |= modifiers.shift;
                    }
                    None => {
                        pending = Some((
                            Instant::now(),
                            vec![name.to_string()],
                            ctrl,
                            modifiers.alt,
                            modifiers.shift,
                        ));
                    }
                }
            }
        }
    });
 
    if let Some(p) = pending {
        if p.0.elapsed() >= Duration::from_millis(CHORD_WINDOW_MS) {
            ui.data_mut(|d| d.remove::<PendingChord>(id));
 
            let mut b = ShortcutBinding::new(p.2, p.3, p.4, p.1.last().unwrap());
            if p.1.len() >= 2 {
                b.extra_key = Some(p.1[0].clone());
            }
            return Some(Capture::Set(b));
        }
        ui.data_mut(|d| d.insert_temp(id, p));
        ui.ctx().request_repaint_after(Duration::from_millis(50));
    }
 
    None
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ShortcutsConfig {
    pub save_notes: ShortcutBinding,
    pub toggle_notes: ShortcutBinding,
    pub toggle_edit_preview: ShortcutBinding,
    pub toggle_outline: ShortcutBinding,
    pub prev_slide_global: ShortcutBinding,
    pub next_slide_global: ShortcutBinding,
    pub view_single: ShortcutBinding,
    pub view_continuous: ShortcutBinding,
    pub view_twopage: ShortcutBinding,
    pub view_slideshow: ShortcutBinding,
    pub view_slideshow_notes: ShortcutBinding,
    pub search_notes: ShortcutBinding,
    pub trackpad_draw_hold: ShortcutBinding,
    pub radial_tools_menu: ShortcutBinding,
}

impl Default for ShortcutsConfig {
    fn default() -> Self {
        Self {
            save_notes: ShortcutBinding::new(true, false, false, "S"),
            toggle_notes: ShortcutBinding::new(true, false, false, "N"),
            toggle_edit_preview: ShortcutBinding::new(true, false, false, "E"),
            toggle_outline: ShortcutBinding::new(true, false, false, "T"),
            prev_slide_global: ShortcutBinding::new(false, true, false, "ArrowLeft"),
            next_slide_global: ShortcutBinding::new(false, true, false, "ArrowRight"),
            view_single: ShortcutBinding::new(false, true, false, "1"),
            view_continuous: ShortcutBinding::new(false, true, false, "2"),
            view_twopage: ShortcutBinding::new(false, true, false, "3"),
            view_slideshow: ShortcutBinding::new(false, true, false, "4"),
            view_slideshow_notes: ShortcutBinding::new(false, true, false, "5"),
            search_notes: ShortcutBinding::new(true, false, false, "F"),
            trackpad_draw_hold: ShortcutBinding::new(false, false, false, "Space"),
            radial_tools_menu: ShortcutBinding::new(false, false, false, "Z"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookmarkItem {
    pub title: String,
    pub page: u32,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ToolsLayoutMode {
    VerticalRail, // Floating vertical pill on the right edge
    SubTopbar,    // Solid horizontal secondary toolbar below the main Topbar
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    /// Font size in the Notes Editor & Preview (default: 14.0 pt)
    pub note_font_size: f32,
    /// Optional path to a user-loaded .ttf / .otf font file
    pub custom_font_path: Option<PathBuf>,
    /// Extra dictionary files (.dic / .txt) loaded for spellcheck
    pub extra_dictionaries: Vec<PathBuf>,
    pub spellcheck_enabled: bool,
    pub slideshow_empty_mode: SlideshowEmptyMode,
    pub tools_layout: ToolsLayoutMode,
    pub tools_button: bool,
    pub radial_menu_enabled: bool,
    pub flash_button: bool,
    pub restore_show_notes: bool,
    pub restore_show_toc: bool,
    pub audio_dock:bool,
    pub audio_notes: bool,
    pub bottom_notes:bool,
    pub default_page_style: crate::notes::BlankPageStyle,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            note_font_size: 14.0,
            custom_font_path: None,
            extra_dictionaries: Vec::new(),
            spellcheck_enabled: true,
            slideshow_empty_mode: SlideshowEmptyMode::StaticEmpty,
            tools_layout: ToolsLayoutMode::SubTopbar,
            tools_button: true,
            radial_menu_enabled: true,
            flash_button: true,
            restore_show_notes: true,
            restore_show_toc: true,
            audio_dock: true,
            audio_notes: true,
            bottom_notes: true,
            default_page_style: crate::notes::BlankPageStyle::Ruled,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AppState {
    pub recent_files: VecDeque<RecentFileEntry>,
    pub export_settings: ExportSettings,
    pub shortcuts: ShortcutsConfig,
    pub settings: AppSettings,
}

impl AppState {
    pub fn config_path() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."));

        let dir = base.join("lapis");
        let _ = fs::create_dir_all(&dir);
        dir.join("state.json")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(mut state) = serde_json::from_str::<Self>(&content) {
                // Filter out files that were deleted or moved from disk
                state.recent_files.retain(|entry| entry.path.exists());
                return state;
            }
        }
        Self::default()
    }

    pub fn save(&self) {
        let path = Self::config_path();
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = fs::write(path, json);
        }
    }

    pub fn find_entry(&self, pdf_path: &Path) -> Option<&RecentFileEntry> {
        let canonical = fs::canonicalize(pdf_path).unwrap_or_else(|_| pdf_path.to_path_buf());
        self.recent_files
            .iter()
            .find(|e| e.path == canonical || e.path == pdf_path)
    }

    pub fn upsert_recent(&mut self, mut entry: RecentFileEntry) {
        if entry.path.as_os_str().is_empty() {
            return;
        }
        if let Ok(canon) = fs::canonicalize(&entry.path) {
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
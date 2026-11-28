//! Shortcut bindings, chord capture, and input tracking.

use eframe::egui;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

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
///      ctx.input(|i| shortcuts::track_key_presses(i));
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShortcutBinding {
    #[serde(default = "shortcut_enabled_default")]
    pub enabled: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: String,
    #[serde(default)]
    pub extra_key: Option<String>,
}

fn shortcut_enabled_default() -> bool {
    true
}

impl ShortcutBinding {
    pub fn new(ctrl: bool, alt: bool, shift: bool, key: &str) -> Self {
        Self {
            enabled: true,
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

    pub fn hint(&self, label: &str) -> String {
        if self.enabled {
            format!("{label} [{}]", self.format_label())
        } else {
            label.to_owned()
        }
    }

    pub fn same_combination(&self, other: &Self) -> bool {
        self.ctrl == other.ctrl
            && self.alt == other.alt
            && self.shift == other.shift
            && self.key == other.key
            && self.extra_key == other.extra_key
    }

    pub fn conflicts_with(&self, other: &Self) -> bool {
        if self.same_combination(other) {
            return true;
        }
        if (self.ctrl, self.alt, self.shift) != (other.ctrl, other.alt, other.shift) {
            return false;
        }
        // A single key must not consume either part of a configured chord.
        match (&self.extra_key, &other.extra_key) {
            (None, Some(prefix)) => self.key == other.key || &self.key == prefix,
            (Some(prefix), None) => self.key == other.key || prefix == &other.key,
            _ => false,
        }
    }

    pub fn is_plain(&self) -> bool {
        !self.ctrl && !self.alt && !self.shift && self.extra_key.is_none()
    }

    pub fn to_egui_key(&self) -> Option<egui::Key> {
        egui::Key::from_name(&self.key)
    }

    pub fn consume(&self, i: &mut egui::InputState) -> bool {
        self.consume_impl(i, true)
    }

    pub fn consume_once(&self, i: &mut egui::InputState) -> bool {
        self.consume_impl(i, false)
    }

    fn modifiers_match(&self, mods: egui::Modifiers) -> bool {
        self.ctrl == (mods.ctrl || mods.command) && self.alt == mods.alt && self.shift == mods.shift
    }

    fn consume_impl(&self, i: &mut egui::InputState, allow_repeat: bool) -> bool {
        if !self.enabled {
            return false;
        }
        track_key_presses(i);
        let Some(k) = self.to_egui_key() else {
            return false;
        };
        if let Some(ek_name) = &self.extra_key {
            return self.consume_chord(i, k, ek_name);
        }

        let mut consumed = false;
        i.events.retain(|event| {
            let matched = matches!(event,
                egui::Event::Key { key, pressed: true, repeat, modifiers, .. }
                    if *key == k && self.modifiers_match(*modifiers)
                        && (allow_repeat || !*repeat));
            consumed |= matched;
            !matched
        });
        consumed
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

            let extra_ok =
                held_extra || t.last_press.get(&ek).map(|&ts| recent(ts)).unwrap_or(false);
            let ctrl_ok = if self.ctrl {
                cur_ctrl || recent(t.last_ctrl)
            } else {
                !cur_ctrl
            };
            let alt_ok = if self.alt {
                cur_alt || recent(t.last_alt)
            } else {
                !cur_alt
            };
            let shift_ok = if self.shift {
                cur_shift || recent(t.last_shift)
            } else {
                !cur_shift
            };
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
        if !self.enabled || !self.modifiers_match(i.modifiers) {
            return false;
        }
        let Some(k) = self.to_egui_key() else {
            return false;
        };
        let extra_held = self
            .extra_key
            .as_ref()
            .is_none_or(|name| egui::Key::from_name(name).is_some_and(|key| i.key_down(key)));
        extra_held && i.key_down(k)
    }
}

pub fn shortcut_entries(cfg: &mut ShortcutsConfig) -> [(&'static str, &mut ShortcutBinding); 35] {
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
        ("Toggle fullscreen", &mut cfg.toggle_fullscreen),
        (
            "Toggle Continuous / Single page",
            &mut cfg.toggle_continuous_single,
        ),
        ("Toggle Sketch tab (notes panel)", &mut cfg.toggle_sketch),
        ("Toggle topbar auto-hide", &mut cfg.toggle_topbar),
        ("Fit PDF and notes layout", &mut cfg.fit_page),
        ("Fit Page", &mut cfg.fit_pdf_page),
        ("Cursor / Select PDF Text", &mut cfg.select_cursor),
        ("Pen", &mut cfg.select_pen),
        ("Highlighter", &mut cfg.select_highlighter),
        ("Textbox", &mut cfg.select_textbox),
        ("Underline", &mut cfg.select_underline),
        ("Eraser", &mut cfg.select_eraser),
        ("Rectangle", &mut cfg.select_rectangle),
        ("Circle", &mut cfg.select_circle),
        ("Line", &mut cfg.select_line),
        ("Arrow", &mut cfg.select_arrow),
        ("Snip", &mut cfg.select_snip),
        ("Occlusion tape", &mut cfg.select_tape),
        ("Fit Width", &mut cfg.fit_width),
        ("PDF night mode", &mut cfg.toggle_night),
        ("Toggle annotation toolbar", &mut cfg.toggle_tools),
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
    [
        "control", "ctrl", "alt", "shift", "super", "meta", "command", "option", "win",
    ]
    .iter()
    .any(|m| l.contains(m))
}

pub fn capture_shortcut(ui: &egui::Ui) -> Option<Capture> {
    let id = egui::Id::unique(PENDING_ID);
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
    #[serde(default)]
    layout_version: u8,
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
    pub toggle_fullscreen: ShortcutBinding,
    pub toggle_continuous_single: ShortcutBinding,
    pub toggle_sketch: ShortcutBinding,
    pub toggle_topbar: ShortcutBinding,
    pub fit_page: ShortcutBinding,
    pub fit_pdf_page: ShortcutBinding,
    pub select_cursor: ShortcutBinding,
    pub select_pen: ShortcutBinding,
    pub select_highlighter: ShortcutBinding,
    pub select_textbox: ShortcutBinding,
    pub select_underline: ShortcutBinding,
    pub select_eraser: ShortcutBinding,
    pub select_rectangle: ShortcutBinding,
    pub select_circle: ShortcutBinding,
    pub select_line: ShortcutBinding,
    pub select_arrow: ShortcutBinding,
    pub select_snip: ShortcutBinding,
    pub select_tape: ShortcutBinding,
    pub fit_width: ShortcutBinding,
    pub toggle_night: ShortcutBinding,
    pub toggle_tools: ShortcutBinding,
}

impl Default for ShortcutsConfig {
    fn default() -> Self {
        Self {
            layout_version: 1,
            save_notes: ShortcutBinding::new(true, false, false, "S"),
            toggle_notes: ShortcutBinding::new(false, false, false, "N"),
            toggle_edit_preview: ShortcutBinding::new(true, false, false, "E"),
            toggle_outline: ShortcutBinding::new(false, false, false, "G"),
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
            toggle_fullscreen: ShortcutBinding::new(false, false, false, "F11"),
            toggle_continuous_single: ShortcutBinding::new(false, false, false, "C"),
            toggle_sketch: ShortcutBinding::new(false, true, false, "S"),
            toggle_topbar: ShortcutBinding::new(false, true, false, "T"),
            fit_page: ShortcutBinding::new(false, true, false, "F"),
            fit_pdf_page: ShortcutBinding::new(false, false, false, "F"),
            select_cursor: ShortcutBinding::new(false, false, false, "V"),
            select_pen: ShortcutBinding::new(false, false, false, "P"),
            select_highlighter: ShortcutBinding::new(false, false, false, "H"),
            select_textbox: ShortcutBinding::new(false, false, false, "T"),
            select_underline: ShortcutBinding::new(false, false, false, "U"),
            select_eraser: ShortcutBinding::new(false, false, false, "E"),
            select_rectangle: ShortcutBinding::new(false, false, false, "R"),
            select_circle: ShortcutBinding::new(false, false, false, "O"),
            select_line: ShortcutBinding::new(false, false, false, "L"),
            select_arrow: ShortcutBinding::new(false, false, false, "A"),
            select_snip: ShortcutBinding::new(false, false, false, "S"),
            select_tape: ShortcutBinding::new(false, false, false, "B"),
            fit_width: ShortcutBinding::new(false, false, false, "W"),
            toggle_night: ShortcutBinding::new(false, false, false, "M"),
            toggle_tools: ShortcutBinding::new(false, false, false, "D"),
        }
    }
}

impl ShortcutsConfig {
    pub(crate) fn migrate_layout(&mut self) {
        if self.layout_version != 0 {
            return;
        }
        // Prefer existing custom bindings over newly introduced defaults.
        let mut entries = shortcut_entries(self);
        // The original actions precede the newly added ones in shortcut_entries.
        let (existing, added) = entries.split_at_mut(21);
        for (_, binding) in added {
            if existing
                .iter()
                .any(|(_, old)| old.enabled && old.conflicts_with(binding))
            {
                binding.enabled = false;
            }
        }
        for (label, old_key, new_key) in [
            ("Toggle notes panel", "N", "N"),
            ("Toggle edit / preview", "E", "X"),
            ("Toggle outline", "T", "G"),
        ] {
            let old = ShortcutBinding::new(true, false, false, old_key);
            let new = ShortcutBinding::new(false, false, false, new_key);
            let mut entries = shortcut_entries(self);
            let taken = entries.iter().any(|(name, binding)| {
                *name != label && binding.enabled && binding.conflicts_with(&new)
            });
            if !taken
                && let Some((_, binding)) = entries.iter_mut().find(|(name, _)| *name == label)
                && **binding == old
            {
                **binding = new;
            }
        }
        self.layout_version = 1;
    }
}

#[cfg(test)]
mod shortcut_tests {
    use super::*;

    #[test]
    fn defaults_are_unique_and_enabled() {
        let mut cfg = ShortcutsConfig::default();
        let entries = shortcut_entries(&mut cfg);
        for (index, (name, binding)) in entries.iter().enumerate() {
            assert!(binding.enabled, "{name}");
            assert!(binding.to_egui_key().is_some(), "{name}");
            for (other_name, other) in &entries[index + 1..] {
                assert!(
                    !binding.same_combination(other),
                    "{name} conflicts with {other_name}"
                );
            }
        }
        assert_eq!(cfg.toggle_notes.format_label(), "N");
        assert_eq!(cfg.toggle_edit_preview.format_label(), "Ctrl + E");
        assert_eq!(cfg.toggle_outline.format_label(), "G");
    }

    #[test]
    fn disabled_bindings_round_trip_and_hide_hints() {
        let mut binding = ShortcutBinding::new(false, true, false, "H");
        assert_eq!(binding.hint("Highlighter"), "Highlighter [Alt + H]");
        binding.enabled = false;
        let saved = serde_json::to_string(&binding).unwrap();
        let restored: ShortcutBinding = serde_json::from_str(&saved).unwrap();
        assert_eq!(binding, restored);
        assert_eq!(restored.hint("Highlighter"), "Highlighter");
        assert_eq!(restored.format_label(), "Alt + H");
    }

    #[test]
    fn legacy_defaults_migrate_without_overwriting_custom_bindings() {
        let mut cfg = ShortcutsConfig::default();
        cfg.layout_version = 0;
        cfg.toggle_notes = ShortcutBinding::new(true, false, false, "N");
        cfg.toggle_edit_preview = ShortcutBinding::new(true, false, false, "E");
        cfg.toggle_outline = ShortcutBinding::new(true, true, false, "G");
        cfg.save_notes = ShortcutBinding::new(false, false, false, "P");
        cfg.migrate_layout();
        assert_eq!(cfg.toggle_notes.format_label(), "N");
        assert_eq!(cfg.toggle_edit_preview.format_label(), "X");
        assert_eq!(cfg.toggle_outline.format_label(), "Ctrl + Alt + G");
        assert_eq!(cfg.save_notes.format_label(), "P");
        assert!(!cfg.select_pen.enabled);
        cfg.toggle_notes = ShortcutBinding::new(true, false, false, "N");
        cfg.migrate_layout();
        assert_eq!(cfg.toggle_notes.format_label(), "Ctrl + N");
    }

    #[test]
    fn plain_keys_do_not_match_modified_keys() {
        let binding = ShortcutBinding::new(false, false, false, "P");
        assert!(binding.modifiers_match(egui::Modifiers::NONE));
        assert!(!binding.modifiers_match(egui::Modifiers::COMMAND));
        assert!(!binding.modifiers_match(egui::Modifiers::ALT));
        assert!(!binding.modifiers_match(egui::Modifiers::SHIFT));
    }

    #[test]
    fn legacy_binding_is_enabled_when_flag_is_missing() {
        let binding: ShortcutBinding =
            serde_json::from_str(r#"{"ctrl":false,"alt":false,"shift":false,"key":"P"}"#).unwrap();
        assert!(binding.enabled);
    }
}

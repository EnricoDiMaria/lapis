#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod annotations;
mod audio;
mod clipboard;
mod dialogs;
mod doc_lock;
mod docid;
mod export;
mod flashcards;
mod markdown;
mod notes;
mod paths;
mod persistence;
mod screenshots;
mod search;
mod shortcuts;
mod sketch;
mod spellcheck;
mod state;
mod sysproc;
#[cfg(test)]
mod test_support;
mod theme;
mod theme_picker;
mod viewer;
mod workspace;

use eframe::egui;
use std::env;
use std::path::PathBuf;
use workspace::Lapis;

fn load_icon() -> Option<egui::IconData> {
    let img = image::load_from_memory(include_bytes!("../assets/icon.png"))
        .ok()?
        .to_rgba8();
    let (width, height) = img.dimensions();
    Some(egui::IconData {
        rgba: img.into_raw(),
        width,
        height,
    })
}

fn main() -> eframe::Result<()> {
    // With windows_subsystem = "windows" and panic = "abort" a crash would be silent.
    std::panic::set_hook(Box::new(|info| {
        let _ = std::fs::write(paths::data_dir().join("crash.log"), info.to_string());
    }));

    let initial_pdf = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .filter(|p| p.exists());

    let mut viewport = egui::ViewportBuilder::default()
        .with_drag_and_drop(true)
        .with_inner_size([1200.0, 720.0])
        .with_min_inner_size([560.0, 400.0])
        .with_title("Lapis");
    if let Some(icon) = load_icon() {
        viewport = viewport.with_icon(icon);
    }
    let opts = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "Lapis",
        opts,
        Box::new(|cc| Ok(Box::new(Lapis::new(cc, initial_pdf)))),
    )
}

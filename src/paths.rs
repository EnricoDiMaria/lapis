
//!
//! Linux  : XDG_CONFIG_HOME|~/.config, XDG_DATA_HOME|~/.local/share
//! Windows: config -> %APPDATA%\lapis  (Roaming)
//!          data   -> %LOCALAPPDATA%\lapis

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Once;

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn ensure(p: PathBuf) -> PathBuf {
    let _ = fs::create_dir_all(&p);
    p
}

#[cfg(windows)]
fn config_base() -> PathBuf {
    env_path("APPDATA")
        .or_else(|| env_path("USERPROFILE").map(|h| h.join("AppData").join("Roaming")))
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(windows)]
fn data_base() -> PathBuf {
    env_path("LOCALAPPDATA")
        .or_else(|| env_path("USERPROFILE").map(|h| h.join("AppData").join("Local")))
        .or_else(|| env_path("APPDATA"))
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(not(windows))]
fn config_base() -> PathBuf {
    env_path("XDG_CONFIG_HOME")
        .or_else(|| env_path("HOME").map(|h| h.join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(not(windows))]
fn data_base() -> PathBuf {
    env_path("XDG_DATA_HOME")
        .or_else(|| env_path("HOME").map(|h| h.join(".local").join("share")))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// state.json, theme.json
pub fn config_dir() -> PathBuf {
    ensure(config_base().join("lapis"))
}

/// media/, models/, locks/, dictionaries/, originals/
pub fn data_dir() -> PathBuf {
    ensure(data_base().join("lapis"))
}

pub fn models_dir() -> PathBuf {
    ensure(data_dir().join("models"))
}

pub fn notes_backup_dir() -> PathBuf {
    static MIGRATE: Once = Once::new();
    let dir = ensure(data_dir().join("notes"));
    MIGRATE.call_once(|| migrate_legacy_notes(&dir));
    dir
}

fn migrate_legacy_notes(new_dir: &Path) {
    let legacy_root = data_base().join("lapispdf");
    let legacy_notes = legacy_root.join("notes");
    let Ok(rd) = fs::read_dir(&legacy_notes) else { return };
    for entry in rd.flatten() {
        let from = entry.path();
        if !from.is_file() {
            continue;
        }
        let to = new_dir.join(entry.file_name());
        if to.exists() {
            continue;
        }
        if fs::rename(&from, &to).is_err() && fs::copy(&from, &to).is_ok() {
            let _ = fs::remove_file(&from);
        }
    }
    let _ = fs::remove_dir(&legacy_notes); // riesce solo se vuota
    let _ = fs::remove_dir(&legacy_root); // idem
}

pub fn file_uri(path: &Path) -> String {
    let s = path.to_string_lossy();
    #[cfg(windows)]
    let s = s.replace('\\', "/");
    format!("file://{s}")
}

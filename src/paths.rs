
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
    let _ = fs::remove_dir(&legacy_notes);
    let _ = fs::remove_dir(&legacy_root);
}

/// `file://` URI usable by egui_extras / Markdown.
///
/// Windows needs the standard form with three slashes: `C:\Users\me\a b.png` ->
/// `file:///C:/Users/me/a b.png`. With two slashes egui_extras reads `C:` as a network host
/// and fails with "The network path was not found (os error 53)". Backslashes are also
/// replaced because in Markdown they are escape characters.
pub fn file_uri(path: &Path) -> String {
    let s = path.to_string_lossy();
    if cfg!(windows) {
        windows_file_uri(&s)
    } else {
        format!("file://{s}")
    }
}

/// Pure function (testable on any OS): Windows path -> `file://` URI.
fn windows_file_uri(path: &str) -> String {
    let s = path.replace('\\', "/");
    if s.starts_with("//") {
        // UNC: //server/share/x.png -> file://server/share/x.png
        format!("file:{s}")
    } else {
        // drive: C:/Users/x.png -> file:///C:/Users/x.png
        format!("file:///{s}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_uris_use_three_slashes_and_forward_slashes() {
        assert_eq!(
            windows_file_uri(r"C:\Users\me\AppData\Local\lapis\media\abc\screenshots\shot_1.png"),
            "file:///C:/Users/me/AppData/Local/lapis/media/abc/screenshots/shot_1.png"
        );
        assert_eq!(windows_file_uri(r"D:\a b\c.png"), "file:///D:/a b/c.png");
        assert_eq!(windows_file_uri(r"\\server\share\x.png"), "file://server/share/x.png");
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_uri_is_unchanged() {
        assert_eq!(file_uri(Path::new("/home/me/a b.png")), "file:///home/me/a b.png");
    }
 }


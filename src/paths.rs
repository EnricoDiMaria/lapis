//!
//! Linux  : XDG_CONFIG_HOME|~/.config, XDG_DATA_HOME|~/.local/share
//! Windows: config -> %APPDATA%\lapis  (Roaming)
//!          data   -> %LOCALAPPDATA%\lapis

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Once;

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
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
    let Ok(rd) = fs::read_dir(&legacy_notes) else {
        return;
    };
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

pub fn write_synced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Atomic replace. Windows may briefly deny the rename (viewer, indexer or antivirus handle),
/// so retry a few times there; the directory is synced on Unix to persist the rename.
pub fn replace_file(tmp: &Path, dest: &Path) -> io::Result<()> {
    replace_file_checked(tmp, dest, || Ok(()))
}

pub(crate) fn replace_file_checked(
    tmp: &Path,
    dest: &Path,
    mut check: impl FnMut() -> io::Result<()>,
) -> io::Result<()> {
    let mut attempt = 0u64;
    loop {
        check()?;
        match fs::rename(tmp, dest) {
            Err(err)
                if cfg!(windows)
                    && err.kind() == io::ErrorKind::PermissionDenied
                    && attempt < 5 =>
            {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(100 * attempt));
            }
            result => {
                #[cfg(unix)]
                if result.is_ok()
                    && let Some(dir) = dest.parent().filter(|d| !d.as_os_str().is_empty())
                    && let Ok(dir) = File::open(dir)
                {
                    let _ = dir.sync_all();
                }
                return result;
            }
        }
    }
}

/// Writes `bytes` to a temporary file next to `dest`, flushes it, then replaces `dest`
/// atomically: after a crash `dest` holds either the old or the new content, never a mix.
pub fn write_atomic(dest: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut name = dest
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?
        .to_os_string();
    name.push(format!(".{}.tmp", std::process::id()));
    let tmp = dest.with_file_name(name);
    let result = write_synced(&tmp, bytes).and_then(|_| replace_file(&tmp, dest));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Keeps an unreadable state file as `<name>.corrupt` so the next save cannot destroy it.
pub fn quarantine_corrupt(path: &Path) -> Option<PathBuf> {
    let mut name = path.file_name()?.to_os_string();
    name.push(".corrupt");
    let target = path.with_file_name(name);
    fs::rename(path, &target).ok().map(|_| target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lapis_paths_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn write_atomic_replaces_content_and_leaves_no_temporary_file() {
        let dir = scratch("atomic");
        let file = dir.join("state.json");
        write_atomic(&file, b"old").unwrap();
        write_atomic(&file, b"new content").unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"new content");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "temporary file left behind");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_atomic_write_keeps_the_old_file_and_cleans_up() {
        let dir = scratch("atomic_fail");
        let file = dir.join("state.json");
        write_atomic(&file, b"keep me").unwrap();
        // A directory in the way of the rename target makes the replacement fail.
        let blocked = dir.join("blocked");
        fs::create_dir(&blocked).unwrap();
        fs::write(blocked.join("child"), b"x").unwrap();
        assert!(write_atomic(&blocked, b"data").is_err());
        assert_eq!(fs::read(&file).unwrap(), b"keep me");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 2, "temporary file left behind");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn quarantine_keeps_the_unreadable_file_out_of_the_way() {
        let dir = scratch("quarantine");
        let file = dir.join("state.json");
        fs::write(&file, b"{ truncated").unwrap();
        let kept = quarantine_corrupt(&file).unwrap();
        assert!(!file.exists());
        assert_eq!(kept.file_name().unwrap(), "state.json.corrupt");
        assert_eq!(fs::read(&kept).unwrap(), b"{ truncated");
        assert!(quarantine_corrupt(&file).is_none(), "nothing left to quarantine");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn windows_uris_use_three_slashes_and_forward_slashes() {
        assert_eq!(
            windows_file_uri(r"C:\Users\me\AppData\Local\lapis\media\abc\screenshots\shot_1.png"),
            "file:///C:/Users/me/AppData/Local/lapis/media/abc/screenshots/shot_1.png"
        );
        assert_eq!(windows_file_uri(r"D:\a b\c.png"), "file:///D:/a b/c.png");
        assert_eq!(
            windows_file_uri(r"\\server\share\x.png"),
            "file://server/share/x.png"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_uri_is_unchanged() {
        assert_eq!(
            file_uri(Path::new("/home/me/a b.png")),
            "file:///home/me/a b.png"
        );
    }
}

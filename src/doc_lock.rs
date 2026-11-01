//! Lock per documento: un solo processo alla volta può avere aperto lo stesso PDF.
//!
//! Usa un lock advisory del sistema operativo (flock): viene rilasciato da solo anche se
//! l'app va in crash o viene chiusa con kill, quindi non restano mai lock "orfani".

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

pub struct DocLock {
    _file: File,
}

#[derive(Debug)]
pub enum LockError {
    Busy,
    Io(io::Error),
}

pub fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn lock_file_for(canon: &Path) -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("lapis")
        .join("locks")
        .join(format!("{:016x}.lock", fnv1a(canon.to_string_lossy().as_bytes())))
}

pub fn acquire(path: &Path) -> Result<DocLock, LockError> {
    let lock_path = lock_file_for(&canonical(path));
    if let Some(dir) = lock_path.parent() {
        fs::create_dir_all(dir).map_err(LockError::Io)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(LockError::Io)?;
    match file.try_lock() {
        Ok(()) => Ok(DocLock { _file: file }),
        Err(TryLockError::WouldBlock) => Err(LockError::Busy),
        Err(TryLockError::Error(e)) => Err(LockError::Io(e)),
    }
}

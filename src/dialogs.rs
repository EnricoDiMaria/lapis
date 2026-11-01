//! Non-blocking file dialogs.
//!
//! `rfd::FileDialog::pick_file()` and similar methods are synchronous; if called from the UI thread,
//! they block the event loop, causing some compositors (e.g., Hyprland) to report that the app is unresponsive.
//! Here, the dialog runs in a dedicated thread, and the result is retrieved each frame via
//! `Dialogs::poll()`.

use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, TryRecvError};

/// Operation to be re-initiated after the user has selected the ASR model folder.
#[derive(Clone, Debug)]
pub enum AsrRetry {
    Lecture {
        wav_path: PathBuf,
    },
    Memo {
        page: u32,
        memo_idx: usize,
        wav_path: PathBuf,
    },
}

#[derive(Clone, Debug)]
pub enum DialogKind {
    OpenPdf,
    NewNotebook,
    ExportOriginalPdf,
    ExportAnki,
    PickAsrModelDir { retry: AsrRetry },
    ImportLectureMedia { media_dir: PathBuf },
    AddDictionary,
    ExportTheme,
    ImportTheme,
    ExportNotes,
    ImportNotes,
}

#[derive(Clone, Copy)]
enum Mode {
    PickFile,
    SaveFile,
    PickFolder,
}

pub struct DialogSpec {
    mode: Mode,
    title: Option<String>,
    filters: Vec<(String, Vec<String>)>,
    file_name: Option<String>,
    directory: Option<PathBuf>,
}

impl DialogSpec {
    fn new(mode: Mode) -> Self {
        Self {
            mode,
            title: None,
            filters: Vec::new(),
            file_name: None,
            directory: None,
        }
    }

    pub fn open_file() -> Self {
        Self::new(Mode::PickFile)
    }
    pub fn save_file() -> Self {
        Self::new(Mode::SaveFile)
    }
    pub fn pick_folder() -> Self {
        Self::new(Mode::PickFolder)
    }

    pub fn title(mut self, t: impl Into<String>) -> Self {
        self.title = Some(t.into());
        self
    }

    pub fn filter(mut self, name: &str, exts: &[&str]) -> Self {
        self.filters
            .push((name.to_string(), exts.iter().map(|s| s.to_string()).collect()));
        self
    }

    pub fn file_name(mut self, n: impl Into<String>) -> Self {
        self.file_name = Some(n.into());
        self
    }

    pub fn directory(mut self, d: impl AsRef<Path>) -> Self {
        self.directory = Some(d.as_ref().to_path_buf());
        self
    }

    fn run(self) -> Option<PathBuf> {
        let mut d = rfd::FileDialog::new();
        if let Some(t) = &self.title {
            d = d.set_title(t.as_str());
        }
        for (name, exts) in &self.filters {
            let e: Vec<&str> = exts.iter().map(String::as_str).collect();
            d = d.add_filter(name.as_str(), e.as_slice());
        }
        if let Some(n) = &self.file_name {
            d = d.set_file_name(n.as_str());
        }
        if let Some(dir) = &self.directory {
            d = d.set_directory(dir);
        }
        match self.mode {
            Mode::PickFile => d.pick_file(),
            Mode::SaveFile => d.save_file(),
            Mode::PickFolder => d.pick_folder(),
        }
    }
}

struct Pending {
    kind: DialogKind,
    rx: Receiver<Option<PathBuf>>,
}

#[derive(Default)]
pub struct Dialogs {
    pending: Option<Pending>,
}

impl Dialogs {
    #[allow(dead_code)]
    pub fn is_open(&self) -> bool {
        self.pending.is_some()
    }

    pub fn open(&mut self, ctx: &egui::Context, kind: DialogKind, spec: DialogSpec) -> bool {
        if self.pending.is_some() {
            return false;
        }
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(spec.run());
            ctx.request_repaint();
        });
        self.pending = Some(Pending { kind, rx });
        true
    }

    pub fn poll(&mut self) -> Option<(DialogKind, Option<PathBuf>)> {
        let res = match self.pending.as_ref()?.rx.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => None,
        };
        let kind = self.pending.take()?.kind;
        Some((kind, res))
    }
}

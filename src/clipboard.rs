use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::OnceLock;

/// qdbus / wl-paste / wl-copy / xclip exist only on Linux/BSD desktops.
/// On Windows and macOS only `arboard` is used.
const EXTERNAL_TOOLS: bool = cfg!(all(unix, not(target_os = "macos")));

/// External methods bit: results absent (binary not installed).
static DEAD_CLIP_METHODS: AtomicU32 = AtomicU32::new(0);

/// A single writer keeps updates external to the clipboard organized.
/// Prevent separately spawned threads from completing in reverse order.
static CLIP_WRITE_TX: OnceLock<Sender<String>> = OnceLock::new();

pub fn clip_read_method(cb: Option<&mut arboard::Clipboard>, m: u8) -> Option<String> {
    let raw = match m {
        0 => cb.and_then(|c| c.get_text().ok()),
        1 => clip_run_capture(1, "qdbus6", &KLIPPER_GET).map(clip_strip_one_newline),
        2 => clip_run_capture(2, "qdbus", &KLIPPER_GET).map(clip_strip_one_newline),
        3 => clip_run_capture(
            3,
            "wl-paste",
            &["--no-newline", "--type", "text/plain;charset=utf-8"],
        ),
        4 => clip_run_capture(4, "wl-paste", &["--no-newline", "--type", "text/plain"]),
        5 => None, // Non usare wl-paste senza --type: potrebbe restituire byte immagine.
        6 => clip_run_capture(6, "xclip", &["-selection", "clipboard", "-t", "UTF8_STRING", "-o"]),
        _ => None,
    }?;

    let text = raw.replace("\r\n", "\n");
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

pub fn clip_dead(bit: u32) -> bool {
    DEAD_CLIP_METHODS.load(Ordering::Relaxed) & (1 << bit) != 0
}

pub fn clip_mark_dead(bit: u32) {
    DEAD_CLIP_METHODS.fetch_or(1 << bit, Ordering::Relaxed);
}

/// Executes a command and returns its stdout; if the program does not exist, it marks it as absent.
pub fn clip_run_capture(bit: u32, cmd: &str, args: &[&str]) -> Option<String> {
    if !EXTERNAL_TOOLS || clip_dead(bit) {
        return None;
    }
    match Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    {
        Ok(out) if out.status.success() => String::from_utf8(out.stdout).ok(),
        Ok(_) => None,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                clip_mark_dead(bit);
            }
            None
        }
    }
}

fn clip_strip_one_newline(mut s: String) -> String {
    if s.ends_with('\n') {
        s.pop();
        if s.ends_with('\r') {
            s.pop();
        }
    }
    s
}

const KLIPPER_GET: [&str; 3] = [
    "org.kde.klipper",
    "/klipper",
    "org.kde.klipper.klipper.getClipboardContents",
];

/// Serialized external writer: all `set_text` calls are executed in order.
fn clip_write_sender() -> &'static Sender<String> {
    CLIP_WRITE_TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            while let Ok(text) = rx.recv() {
                clip_write_external(&text);
            }
        });
        tx
    })
}

/// External write performed exclusively by the serialized worker.
pub fn clip_write_external(text: &str) {
    if !EXTERNAL_TOOLS {
        return;
    }
    for (bit, cmd) in [(1u32, "qdbus6"), (2u32, "qdbus")] {
        if clip_dead(bit) {
            continue;
        }
        match Command::new(cmd)
            .args([
                "org.kde.klipper",
                "/klipper",
                "org.kde.klipper.klipper.setClipboardContents",
                text,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            Ok(st) if st.success() => return,
            Ok(_) => {}
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    clip_mark_dead(bit);
                }
            }
        }
    }

    if !clip_dead(7) {
        match Command::new("wl-copy")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(text.as_bytes());
                }
                let _ = child.wait();
                return;
            }
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    clip_mark_dead(7);
                }
            }
        }
    }

    if !clip_dead(8) {
        match Command::new("xclip")
            .args(["-selection", "clipboard"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(text.as_bytes());
                }
                let _ = child.wait();
            }
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    clip_mark_dead(8);
                }
            }
        }
    }
}

pub struct SystemClipboard {
    backend: Option<arboard::Clipboard>,
    /// Last reading method that worked (0 = onboard, 1..6 = external).
    last_ok: u8,
}

impl SystemClipboard {
    pub fn new() -> Self {
        Self {
            backend: arboard::Clipboard::new().ok(),
            last_ok: 0,
        }
    }

    fn read_method(&mut self, m: u8) -> Option<String> {
        clip_read_method(self.backend.as_mut(), m)
    }

    /// Reads the current content, without restoring or overwriting previous values.
    pub fn get_text(&mut self) -> Option<String> {
        let first = self.last_ok;
        if let Some(text) = self.read_method(first) {
            return Some(text);
        }
        for method in 0..=6u8 {
            if method == first {
                continue;
            }
            if let Some(text) = self.read_method(method) {
                self.last_ok = method;
                return Some(text);
            }
        }
        None
    }

    /// Copy new text: immediate update with an artboard and an external fallback in the queue.
    pub fn set_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }

        let written_by_arboard = self
            .backend
            .as_mut()
            .is_some_and(|cb| cb.set_text(text.to_owned()).is_ok());

        if !written_by_arboard {
            let _ = clip_write_sender().send(text.to_owned());
        }
    }
}

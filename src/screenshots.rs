//! Screenshot: reading from the clipboard or a file, saving as PNG, Markdown support for notes,
//! and asynchronous handling (reading from the clipboard may invoke external programs
//! and must not block the UI).

use eframe::egui;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::{Mutex, OnceLock};

/// Non-premultiplied RGBA8 image.
pub struct Shot {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Shot {
    pub fn from_bytes(bytes: &[u8]) -> Option<Shot> {
        let img = image::load_from_memory(bytes).ok()?.to_rgba8();
        Some(Shot {
            width: img.width(),
            height: img.height(),
            rgba: img.into_raw(),
        })
    }

    pub fn from_file(path: &Path) -> Option<Shot> {
        Shot::from_bytes(&std::fs::read(path).ok()?)
    }

    pub fn downscaled(self, max_width: u32) -> Shot {
        if self.width <= max_width || self.width == 0 {
            return self;
        }
        let nh = ((self.height as f64) * (max_width as f64) / (self.width as f64)).round().max(1.0) as u32;
        let Some(img) = image::RgbaImage::from_raw(self.width, self.height, self.rgba) else {
            return Shot { width: 0, height: 0, rgba: Vec::new() };
        };
        let small = image::imageops::resize(&img, max_width, nh, image::imageops::FilterType::Triangle);
        Shot {
            width: small.width(),
            height: small.height(),
            rgba: small.into_raw(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0 || self.rgba.len() < (self.width as usize) * (self.height as usize) * 4
    }
}

static CACHED_SHOT: OnceLock<Mutex<Option<Shot>>> = OnceLock::new();

fn shot_cache() -> &'static Mutex<Option<Shot>> {
    CACHED_SHOT.get_or_init(|| Mutex::new(None))
}

pub fn prefetch_clipboard_image() -> bool {
    if !clipboard_has_image() {
        if let Ok(mut g) = shot_cache().lock() {
            *g = None;
        }
        return false;
    }
    if let Some(shot) = read_clipboard_image() {
        if let Ok(mut g) = shot_cache().lock() {
            *g = Some(shot);
        }
        return true;
    }
    false
}

pub fn is_image_path(p: &Path) -> bool {
    matches!(
        p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("png" | "jpg" | "jpeg" | "webp" | "bmp" | "gif")
    )
}

/// Reads an image from the system clipboard (blocking: must be called from a thread).
/// 1) arboard (X11 and, with the `wayland-data-control` feature, Wayland)
/// 2) External commands, as you already do for text: wl-paste (Wayland) and xclip (X11)
fn read_clipboard_image() -> Option<Shot> {
    if let Ok(g) = shot_cache().lock() {
        if let Some(cached) = g.as_ref() {
            return Some(Shot {
                width: cached.width,
                height: cached.height,
                rgba: cached.rgba.clone(),
            });
        }
    }
    if let Ok(mut cb) = arboard::Clipboard::new() {
        if let Ok(img) = cb.get_image() {
            let shot = Shot {
                width: img.width as u32,
                height: img.height as u32,
                rgba: img.bytes.into_owned(),
            };
            if !shot.is_empty() {
                return Some(shot);
            }
        }
    }
    let attempts: [(&str, &[&str]); 2] = [
        ("wl-paste", &["--type", "image/png"]),
        ("xclip", &["-selection", "clipboard", "-t", "image/png", "-o"]),
    ];
    for (cmd, args) in attempts {
        let out = Command::new(cmd)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();
        if let Ok(out) = out {
            if out.status.success() && !out.stdout.is_empty() {
                if let Some(shot) = Shot::from_bytes(&out.stdout) {
                    return Some(shot);
                }
            }
        }
    }
    None
}

pub fn clipboard_has_image() -> bool {
    let attempts: [(&str, &[&str]); 2] = [
        ("wl-paste", &["--list-types"]),
        ("xclip", &["-selection", "clipboard", "-t", "TARGETS", "-o"]),
    ];
    for (cmd, args) in attempts {
        if let Ok(out) = Command::new(cmd)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
        {
            if out.status.success() {
                let types = String::from_utf8_lossy(&out.stdout);
                if types.lines().any(|l| l.trim().starts_with("image/")) {
                    return true;
                }
            }
        }
    }
    false
}

pub fn is_image_placeholder(text: &str) -> bool {
    let t = text.trim();
    let Some(rest) = t.strip_prefix('\u{25A8}') else {
        return false;
    };
    let rest = rest.trim();
    let mut it = rest.splitn(2, 'x');
    matches!(
        (it.next().map(|a| a.trim().parse::<u32>()), it.next().map(|b| b.trim().parse::<u32>())),
        (Some(Ok(_)), Some(Ok(_)))
    )
}

pub fn save_png(shot: &Shot, dir: &Path) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let mut path = dir.join(format!("shot_{millis}.png"));
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("shot_{millis}_{n}.png"));
        n += 1;
    }
    image::save_buffer(&path, &shot.rgba, shot.width, shot.height, image::ColorType::Rgba8)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
    Ok(path)
}

pub fn markdown_image(path: &Path) -> String {
    let clean = path.to_string_lossy().replace(' ', "%20");
    format!("![screenshot](file://{clean})")
}

#[derive(Clone, Debug)]
pub enum ShotTarget {
    Pdf { page: u32, center: (f32, f32) },
    Note { page: u32, cursor: usize },
}

#[derive(Default)]
pub struct ShotJobs {
    pending: Option<(ShotTarget, Receiver<Option<Shot>>)>,
}

impl ShotJobs {
    /*pub fn is_busy(&self) -> bool {
        self.pending.is_some()
    }*/

    pub fn request_clipboard(&mut self, ctx: &egui::Context, target: ShotTarget) -> bool {
        self.spawn(ctx, target, read_clipboard_image)
    }

    pub fn request_file(&mut self, ctx: &egui::Context, target: ShotTarget, path: PathBuf) -> bool {
        self.spawn(ctx, target, move || Shot::from_file(&path))
    }

    fn spawn(
        &mut self,
        ctx: &egui::Context,
        target: ShotTarget,
        f: impl FnOnce() -> Option<Shot> + Send + 'static,
    ) -> bool {
        if self.pending.is_some() {
            return false;
        }
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(f());
            ctx.request_repaint();
        });
        self.pending = Some((target, rx));
        true
    }

    pub fn poll(&mut self) -> Option<(ShotTarget, Option<Shot>)> {
        let res = match self.pending.as_ref()?.1.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => None,
        };
        let (target, _) = self.pending.take()?;
        Some((target, res))
    }
}

pub fn ctrl_recently_held(ctx: &egui::Context) -> bool {
    use std::cell::Cell;
    use std::time::{Duration, Instant};
    thread_local! {
        static LAST_CTRL: Cell<Option<Instant>> = const { Cell::new(None) };
    }
    let now = Instant::now();
    let held_now = ctx.input(|i| {
        i.modifiers.ctrl
            || i.modifiers.command
            || i.events.iter().any(|e| match e {
                egui::Event::Key { modifiers, .. } => modifiers.ctrl || modifiers.command,
                _ => false,
            })
    });
    LAST_CTRL.with(|c| {
        if held_now {
            c.set(Some(now));
            true
        } else if let Some(t) = c.get() {
            now.duration_since(t) <= Duration::from_millis(250)
        } else {
            false
        }
    })
}
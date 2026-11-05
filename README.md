<h1>
  <img src="assets/icon.png" alt="Lapis icon" width="52" align="absmiddle">
  Lapis
</h1>

**Advanced PDF Study & Lecture Workspace**

*PDF annotation • Markdown & LaTeX notes • Lecture audio synchronization • Offline transcription • Active Recall*

<div align="center">

[![Rust](https://img.shields.io/badge/Built_with-Rust-dca282.svg)](https://www.rust-lang.org/)
[![GUI](https://img.shields.io/badge/GUI-egui-blue.svg)](https://github.com/emilk/egui)
[![Platform](https://img.shields.io/badge/Platform-Linux_%7C_Windows-lightgrey.svg)](../../releases/latest)
[![License](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)

</div>

---

## Overview

**Lapis** is a native, feature-rich study environment built in Rust. It brings lecture slides, handwritten-style annotations, structured Markdown notes, mathematical notation, audio recordings, automatic transcription, and active-recall tools together in a single workspace.

Instead of switching between a PDF reader, a note-taking application, an audio player, and a flashcard program, Lapis lets you work with all these materials in the context of the same document.

Each PDF page can have its own notes, annotations, audio references, transcript segments, and study material. Your work is saved locally and associated with the document.

## Features at a Glance

- **Advanced PDF Viewer** — Single Page, Continuous Scroll, Two-Page Book, Slideshow, and Slideshow with Notes modes.
- **Split View** — Display and annotate two PDF pages side by side.
- **PDF Night Mode** — Toggle a dark viewing treatment for PDFs.
- **Vector Annotation Tools** — Freehand drawing, highlighting, underlining, shapes, text boxes, erasing, occlusion tapes, region snipping, and laser pointer.
- **Custom Study Pages** — Insert ruled, dotted, grid, or blank pages into your document.
- **Markdown & LaTeX Notes** — Edit and preview formatted notes, including mathematical expressions.
- **Spellchecking & Dictionaries** — Offline spellchecking, suggestions, custom words, and additional dictionaries.
- **Lecture Audio Workspace** — Import or record lecture audio, inspect its waveform, trim recordings, and navigate through playback.
- **Offline Speech Recognition** — Transcribe lectures and individual voice memos with Parakeet.
- **Slide-Aware Transcripts** — Associate transcript segments with PDF pages and navigate between the recording and the corresponding slide.
- **Voice Memos** — Record and transcribe additional audio notes associated with individual pages.
- **Active Recall** — Use occlusion tapes and flashcards to test your knowledge.
- **Anki Export** — Export supported question-and-answer notes as TSV decks.
- **Global Search & Command Palette** — Search across PDF content, notes, and audio transcripts, or quickly access application commands.
- **Bookmarks, Thumbnails & Outline** — Navigate large documents more efficiently.
- **Import & Export** — Import Markdown/text notes, export notes, and export a PDF with your annotations, your notes, or both, keeping the original pages untouched.
- **Customizable Workspace** — Themes, typography, keyboard shortcuts, study tools, and storage management.

> ### 📖 [Read the complete User Guide →](docs/USER_GUIDE.md)
> Viewing modes, annotation tools, Markdown & LaTeX notes, the audio workspace, offline transcription, flashcards, search, settings and export, all explained step by step.

---

# Download & Installation

Lapis is distributed as a native application. Pre-built packages are available from the [Releases](../../releases/latest) page.

## Linux

### 1. Remote installation

The installation script can download and install the latest release into `~/.local/bin`, install the application icon, register a `.desktop` entry, and optionally download the Parakeet transcription model.

```bash
curl -fsSL https://raw.githubusercontent.com/EnricoDiMaria/lapis/main/install.sh | bash
```

If Lapis is already installed, the script provides an interactive management menu for updating the application, installing or repairing the transcription model, reinstalling desktop integration, and uninstalling Lapis.

You can also invoke individual actions:

```bash
# Update Lapis
curl -fsSL https://raw.githubusercontent.com/EnricoDiMaria/lapis/main/install.sh | bash -s -- update

# Download or repair the Parakeet model
curl -fsSL https://raw.githubusercontent.com/EnricoDiMaria/lapis/main/install.sh | bash -s -- model

# Uninstall Lapis
curl -fsSL https://raw.githubusercontent.com/EnricoDiMaria/lapis/main/install.sh | bash -s -- uninstall
```

### 2. Local / offline installation

If you have already downloaded the Linux archive from [Releases](../../releases/latest), or compiled the application yourself, you can install it locally:

```bash
chmod +x install.sh

# Automatically detect a local archive or release binary
./install.sh local

# Or specify an archive explicitly
./install.sh /path/to/lapis-linux-x86_64.tar.gz
```

## Windows

Lapis runs natively on **Windows 10 and 11 (x86_64)**.

### 1. Installer (recommended)

Download `lapis-<version>-windows-x86_64-setup.exe` from [Releases](../../releases/latest) and run it.

- It installs for the current user (no administrator rights needed), adds a Start menu entry and an **Open with → Lapis** entry for PDFs.
- Optional steps: a desktop icon, setting Lapis as the default PDF reader, downloading the offline **Parakeet** model (~650 MB), and downloading **ffmpeg**.
- The *Microsoft Visual C++ Runtime* is downloaded from Microsoft if it is missing (Windows asks for administrator approval).
- **Updating:** run the newer installer over the existing installation. It closes a running Lapis, keeps your data, and asks before replacing a newer version with an older one.
- **Uninstalling:** *Settings → Apps*. You can choose whether to also remove the data Lapis keeps on the computer (models, audio recordings, note backups, dictionaries and settings). Notes saved inside your PDF files are not affected.

> The installer is not code-signed yet, so Windows SmartScreen may show "Windows protected your PC": choose **More info → Run anyway**. Each release includes `SHA256SUMS-windows.txt` to verify the download:
> `Get-FileHash -Algorithm SHA256 .\lapis-<version>-windows-x86_64-setup.exe`

### 2. PowerShell script

`install.ps1` is the Windows counterpart of `install.sh`. Download it from the repository (`packaging/windows/install.ps1`) and run:

```powershell
powershell -ExecutionPolicy Bypass -File install.ps1            # install (interactive)
powershell -ExecutionPolicy Bypass -File install.ps1 update     # update
powershell -ExecutionPolicy Bypass -File install.ps1 model      # download or repair the Parakeet model
powershell -ExecutionPolicy Bypass -File install.ps1 ffmpeg     # install ffmpeg (winget)
powershell -ExecutionPolicy Bypass -File install.ps1 local      # install from a local archive or build
powershell -ExecutionPolicy Bypass -File install.ps1 uninstall
```

### ffmpeg

Lapis uses [ffmpeg](https://ffmpeg.org/) to import audio and video, change the playback speed and convert recordings. On Windows it is **not bundled**: let the installer download it, run `winget install Gyan.FFmpeg`, put `ffmpeg.exe` next to `lapis.exe`, or set `LAPIS_FFMPEG` to its path. Lapis looks for it in that order: `LAPIS_FFMPEG`, the Lapis folder, then `PATH`.

### Where Lapis keeps its files on Windows

| What | Location |
|---|---|
| Application | `%LOCALAPPDATA%\Programs\Lapis` |
| Settings and theme | `%APPDATA%\lapis` |
| Note backups, audio, models, dictionaries, caches | `%LOCALAPPDATA%\lapis` |
| Crash log | `%LOCALAPPDATA%\lapis\crash.log` |

---

## Build from source

Install the [Rust toolchain](https://rustup.rs/), including `cargo` and `rustc`.

### Linux

Install the system dependencies required by the PDF and audio backends.

**Arch Linux / EndeavourOS**

```bash
sudo pacman -S base-devel cmake clang pkgconf alsa-lib xcb-util-cursor libxkbcommon
```

**Ubuntu / Debian**

```bash
sudo apt install build-essential cmake libclang-dev pkg-config libasound2-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev libfontconfig1-dev
```

Clone and compile:

```bash
git clone https://github.com/<YOUR-USERNAME>/lapis.git
cd lapis
cargo build --release
```

To install the locally compiled binary on Linux:

```bash
./install.sh local
```

### Windows

Native builds must be made on Windows (the PDF backend is compiled with MSBuild).

1. Install Rust with the **MSVC** toolchain: `rustup default stable-x86_64-pc-windows-msvc` (Rust 1.89 or newer).
2. Install **Visual Studio 2022 Build Tools** with the *Desktop development with C++* workload (MSVC v143 and the Windows SDK).
3. Install **LLVM** (`winget install LLVM.LLVM`) and set `LIBCLANG_PATH` to `C:\Program Files\LLVM\bin`.
4. Enable long paths: `git config --system core.longpaths true`.

```powershell
git clone https://github.com/<YOUR-USERNAME>/lapis.git
cd lapis
$env:CARGO_TARGET_DIR = "C:\t"      # short path: the PDF backend has a deep build tree
cargo build --release
```

The executable is `C:\t\release\lapis.exe`. To build the installer, install [Inno Setup](https://jrsoftware.org/isinfo.php) and run `iscc packaging\windows\lapis.iss /DAppVersion=<version>` (the GitHub Actions workflow `release-windows.yml` does all of this on tag push).

Run the tests with `cargo test`.

---

# Parakeet Setup — Offline Transcription

Lapis supports offline speech recognition through **Parakeet TDT 0.6B v3**, using native ONNX inference.

No Python environment or external transcription service is required.

The model is needed for automatic lecture and voice-memo transcription. PDF viewing, annotation, and note-taking do not depend on it.

## Automatic installation

On Linux, run:

```bash
./install.sh model
```

On Windows, select the model download in the installer, or run `powershell -ExecutionPolicy Bypass -File install.ps1 model`.

The installer downloads the model files into the application model directory.

## Manual installation

The model directory must contain:

- `vocab.txt`
- An encoder ONNX model, such as `encoder-model.int8.onnx`
- A decoder/joint ONNX model, such as `decoder_joint-model.int8.onnx`

### Linux

```bash
MODEL_DIR="$HOME/.local/share/lapis/models/parakeet-tdt-0.6b-v3-int8"

mkdir -p "$MODEL_DIR"

BASE_URL="https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main"

curl -fL "$BASE_URL/vocab.txt" -o "$MODEL_DIR/vocab.txt"
curl -fL "$BASE_URL/encoder-model.int8.onnx" -o "$MODEL_DIR/encoder-model.int8.onnx"
curl -fL "$BASE_URL/decoder_joint-model.int8.onnx" -o "$MODEL_DIR/decoder_joint-model.int8.onnx"
```

### Windows

```powershell
$ModelDir = "$env:LOCALAPPDATA\lapis\models\parakeet-tdt-0.6b-v3-int8"
New-Item -ItemType Directory -Force $ModelDir | Out-Null

$BaseUrl = "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main"

curl.exe -fL "$BaseUrl/vocab.txt" -o "$ModelDir\vocab.txt"
curl.exe -fL "$BaseUrl/encoder-model.int8.onnx" -o "$ModelDir\encoder-model.int8.onnx"
curl.exe -fL "$BaseUrl/decoder_joint-model.int8.onnx" -o "$ModelDir\decoder_joint-model.int8.onnx"
```

## Optional environment variables

| Variable | Description |
|---|---|
| `LAPIS_ASR_MODEL` | Path to a custom Parakeet model directory. |
| `LAPIS_ASR_THREADS` | Number of CPU threads allocated to transcription inference. |

---

# How Lapis Works

A short tour of the technology and the design decisions behind Lapis, for contributors and curious users.

## Stack

| Area | Technology |
|---|---|
| Language | Rust (2024 edition) |
| UI | [egui](https://github.com/emilk/egui) / eframe with the `wgpu` renderer; X11 and Wayland on Linux, native window on Windows |
| PDF rendering and text | [MuPDF](https://mupdf.com/) through the `mupdf` crate |
| PDF editing and export | `lopdf` (page tree, content streams, Form XObjects) |
| Markdown preview | `egui_commonmark`; LaTeX expressions are pre-processed for display and export |
| Audio | `cpal` (capture), `rodio` (playback), `hound` (WAV); `ffmpeg` as an external helper for import and conversion |
| Speech recognition | `parakeet-rs` (ONNX Runtime), Parakeet TDT 0.6B v3 running offline |
| Spellchecking | `spellbook` with Hunspell dictionaries |
| Dialogs, clipboard | `rfd` (non-blocking, so tiling compositors such as Hyprland do not flag the app as unresponsive), `arboard` |

## Source layout

| File | Responsibility |
|---|---|
| `src/main.rs` | Application state, UI, viewing modes, annotation tools, search, flashcards |
| `src/notes.rs` | Notes and annotation data model, storage inside the PDF, local backups, page operations |
| `src/export.rs` | PDF export (vector pipeline and rasterised fallback) and note layout |
| `src/docid.rs` | Stable document identity and migration of legacy data |
| `src/audio.rs` | Audio workspace, recording, voice memos, transcription |
| `src/state.rs`, `src/theme.rs` | Persistent settings, recent files, themes |
| `src/paths.rs`, `src/sysproc.rs` | Per-OS data locations and helper processes (ffmpeg) |
| `src/screenshots.rs`, `src/clipboard.rs`, `src/dialogs.rs`, `src/spellcheck.rs`, `src/doc_lock.rs` | Snipping and clipboard, file dialogs, spellchecking, one-process-per-document lock |

## Where your work is stored

- **Notes and annotations live inside the PDF**, in a small JSON block appended after the final `%%EOF`. Other PDF readers ignore it, so the file stays a valid PDF and your work travels with it.
- A **local backup** of the same data is written first on every save (it survives a PDF that is locked or read-only).
- Every document has a random **document ID** stored in that block. Local data (note backup, audio folder, original-PDF copy) is keyed by the ID rather than by the file path, so moving or renaming a PDF, or opening it on another computer, keeps everything connected. A copy of a file made on the same computer gets its own ID.
- Each save records a **revision**. When the PDF and the local backup disagree, the newest revision wins as a whole (so deletions propagate too) and the discarded version is kept as `notes/<id>.conflict-<timestamp>.bak` (the latest five).
- Annotations are stored in coordinates normalised to the page (0–1), so they follow the page at any zoom and in the export.
- Audio recordings, transcripts, voice memos, screenshots and dictionaries are stored only in the local data folder.

| Data | Linux | Windows |
|---|---|---|
| Settings, theme | `~/.config/lapis` | `%APPDATA%\lapis` |
| Notes backups, audio, models, dictionaries | `~/.local/share/lapis` | `%LOCALAPPDATA%\lapis` |

## PDF export

Exporting does not re-draw the document. The original PDF is opened, pages without notes are left exactly as they are (with the annotation layer added on top), and pages with notes are rebuilt as *slide + notes* using the original page as an embedded vector object. This keeps page size, resolution, selectable text and bookmarks. Encrypted or damaged files fall back to a rasterised export.

## Cross-platform notes

- Per-OS paths are centralised in `paths.rs`; on Linux the layout is unchanged from earlier versions.
- On Linux, clipboard images can also be read through `wl-paste` / `xclip` as a fallback; on Windows the clipboard goes through `arboard`.
- Helper processes (ffmpeg) are started without a console window on Windows.
- A per-document OS lock prevents two Lapis processes from opening the same PDF at once.

## Development

```bash
cargo build --release
cargo test
```

Releases are built by GitHub Actions: pushing a `v*` tag builds the Windows installer and portable archive and creates a **draft** release to complete and publish by hand.

---

# Keyboard Shortcuts

Some shortcuts are configurable through **Settings → Keybindings**. The following table lists the fixed shortcuts and the principal default navigation shortcuts visible in the application.

| Shortcut | Action |
|---|---|
| `Ctrl + O` | Open PDF |
| `Ctrl + S` | Save notes and annotations |
| `Ctrl + P` / `Ctrl + K` | Open Command Palette |
| `Ctrl + F` | Open Global Search |
| `Ctrl + Z` | Undo annotation |
| `Ctrl + Y` | Redo annotation |
| `Alt + 1` | Single Page |
| `Alt + 2` | Continuous Scroll |
| `Alt + 3` | Two-Page Book |
| `Alt + 4` | Slideshow |
| `Alt + 5` | Slideshow + Notes |
| `PageUp` | Previous page |
| `PageDown` | Next page |
| `Left / Right` | Previous / Next page when not typing |
| `K / J` | Previous / Next page when not typing |
| `Space` | Audio play / pause, when applicable |
| `Ctrl + Plus` | Zoom in |
| `Ctrl + Minus` | Zoom out |
| `Ctrl + 0` | Reset zoom to 100% |

The configurable shortcut system also provides global previous/next slide navigation and other application actions.

For the complete and current shortcut assignments, open **Settings → Keybindings**.

# Typical Study Workflow

A typical Lapis workflow might look like this:

1. **Open your lecture PDF.**
2. **Navigate through the slides** using thumbnails, the outline, bookmarks, or keyboard navigation.
3. **Annotate important content** with the drawing, highlighting, shape, and text tools.
4. **Write structured notes** in Markdown alongside each slide.
5. **Capture diagrams and equations** using the Snip tool.
6. **Import or record the lecture audio** in the Audio Dock.
7. **Transcribe the recording offline** using Parakeet.
8. **Review slide-associated transcript segments** while listening to the lecture.
9. **Create active-recall exercises** with Occlusion Tapes and Markdown flashcards.
10. **Export your flashcards to Anki** and your notes to Markdown when needed.

The result is a single study document combining the original lecture material, your annotations, your own explanations, and the corresponding audio context.

> ### 📖 [Read the complete User Guide →](docs/USER_GUIDE.md)
> Every feature explained step by step: viewing modes, annotation tools, notes, audio, transcription, flashcards, search, settings and export.

---

# Roadmap

### Done in 1.2.0

- [x] Native **Windows** build with installer and update support.
- [x] Application icon.
- [x] **Better PDF export**: choose annotations, notes, or both; pages without notes stay identical to the original (size, resolution, no borders); pen widths and all annotation types are exported faithfully; compact *slide | notes* layout without large margins.

### Planned

**Documents and export**

- [ ] Alternative export layouts for pages with notes: notes on a following page, notes below the slide, or notes as PDF comments.
- [ ] Fix the *Original PDF* feature.
- [ ] Fix: closing a PDF to go back to the home screen still leaves the document marked as open.
- [ ] Add SHA256 verification to model downloads (installer, `install.sh` and `install.ps1`).

**Viewing and interface**

- [ ] **Smart scientific dark mode**: selective inversion (shader- or MuPDF-based) that darkens white slide backgrounds while keeping figures, plots and coloured lines correct.
- [ ] Fullscreen mode, including tiling compositors such as Hyprland.
- [ ] Frameless toolbar buttons with hover colour feedback and other visual polish.
- [ ] Right-click on the zoom percentage to fit the page.
- [ ] Right-click menu with only the Undo button.
- [ ] Syntax highlighting for more programming languages in code blocks.

**Study tools**

- [ ] Blank pages / whiteboard in split view, or a page-like canvas inside the notes panel.

**Platform and project**

- [ ] Screenshots in the README.
---

# Data & Privacy

Lapis is designed around local document workflows.

Lecture transcription uses the locally installed Parakeet model rather than requiring an external transcription service.

Notes and annotations are stored with the PDF itself and in a local backup; audio recordings, transcripts, voice memos and screenshots are stored in the local Lapis data folder (see [Where your work is stored](#where-your-work-is-stored)). Nothing is uploaded anywhere.

The application does not require a cloud account to use its core PDF annotation and note-taking features. The only network access is the optional download of the transcription model, ffmpeg and the Microsoft Visual C++ Runtime (on Windows), and the update scripts.

---

# License

Lapis is distributed under the **MIT License**.

See [LICENSE](LICENSE) for details.

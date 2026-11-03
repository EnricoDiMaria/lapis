# Changelog

All notable changes to Lapis are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.2.0] - UNRELEASED

### Added

- **Windows support (x86_64).** Lapis now builds and runs natively on Windows 10/11.
  - Windows installer (Inno Setup): per-user install without UAC, Start menu entry,
    "Open with" integration, optional default-PDF association, and optional downloads of
    the Parakeet transcription model and of ffmpeg. The Microsoft Visual C++ runtime is
    fetched from Microsoft when missing. No third-party binaries are bundled.
  - `install.ps1`, the Windows counterpart of `install.sh`
    (`install`, `local`, `update`, `model`, `ffmpeg`, `uninstall`).
  - Application icon embedded in `lapis.exe` and used for the window; no console window
    is shown, and crashes are written to `crash.log` in the data folder.
  - GitHub Actions workflow that builds the Windows installer and a portable zip.
- **Stable document identity.** Every PDF with saved notes now carries a unique ID inside
  the note block appended to the file. Local data (notes backup, audio/media folder,
  original-PDF copy) follows the document when it is moved or renamed, or opened on another
  machine. A copy of a PDF made on the same machine gets its own identity.
- **Revision tracking for notes.** Each save records a revision, so the most recent version
  of the notes is always the one that is loaded.
- **Conflict safety net.** When two versions of the notes differ, the one that is not used is
  kept as `notes/<id>.conflict-<timestamp>.bak` (the latest five are retained).
- `ffmpeg` is looked up next to the Lapis executable (or via `LAPIS_FFMPEG`) before falling
  back to `PATH`.

### Changed

- **Single data folder.** Everything now lives under `lapis/`; notes backups moved from
  `lapispdf/notes` to `lapis/notes`. The old folder is migrated automatically on first run.
- **Per-platform locations.** Linux is unchanged (`~/.config/lapis`, `~/.local/share/lapis`).
  On Windows, settings live in `%APPDATA%\lapis` and large data (transcription models, audio,
  caches) in `%LOCALAPPDATA%\lapis`.
- Local data is no longer keyed by a hash of the file path. Existing data is migrated to the
  new document ID the first time each PDF is opened.
- On launch, a file path passed on the command line may now contain non-UTF-8 characters.
- Minimum supported Rust version is now 1.89.

### Fixed

- Notes edited on another machine (or by another copy of the app) could be silently ignored
  in favour of an older local backup, and notes deleted elsewhere could reappear. The newest
  revision now wins, and the PDF is brought back in sync when it was behind the local backup.
- Local data could become orphaned after a Rust toolchain upgrade, because the path hash it
  was keyed on is not guaranteed to be stable across compiler versions.
- Image paths with backslashes (Windows) no longer break Markdown image links.

### Compatibility and upgrade notes

- Files and data created by previous versions keep working: they are migrated automatically
  and nothing needs to be done manually.
- Migration moves files instead of copying them. To be able to return to 1.1.x, back up
  `~/.local/share/lapis` (and `~/.local/share/lapispdf`) before the first launch of 1.2.0.
- Versions before 1.2.0 ignore revision and document ID information. If a PDF is edited with
  an older version on another machine, its changes to pages that already have notes in the
  local backup may be ignored by 1.2.0. Update all your machines.
- A PDF without saved notes does not carry an ID: moving it before saving any note
  disconnects its audio and original-PDF copy.
- Deleting *all* notes of a document is not propagated to other machines.

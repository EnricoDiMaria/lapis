# Changelog
All notable changes to Lapis are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Standard PDF annotation interoperability (Import & Export):**
    - **Visible in external viewers:** On save or exit, Lapis exports freehand strokes, shapes, highlights, and text boxes as standard PDF annotations (`/Ink`, `/Highlight`, `/FreeText`) inside a single PDF incremental update. Your markup is now natively visible in other PDF readers/editors without needing a separate export.
    - **Import from external readers:** Annotations created in external PDF viewers (`/Ink`, `/Highlight`, `/FreeText`, `/Text`) are automatically imported into Lapis's vector layer when opening the document, handling page rotations and coordinate conversions.
    - **Two-way edit sync:** Exported Lapis annotations embed a lightweight content signature (`/LapisSig` and `/LapisKind`). If you move, resize, or edit a Lapis text box or annotation in an external editor, Lapis detects the change on reopen and updates the local annotation accordingly.
    - **Immediate FreeText rendering (`/AP` streams):** Exported text boxes include a standard PDF Form XObject appearance stream with `WinAnsiEncoding` (supporting Latin-1 accented characters), ensuring text is immediately visible.

- **Redesigned Global Search (`Ctrl + F`):** The search popup has been rebuilt to be clearer and nicer to use.
  - **Cleaner layout:** A rounded search box with a clear button, a live "n of m" counter and a scrollable result list. The old tri-modal indication is gone: every result now shows what it is (Note, Slide, Audio with its timestamp, or Memo) and its page, so the kind of content is obvious at a glance.
  - **Readable results:** Each result shows the text around the match with the matched words highlighted.
  - **Keyboard cycling:** `Enter` jumps to the next result and `Shift + Enter` (or `↑` / `↓`) moves between them, wrapping around at the ends, without leaving the search box. Previous / next buttons are available as well.
  - **Live search:** Results update while typing and the first match is shown right away, like a browser's find. Edits to the note being written are picked up too.
  - **Reset on close:** Closing the popup (`Esc`, the close button or `Ctrl + F` again) clears the query, the results and every highlight.
- **Current match highlighting:** The selected result is highlighted where it lives. On PDF pages the matching text is marked (the current hit in a stronger color, the other hits on the page more faintly), and in the notes editor the match is highlighted and scrolled into view. Selecting a note result opens the notes panel in Edit mode, since Preview cannot show the highlight.

### Changed

- **Zero-bloat hybrid persistence:** Periodic background autosaves now write exclusively to the local backup (`~/.local/share/lapis` / `%LOCALAPPDATA%\lapis`), keeping editing instantaneous and preventing the PDF file from growing on every stroke. The PDF file itself is updated in a single atomic commit on explicit Save (`Ctrl + S`), document switch, or application exit. When committing to a PDF whose last revision was already written by Lapis, Lapis replaces its own previous trailing incremental update instead of stacking new revisions on every session.
- **Stream-based metadata & fast ID lookup:** Lapis's portable metadata block (`%LAPIS_CUSTOM_START` ... `%LAPIS_CUSTOM_END`) is now stored inside an uncompressed PDF stream object rather than as raw bytes after `%%EOF`, keeping `startxref` and `%%EOF` at the physical end of the file. Document ID lookup scans the last 16 MiB first and falls back to a full read only when needed.
- **Jump back / forward moved into the search popup:** The two navigation-history buttons no longer take space in the topbar; they now live in the footer of the search popup. The `Alt + Backspace` shortcuts are unchanged. Moving through results is recorded as a single step in the history, so **Back** returns to the page the search started from instead of replaying every result.

### Fixed

- **External PDF annotations no longer overwritten:** Lapis no longer truncates bytes after the first `%%EOF` marker. Incremental updates appended by external software (such as Microsoft Edge) are preserved intact across saves.
- **The eraser icon now displays properly in the main PDF area as well.**
- **Search results that did not contain the searched text:** Results used to show the beginning of the matching line instead of the match, so most entries looked unrelated to the query. Results now show the text around the match and always contain it.
- **More reliable matching:** Searches are case-insensitive, ignore differences in whitespace and can match phrases that wrap onto the next line of a PDF page. Each occurrence is listed separately, and the results are computed only when the query or the content changes instead of on every frame.
- **PDFs without text:** The PDF text index of a document without any extractable text was rebuilt on every frame while the search was open.

### Compatibility and upgrade notes

- **Automatic migration of existing documents (Backward Compatibility):** PDFs annotated with Lapis 1.3.0 and earlier (using the legacy `---Lapis_NOTES_V1---` trailer after `%%EOF`) are recognized and migrated automatically the first time they are opened and saved in 1.4.0. No manual action is required.
- **Opening 1.4.0 files in older Lapis versions (Forward Compatibility):** Because 1.4.0 stores portable metadata inside a standard PDF stream using the new `%LAPIS_CUSTOM_START` delimiters, Lapis 1.3.0 and earlier will not read the new embedded metadata block from the PDF (though standard PDF annotations will still be visible if rendered). Please update Lapis to 1.4.0 across all your machines.
- **Deletion safety net:** Deleting a Lapis-created annotation inside an external viewer (e.g. Microsoft Edge) does not delete it from Lapis's local backup on reopen, protecting study-specific items (such as Occlusion Tapes and custom shapes) from accidental external removal. To permanently delete a Lapis annotation, remove it inside Lapis.

## [1.3.0] - 2026/11/08 

### Added

- **UI/UX redesign and visual polish:** Reworked the application’s visual language to create a more consistent, compact and polished experience. Refined the topbar, spacing, surfaces, borders, hover feedback and toast notifications, while preserving existing functionality.
  - **Topbar and file menu:** Removed the enclosing topbar frame, improved button spacing and interaction feedback, and reorganized the File dropdown into clearer sections. Made the central File button and menu actions more obviously clickable, with improved hover states and a clearer Settings entry.
  - **Right panel layout:** Reorganized the notes panel controls into a compact inline toolbar spanning the available width. Positioned **Sketch** to the left of **Edit**, keeping it compact while allowing Edit and Preview to expand and receive greater visual prominence.
  - **Responsive notes panel:** Improved the panel's resizing behavior, introducing earlier responsive collapsing to prevent controls and content from becoming squeezed or cropped. Restored slightly reduced horizontal padding and ensured the toolbar remains stable when switching between modes.
  - **Edit and Preview alignment:** Standardized text positioning, insets and available width between Edit and Preview to minimize visual movement when switching modes. Improved text wrapping during resizing and removed the unwanted Preview outline, restoring its background to match the surrounding panel.
  - **Sketch mode consistency:** Prevented the right panel from shifting vertically when selecting Sketch and corrected the horizontal alignment of the bottom audio controls so they remain in the same position across modes.
  - **Button styling:** Refined the button appearance to balance the frameless look with clear interactive affordances. Added subtle resting-state boundaries and distinct hover feedback without changing button dimensions.
  - **Toast notifications:** Updated toast backgrounds, borders, shadows and corner radii to better integrate with the application's overall theme.

- **Note Editor**: Added dynamic scrolloff behavior to the notes editor, automatically keeping a 2.5-line visual margin
  around the cursor during typing and keyboard navigation.Added adaptive bottom padding to the TextEdit to maintain proper
  scrolling clearance on the final line and fixed a 1-frame flash artifact on the scroll indicator pill.
- **Sketch: a scratchpad for every page.** A new **Sketch** tab in the notes panel, to the left of Edit
  and Preview, gives each PDF page its own drawing surface (`Alt  S`, configurable, also in the Command
  Palette). It is just a background (blank, ruled, grid or dotted, chosen page by page) plus strokes on a
  canvas that fills the whole panel below its toolbar and grows as you draw, so it works the same in a
  narrow side panel and in a wide bottom panel. It has its own toolbar (hand, pen, eraser, line, arrow,
  rectangle, circle, text, colors, sizes, undo / redo and a menu for the background and zoom). You move
  around with the hand tool, the middle mouse button or the wheel, and the canvas also scrolls by itself
  when you draw near the edge of the panel; a thin scroll bar shows where you are and whether there is
  more canvas. `Ctrl + wheel` or pinch zooms around the cursor. Sketches are saved with the document (notes data, backup and PDF trailer),  follow their page when pages are inserted, deleted or reordered, and can be turned off in
  *Advanced Settings*.
- **Sketches in the PDF export.** They are exported together with the notes, cropped to the drawing: to
  the right of the slide when the page has no notes, under the notes when these are short enough to leave
  room, otherwise on a page of their own right after the slide and its notes.
- The Notes panel header collapses more gracefully in narrow panels: the Sketch tab shrinks to an icon
  first, then Edit and Preview, and the page navigation (lock, arrows) is more compact.
- **Complete Home Screen Redesign.** Redesigned *Open PDF* and *Blank Notebook* cards with stable borders, dedicated Nerd Font icons (`󰝰` and `󰎔`), and optimized click targets. **Grid & List Views:** Introduced toggle views for *Recent documents* (grid view with flush thumbnails, progress bars; and list view with full-row click targets).


- **Active Recall & Flashcards Trainer ()**:
  - **Slide Sync & Peek Workflow**: Added a `󰈙 Go to Slide` action that jumps the PDF view to the card's source page and hides the modal so you can inspect the slide without losing your review progress.
  - **Persistent Session State**: Closing the flashcards modal (`Esc` or backdrop click) mid-review now preserves your current card position, queue order, and session stats, resuming right where you left off when re-opened.
  - **Document-Wide Flashcard Indicator**: Added an accent notification dot on the toolbar's Flashcards button whenever flashcards are detected anywhere in the open document.
  - **Fixed Flashcard Re-queuing Count (`Again [1]`)**: Re-queuing a missed card now rotates it to the back of the remaining queue in-place instead of appending duplicate clones, keeping total deck counts accurate and preventing duplicate entries in Anki exports.
  - **Fixed Flashcards Modal Header Layout**: Resolved an issue where the `Refresh`, `Export Anki`, and `Close` buttons could overlap the modal title, and added dedicated scroll containers for long Markdown questions and answers.

### Fixed
- Textboxes are now rendered on top of screenshots added in the PDF page.
- Screenshots and snips are now correctly exported
- Overall code improvement

## [1.2.1] - 2026/11/06 

### Added

- **Fullscreen outside slideshow.** `F11` (configurable in Settings → Keybindings, also available in
  the Command Palette) toggles fullscreen in every view mode. The window command is now sent only when
  the wanted state changes, and changes made from outside (e.g. a Hyprland keybinding) are followed,
  so fullscreen works in tiling compositors.
- Key `C` toggles between Continuous and Single Page view (configurable, also in the Command Palette).
- **Syntax highlighting for many more languages** in fenced code blocks (C/C++, Java, Go, JavaScript,
  Python, Ruby, PHP, SQL, shell, Haskell, Lua, Scala, ... through syntect). Common aliases such as
  `ts`, `tsx`, `jsx`, `kotlin`, `zsh` or `postgres` map to the closest available syntax.

### Fixed

- Closing a PDF to go back to the home screen left the document marked as open: the per-document
  lock was never released, so the file still counted as open (other Lapis windows refused to open
  it and its storage cleanup actions stayed disabled). The lock is now released on close.
- Zoom with the mouse wheel / trackpad is anchored to the cursor position again in Single Page mode.
- Switching from continuous to single page now correctly update the texture of the page.

## [1.2.0] - 2026/11/05

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
- **Choose what to export.** The PDF export can now contain annotations only, notes only, or
  both (Settings → System & Export → PDF Export → Export content). The FILE menu button shows the
  current mode and the output file is named accordingly (`_annotated`, `_notes`, `_annotated_notes`).
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
- **PDF export keeps the original pages.** Pages without notes are now the original pages (same
  size, resolution, selectable text, links and bookmarks, no borders), with the annotations drawn on
  top. Pages with notes show the slide on the left at its original size, without margins, and the
  notes on the right, using the slide width set in the settings. Files that cannot be processed this
  way (encrypted or damaged) fall back to the previous rasterised export.
- The user guide moved to `docs/USER_GUIDE.md`; the README gained Windows instructions, a technical
  overview and an updated roadmap.
- Minimum supported Rust version is now 1.89.

### Fixed

- Notes edited on another machine (or by another copy of the app) could be silently ignored
  in favour of an older local backup, and notes deleted elsewhere could reappear. The newest
  revision now wins, and the PDF is brought back in sync when it was behind the local backup.
- Local data could become orphaned after a Rust toolchain upgrade, because the path hash it
  was keyed on is not guaranteed to be stable across compiler versions.
- Image paths with backslashes (Windows) no longer break Markdown image links, and screenshots load
  on Windows.
- PDF export used fixed stroke widths: pen, line, shape and underline widths now match the width
  used in the app.
- PDF export drew highlights and occlusion tapes as outlines: highlights are now translucent fills
  and tapes are opaque boxes, as on screen.
- PDF export lowered the resolution of every page; pages are now kept as vector content.

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

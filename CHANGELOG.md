# Changelog
Changes introduced in Lapis 1.4.0 relative to 1.3.0 are documented here.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.4.0] - 2026/11/10

### Added

- **Standard PDF annotation interoperability:**
  - **External export & immediate rendering:** On save or exit, freehand strokes, shapes, highlights, and text boxes export as standard PDF annotations (`/Ink`, `/Highlight`, `/FreeText`) in a single incremental update for native visibility in external viewers. Text boxes include a `WinAnsiEncoding` Form XObject appearance stream (`/AP`) supporting Latin-1 accents for immediate visibility.
  - **External import & two-way sync:** External annotations (`/Ink`, `/Highlight`, `/FreeText`, `/Text`) import into Lapis's vector layer on open, handling page rotations and coordinate conversions. Exported items embed `/LapisSig` and `/LapisKind` signatures so external moves, resizes, or edits are detected and synced locally on reopen.

- **Redesigned Global Search (`Ctrl + F`):**
  - **Cleaner layout & readable snippets:** Rebuilt with a rounded search box, clear button, live "n of m" counter, and scrollable results. Replacing the old tri-modal indicator, entries now badge their content type (Note, Slide, Audio with timestamp, or Memo), page number, and surrounding context with matched words highlighted.
  - **Live search & navigation:** Results update as you type (including live edits in the active note) and immediately jump to the first match. Cycle results with wrap-around via `Enter`, `Shift + Enter`, `↑` / `↓`, or UI buttons without leaving the input box.
  - **In-place highlighting & reset on close:** PDF pages highlight all matches (stronger color for the active hit, fainter for others), while note matches scroll into view and open in Edit mode (as Preview cannot display highlights). Closing via `Esc`, the close button, or `Ctrl + F` clears the query, results, and all highlights.

- **Multilingual Hunspell:**
  - **Dictionary management:** Load matching `.aff`/`.dic` pairs with affix rules. Select `.dic` and `.aff` together, or import either file now and complete its pair later (incomplete dictionaries show a warning and remain inactive). Shows both file paths and retains disabled dictionaries, including bundled English.
  - **Simultaneous checking & smart suggestions:** Check across all enabled languages simultaneously, tag suggestions by source language, and display up to five closest merged matches regardless of dictionary load order. Simple letter edits are prioritized over word splits or hyphenation, while retaining split suggestions.

- **Sketch Spacebar drawing:** Hold `Space` and move the pointer to sketch with the pen, shapes, or eraser without holding down a mouse button.
- **Context menus & radial tools:**
  - **PDF & Notes context menus:** PDF text selection now offers Copy; Notes Edit offers Copy/Cut/Paste (Paste is available even without an active selection); Notes Preview offers Copy.
  - **Radial menu enhancements:** Added an image-paste satellite button (visible only when an image is available in the clipboard) and right-click selection support without closing the menu.

- **Flashcard notification preference:** Added an Advanced setting to hide the toolbar flashcard notification dot while keeping the Flashcards button visible.

### Changed

* **Zero-bloat hybrid persistence:** Background autosaves write exclusively to the local backup (`~/.local/share/lapis` / `%LOCALAPPDATA%\lapis`) for instant editing without file bloat. The PDF updates in a single incremental commit on explicit Save (`Ctrl + S`), document switch, or exit, replacing Lapis's own prior trailing incremental update rather than stacking revisions.
* **Stream-based metadata & fast ID lookup:** Portable metadata (`%LAPIS_CUSTOM_START` ... `%LAPIS_CUSTOM_END`) is now stored inside an uncompressed PDF stream object instead of raw bytes after `%%EOF`, keeping `startxref` and `%%EOF` at the physical end of the file. Document ID lookup scans the last 16 MiB before falling back to a full read.
* **Unified glass UI:** The FILE, zoom, view-mode, overflow (`⋯`), bookmark, and right-click menus (page style, spelling, voice memos) now replace the default `egui` frame with a rounded, translucent glass style controlled by `MENU_GLASS_ALPHA`.
* **Study & Canvas settings:** Organized annotation, panel, audio, recall, and presentation controls into clearly labeled groups without removing options.
* **Slideshow + Notes header:** Redesigned the note header with compact, frameless topbar-style spacing and a smaller pin control.
* **Annotation bar close behavior:** Closing the top or vertical annotation bar now restores the Cursor / Select tool by default (an Advanced checkbox lets users retain the active tool).
* **Topbar navigation:** Removed the rarely used topbar back/forward buttons.

### Fixed

* **Preserved external PDF annotations:** Lapis no longer truncates bytes after the first `%%EOF`, preserving incremental updates appended by external viewers like Microsoft Edge.
* **Context menu selection & clipboard handling:** Preserved the editor selection range before right-click handling in Notes Edit so Copy/Cut work reliably, and cached Preview label selections to write directly to the system clipboard on Copy.
* **Accurate search snippets & matching:** Snippets now center around the matched text rather than the start of the line. Searches are case-insensitive, whitespace-agnostic, match across wrapped PDF lines, list every occurrence separately, and recompute only when the query or content changes (rather than on every frame).
* **Textless PDF indexing:** Fixed an issue where PDFs without extractable text rebuilt their text index on every frame while search was open.
* **Eraser icon:** Fixed display issues with the eraser icon in the main PDF area.

### Compatibility and Upgrade Notes

* **Automatic backward migration:** Documents from Lapis 1.3.0 and earlier (using the legacy `---Lapis_NOTES_V1---` trailer after `%%EOF`) migrate automatically on their first open and save in 1.4.0.
* **Forward compatibility:** Lapis 1.3.0 and earlier cannot read 1.4.0's stream-based `%LAPIS_CUSTOM_START` metadata block (though standard rendered PDF annotations remain visible). Update all machines to 1.4.0.
* **Deletion safety net:** Deleting a Lapis annotation in an external viewer will not remove it from Lapis's local backup on reopen, protecting study-specific items like Occlusion Tapes and custom shapes. Permanent deletion must be done inside Lapis.

# Changelog
All notable changes to Lapis are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Annotation cursors:** Tool-specific PDF cursors with a thin pen ring, chisel-tip highlighter cursor and stable, unframed tool glyphs; the topbar tools button shows the active tool.
- **PDF and workspace shortcuts:** Configurable single-key shortcuts for annotation tools, Fit Page (`F`), Fit Width (`W`), night mode (`M`), the annotation toolbar (`D`), notes (`N`), Edit / Preview (`X`) and outline (`G`). `Alt+F` still fits both the PDF and notes panel. Each configurable shortcut can be disabled without losing its binding. Keybindings are grouped by function, with bulk enable/disable controls and current shortcut hints in toolbars, the radial menu and the Command Palette. Former default panel bindings migrate to the new layout without replacing custom bindings.

- **Unicode font fallback:** Use installed system fonts for missing characters in Markdown notes and other interface text, while preserving custom fonts and Nerd Font icons.

- **Independent annotation toolbar placement:** Choose Top, Bottom, or Follow topbar under **Settings → Tools & Study → Annotation toolbar**. The vertical rail remains available.

- **Fit PDF and notes layout:** A button beside the notes page lock, configurable **Alt+F** shortcut and Command Palette action maximize the PDF page or book spread, then give the remaining width to the right notes panel. The panel can expand beyond its former width limit and remains manually resizable.

- **Catppuccin:** Choose Latte, Frappe, Macchiato, or Mocha, then select one of the 14 accent colors.
- **Saved custom themes:** Save a named palette, recall it from the custom-theme menu, or explicitly update an existing name. JSON import/export remains available.
- **Topbar position:** The topbar can sit at the bottom of the window instead of the top, under **Settings → Appearance → Topbar → Position** (or **Move Topbar to Bottom / Top** in the Command Palette). Its menus, the bookmark popup and the search popup open upwards, auto-hide reveals it from the bottom edge, and notifications stay above it.
- **Auto-hide topbar:** An optional mode hides the topbar and reveals it as an overlay when the pointer reaches the edge the topbar sits on. It stays visible while a menu or popup opened from it (File menu, zoom, page and overflow menus, right-click menus, bookmarks, search) is open. **Alt+T** (also **Toggle Topbar Auto-hide** in the Command Palette) turns auto-hide on or off, and the setting is under **Settings → Appearance → Topbar**. The choice is saved.
- **Per-file view memory:** Each document remembers its view mode and the size of its notes panel (side, bottom, and Slideshow + Notes). The view mode is saved as soon as it changes; slideshows reopen in the mode they were started from.
- **Interface density:** Choose Standard, Compact, or Ultra-compact under **Settings → Appearance → Interface density**. The whole app follows it: the topbar (about 7 px / 11 px shorter), annotation toolbars, notes panel header, sidebar tabs, split-view header, and the global spacing, padding, menu/window margins, and default text size used by menus, dialogs, and settings. Notes text keeps its own font-size setting. Standard remains the default, and an earlier compact setting is kept as Compact.
- **Sketch indicator:** The Sketch tab of the notes panel shows the same dot as Notes and Flashcards when the page has a sketch.
- **Exclusive theme sources:** Select Presets, Catppuccin, or Custom; only the active source’s controls are shown. Preset entries retain their original accent colors and background previews. Palette editing switches to Custom.

### Changed

- **Annotation colors:** Pen, highlighter, textboxes, underline, shapes and occlusion tape remember their own last-used colors. The pen defaults to blue and the highlighter to yellow. Palette selections apply to the next annotation, even when selected before switching tools.

- **Radial annotation menu:** Reduced shadow spread and opacity, refined translucent surfaces, subtle accent-colored ring borders, tool selection, color swatches and hover labels.

- **Consistent page scrolling:** The mouse wheel and middle-button drag now move the page the same way in every view mode. Before, Continuous Scroll used a faster wheel and drag than Single Page, and the two-page book view and the Split-View pane ignored middle-button dragging. Both speeds are adjustable under **Settings → Workspace → Page scrolling** (defaults: wheel 2.2×, drag 4.0×).
- **Advanced Settings layout:** Every page now uses the same structure: a title with a description, titled cards for related options, and a short explanation under each option. Options are regrouped into seven sections (Appearance, Tools & Study, Workspace, Keybindings, Export & PDF, Storage & Data, Dictionaries); no option was removed. The Study & Canvas options are split into Tools & Study and Workspace, choices such as toolbar position, density and page style use the same button style, and the audio option label now reads "recording" instead of "registation".
- **Textbox controls:** Compact translucent toolbar with glass formatting and color popups, a fixed-width Edit/Done action at the right edge, reliable formatting-control focus, and consistent text positioning while editing and viewing. An icon toolbar action copies the whole textbox.

- **Readable theme controls:** Selected controls throughout the workspace, including Sketch/Edit/Preview, formatting buttons, and toolbars, use contrasting palette tones even with emphasized labels. Active radial tools use the same palette-based contrast; hover/pressed widget text adapts without altering Catppuccin’s surface colors.

- **Code and math in notes:** Spellchecking no longer underlines words inside code (fenced blocks and inline code) or LaTeX (`$…$`, `$$…$$`), and flashcards are no longer extracted from them (for example `std::vector` is not a `::` card). Code blocks inside a flashcard answer are kept intact.
- **Spelling suggestions:** Every valid single-letter edit (missing, extra, swapped or wrong letter, using only the letters of each dictionary) is now offered even when Hunspell does not return it, and a cut-off word ranks its completion first (`goin` → `going`). Ranking now treats a missing or wrong accent as a minor error, favors candidates that keep the first letter and the typed capitalization, and uses Hunspell's own order to break ties instead of alphabetical order. Suggestions identical to the typed word are dropped.

### Fixed

- **Audio shutdown:** Open output devices only when playback starts and fade active audio before closing the application to reduce audible pops.

- **Drawing responsiveness:** Avoid duplicate local backup writes when finishing an annotation stroke.

- **Keybindings layout:** Prevent overlapping action labels and key combinations in both configurable and fixed shortcuts, including narrow settings windows.

- **Auto-hide toolbar layout:** When both horizontal bars share an edge, the revealed topbar stays outermost and the annotation toolbar docks directly inward without leaving a gap or covering the PDF canvas, including the last page in Continuous Scroll. Notifications account for both bars at the bottom. Notes and Flashcards notification dots retain their corner overlap without clipping in auto-hide mode.

- **Notes panel resizing:** Reuse cached PDF textures during a resize and defer automatic Continuous Scroll page changes until release, reducing rendering work and session writes while keeping the layout responsive.

- **Slideshow:** Keep the notes panel width stable when interacting with code blocks, and refresh PDF rendering at the fullscreen viewport resolution.
- **Markdown preview:** Preserve shell arguments such as `$@` and dollar signs inside inline and fenced code instead of interpreting them as LaTeX.

- **Textboxes after an external edit:** Saving a PDF in another viewer no longer resets the background, bold and italic of Lapis textboxes. The textbox signature now ignores the box height (grown on export) and the Lapis-only markers; older signatures are still recognized.
- **Textbox size in other viewers:** Text boxes are exported and imported relative to the page height (800-unit convention, like the slides export), so they look the same in Lapis and in other viewers on 16:9 slides and other non-800 pt pages. Older files keep their size.
- **Textbox height in other viewers:** The exported box is fitted to its text instead of keeping a taller stored height, so no empty band (or extra background) appears below the text.
- **Textbox appearance:** The background fill (dark or light) is exported, so light text stays readable elsewhere; typographic characters (’ “ ” – • …) no longer turn into `?`; `__`/`~~` delimiters are hidden and `- ` shows as a bullet, as in Lapis.
- **Imported annotation text:** Text without a byte-order mark is decoded as UTF-8 or PDFDocEncoding, so accented characters from other editors are no longer corrupted.
- **Circle export:** A circle is exported inscribed in its box, as drawn in Lapis, instead of as a stretched ellipse.
- **Page edits:** Deleting, moving or inserting a page now rewrites the embedded Lapis data with the new page numbers immediately, not only at the next save.
- **Eraser cursor:** The circle now scales with the page zoom and sits halfway between the old fixed size and the real erase radius, instead of being much smaller than the area it erases.
- **Sidebar tab sync:** Switching between Thumbnails and Outline/Bookmarks now scrolls the new tab to the current page, even if the page changed while the other tab was open.
- **View switching:** Switching between Single Page and Continuous Scroll keeps your vertical (and horizontal) position instead of jumping to the top of the page.
- **Notes Preview scroll arrow:** The "more below" arrow now disappears at the bottom of the Preview, as in Edit, including when the floating audio buttons are enabled.
- **Zoomed page edges:** The shadow-like fade along the canvas borders is much weaker and narrower when the page is zoomed beyond the window.
- **Settings color picker:** Color popups now appear above Advanced Settings and accept input. Escape dismisses an open popup before closing settings, and document shortcuts remain inactive while settings are open.
- **Textbox export:** Automatically wrapped text is included in the saved PDF appearance, with enough height to display every line. Internal background markers no longer appear as visible characters in external PDF viewers.
- **Textbox outline:** The editing outline and resize handle follow wrapped text immediately while typing or deleting.
- **Textbox layout:** Editing and Done mode use consistent word wrapping. Textboxes automatically fit their text height, growing and shrinking without unused vertical space.

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

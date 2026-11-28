//! Application state and coordination of the document and interface.

use eframe::egui;
use egui::InputState;
use mupdf::{Colorspace, Document, Matrix};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

use self::ZoomMode::FitWidth;
use crate::annotations::{self, *};
use crate::audio::*;
use crate::clipboard::*;
use crate::dialogs::{AsrRetry, DialogKind, DialogSpec, Dialogs};
use crate::persistence::{self, AfterPdfSave};
use crate::screenshots::{ShotJobs, ShotTarget};
use crate::shortcuts::{self, *};
use crate::spellcheck::{self, *};
use crate::state::{self, *};
use crate::theme::{SuiteTheme, rgb};
use crate::viewer::{
    PageText, PdfLinkZone, RenderView, TocItem, Viewer, extract_text, inspect_pdf, outline,
    render_scale, render_to_image,
};
use crate::{
    audio, doc_lock, export, flashcards, markdown, notes, paths, screenshots, search, sketch,
    theme, theme_picker,
};

mod audio_session;
mod document;
mod frame;
mod navigation;
mod pdf_text;
mod save;
#[cfg(test)]
mod tests;

// Feature UI implementations use the workspace's private state.
#[path = "../annotations/workspace.rs"]
mod annotation_tools;
#[path = "../flashcards/trainer.rs"]
mod flashcards_trainer;
#[path = "../markdown/preview.rs"]
mod markdown_preview;
#[path = "../ui/mod.rs"]
mod ui;

use document::{
    SignedPageOp, has_extra_study_content, merge_imported_notes, page_has_notes_or_sketch,
};
use navigation::{SinglePageWheel, clamp_scroll_offset, wheel_page_target, zoom_scroll_offset};
use pdf_text::{PdfSearchIndex, TextCaret, TextSelection};
use save::{PdfSaveFailure, PdfSaveJob};
use ui::{COMMAND_PALETTE_LABEL, MENU_GLASS_ALPHA, SettingsTab, TopbarSlot, truncate_filename};

fn setup_nerd_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    // Using include_bytes! to incorporate in the executable binary (in .rodata) the font. It returns a static reference
    fonts.font_data.insert(
        "nerd_font".to_string(),
        egui::FontData::from_static(include_bytes!(
            "../../assets/JetBrainsMonoNerdFont-Regular.ttf"
        ))
        .into(),
    );

    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, "nerd_font".to_string());
    }

    ctx.set_fonts(fonts);
}

fn crop_slide_region_to_png(
    pdf_path: &Path,
    page_num: u32,
    p1: (f32, f32),
    p2: (f32, f32),
    dark_mode: bool,
) -> Result<PathBuf, String> {
    let min_x = p1.0.min(p2.0).clamp(0.0, 1.0);
    let max_x = p1.0.max(p2.0).clamp(0.0, 1.0);
    let min_y = p1.1.min(p2.1).clamp(0.0, 1.0);
    let max_y = p1.1.max(p2.1).clamp(0.0, 1.0);

    if (max_x - min_x) < 0.015 || (max_y - min_y) < 0.015 {
        return Err("Selection area is too small".to_string());
    }

    let doc = Document::open(pdf_path.to_str().unwrap_or_default()).map_err(|e| e.to_string())?;
    let page = doc
        .load_page((page_num.saturating_sub(1)) as i32)
        .map_err(|e| e.to_string())?;

    // Render at crisp 2.5x resolution for high-DPI diagram clarity
    let scale = 2.5_f32;
    let pix = page
        .to_pixmap(
            &Matrix::new_scale(scale, scale),
            &Colorspace::device_rgb(),
            false,
            false,
        )
        .map_err(|e| e.to_string())?;

    let full_w = pix.width() as usize;
    let full_h = pix.height() as usize;
    let samples = pix.samples();

    let x0 = ((min_x * full_w as f32).floor() as usize).min(full_w.saturating_sub(1));
    let y0 = ((min_y * full_h as f32).floor() as usize).min(full_h.saturating_sub(1));
    let x1 = ((max_x * full_w as f32).ceil() as usize).clamp(x0 + 1, full_w);
    let y1 = ((max_y * full_h as f32).ceil() as usize).clamp(y0 + 1, full_h);

    let crop_w = x1 - x0;
    let crop_h = y1 - y0;
    let mut rgb_buf = Vec::with_capacity(crop_w * crop_h * 3);

    for row in y0..y1 {
        for col in x0..x1 {
            let offset = (row * full_w + col) * 3;
            if offset + 2 < samples.len() {
                if dark_mode {
                    let r = 22 + ((255 - samples[offset]) as u16 * 215 / 255) as u8;
                    let g = 24 + ((255 - samples[offset + 1]) as u16 * 215 / 255) as u8;
                    let b = 31 + ((255 - samples[offset + 2]) as u16 * 212 / 255) as u8;
                    rgb_buf.extend_from_slice(&[r, g, b]);
                } else {
                    rgb_buf.extend_from_slice(&samples[offset..offset + 3]);
                }
            }
        }
    }

    let snips_dir = audio::AudioDocIndex::media_dir_for_pdf(pdf_path).join("snips");
    let _ = std::fs::create_dir_all(&snips_dir);

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let filename = format!("snip_p{}_{}.png", page_num, ts);
    let out_path = snips_dir.join(filename);

    image::save_buffer(
        &out_path,
        &rgb_buf,
        crop_w as u32,
        crop_h as u32,
        image::ColorType::Rgb8,
    )
    .map_err(|e| e.to_string())?;

    Ok(out_path)
}

#[derive(PartialEq, Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum ZoomMode {
    Custom,
    FitWidth,
    FitHeight,
    FitPage,
}

#[derive(PartialEq, Clone, Copy)]
enum NoteMode {
    Edit,
    Preview,
    /// Per-page scratchpad (see `sketch.rs`).
    Sketch,
}

#[derive(PartialEq, Clone, Copy)]
enum SidebarTab {
    Thumbnails,
    Outline,
}

/// Vertical gap between pages in Continuous mode.
const CONTINUOUS_GAP: f32 = 14.0;

#[derive(Debug, PartialEq, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub(crate) enum ViewMode {
    SinglePage,
    Continuous,
    TwoPage,
    Slideshow,
    SlideshowWithNotes,
}

#[derive(Clone, Copy)]
enum ImageDragMode {
    /// Move: normalized pointer offset from the top-left corner.
    Move { grab: (f32, f32) },
    /// Resize: keep the opposite corner fixed.
    Resize { anchor: (f32, f32) },
}

#[derive(Clone, Copy)]
struct ImageDrag {
    page: u32,
    idx: usize,
    mode: ImageDragMode,
}

pub(crate) struct Lapis {
    current_page: u32,
    target_page: u32,
    total_pages: u32,
    current_note: String,
    notes: HashMap<u32, String>,
    study_revision: u64,
    checkpointed_revision: Option<u64>,
    session_save_due: Option<std::time::Instant>,
    pending_signed_page_op: Option<SignedPageOp>,
    /// True only while a confirmed page change is replayed on a signed PDF.
    allow_signature_loss: bool,
    /// Set while local checkpoints fail; throttles the per-frame retry.
    checkpoint_failed_at: Option<std::time::Instant>,
    /// Guards against a merge/retry loop when an external change cannot be acknowledged.
    last_external_merge: Option<std::time::Instant>,
    queued_pdf_save: Option<AfterPdfSave>,
    pdf_save_job: Option<PdfSaveJob>,
    pdf_save_failure: Option<PdfSaveFailure>,
    allow_window_close: bool,
    pdf_path: PathBuf,
    pdf_name: String,
    texture_handle: Option<egui::TextureHandle>,
    page_dims: (u32, u32),
    show_notes: bool,
    focus_note_editor: bool,
    zoom_scale: f32,
    zoom_mode: ZoomMode,
    view_mode: ViewMode,
    prev_view_mode: ViewMode,
    /// View mode last written to `state.json`; a difference triggers a save.
    saved_view_mode: ViewMode,
    /// Auto-hide: time until which a revealed topbar stays (hover grace period).
    topbar_hold_until: f64,
    /// Height of the revealed overlay topbar; 0 while hidden or docked.
    topbar_overlay_h: f32,
    /// Height of the docked topbar in the last frame (0 while hidden or floating).
    topbar_dock_h: f32,
    /// Height of the docked annotation sub-topbar in the last frame (0 when not shown).
    subbar_h: f32,
    /// A popup drawn by the topbar (file menu, bookmark popup) was open in the last frame.
    topbar_popup_open: bool,
    /// Notes-panel sizes of the open document.
    panel_sizes: PanelSizes,
    /// Panels that take the saved size the next time they are shown.
    panel_apply: [bool; 3],
    /// Last size seen per panel, to detect resizes by the user.
    panel_seen: [Option<f32>; 3],
    panel_sizes_dirty: bool,
    notes_panel_resizing: bool,
    fit_notes_layout_pending: bool,
    continuous_scroll_to_page: Option<u32>,
    /// Scroll offset to keep when switching between Single and Continuous:
    /// x, and y relative to the top of the current page.
    mode_switch_offset: Option<egui::Vec2>,
    single_page_wheel: SinglePageWheel,
    continuous_scroll_delta: f32,
    viewer: Viewer,
    is_rendering: bool,
    /// page size in PDF points and on-screen width in points (drive the render scale)
    page_pt: (f32, f32),
    display_w_pt: f32,
    /// first/last visible page (1-based, inclusive) in continuous mode
    visible_pages: (u32, u32),
    show_toc: bool,
    outline: Vec<TocItem>,
    note_mode: NoteMode,
    commonmark_cache: egui_commonmark::CommonMarkCache,
    export_rx: Receiver<Result<PathBuf, String>>,
    export_tx: Sender<Result<PathBuf, String>>,
    is_exporting: bool,
    toast_message: Option<(String, std::time::Instant)>,
    clipboard: SystemClipboard,
    paste_handled_on_press: bool,
    radial_clipboard_image_available: bool,
    theme: SuiteTheme,
    dialogs: Dialogs,
    pending_delete_page: Option<u32>,
    app_state: AppState,
    sidebar_tab: SidebarTab,
    user_bookmarks: Vec<state::BookmarkItem>,
    bookmark_input: String,
    last_synced_sidebar_page: u32,
    /// Sidebar tab that was last scrolled to the current page.
    last_synced_sidebar_tab: SidebarTab,
    annotation_tool: AnnotationTool,
    pending_annotation_color: Option<[u8; 3]>,
    last_annotation_color: [u8; 3],
    annotations: HashMap<u32, Vec<annotations::PageAnnotation>>,
    active_stroke: Option<annotations::PageAnnotation>,
    stroke_width: f32,
    selected_textbox: Option<usize>,
    note_editor_epoch: u64,
    textbox_epoch: u64,
    textbox_font_size: f32,
    textbox_bold: bool,
    textbox_italic: bool,
    pdf_selection: Option<TextSelection>,
    /// True while the primary button is held during text selection.
    selecting: bool,
    /// page rectangles drawn during the previous frame (to resolve cross-page drags)
    slide_rects: Vec<(u32, egui::Rect)>,
    canvas_rect: egui::Rect,
    show_settings: bool,
    settings_tab: SettingsTab,
    rebinding_action: Option<String>,
    show_search: bool,
    focus_search_input: bool,
    search_query: String,
    /// Query the current `search_hits` were computed for.
    search_last_query: String,
    search_hits: Vec<search::Hit>,
    search_sel: usize,
    /// Hash of everything the hits depend on (query, notes, ...): hits are recomputed when it changes.
    search_key: u64,
    search_scroll_to_sel: bool,
    /// Ask the notes editor to scroll to the current hit.
    search_note_scroll: bool,
    pdf_dark_mode: bool,
    hidden_pages: HashSet<u32>,
    shape_kind: ShapeKind,
    trackpad_drawing_active: bool,
    audio_engine: AudioEngine,
    memo_audio_engine: AudioEngine,
    audio_index: AudioDocIndex,
    recording_timeline: Vec<(u64, u32)>,
    asr_rx: Receiver<audio::AsrJobResult>,
    audio_index_pending: bool,
    asr_cancel: Arc<std::sync::atomic::AtomicBool>,
    /// (done_ms, total_ms) of the running lecture transcription
    asr_progress: Option<(u64, u64)>,
    asr_model_dir: Option<PathBuf>,
    is_transcribing: bool,
    audio_dock_open: bool,
    confirm_remove_lecture: bool,
    transcript_all: bool,
    transcript_last_seg: Option<usize>,
    show_align_panel: bool,
    align_from: u32,
    align_to: u32,
    note_page: u32,
    lock_note_page: bool,
    show_audio_trimmer: bool,
    trim_start_ms: u64,
    trim_end_ms: u64,
    /// `None` = trimming the main Lecture Track; `Some((page, memo_idx))` = trimming a specific Voice Memo
    trim_memo_target: Option<(u32, usize)>,
    pdf_search_index: PdfSearchIndex,
    spellchecker: spellcheck::SpellChecker,
    blank_pages: std::collections::HashSet<u32>,
    /// Per-page sketch sheets shown in the notes panel "Sketch" tab.
    sketches: HashMap<u32, notes::SketchPage>,
    sketch: sketch::SketchState,
    split_view_enabled: bool,
    split_secondary_page: u32,
    split_secondary_zoom: f32,
    undo_stack: HashMap<u32, Vec<Vec<annotations::PageAnnotation>>>,
    redo_stack: HashMap<u32, Vec<Vec<annotations::PageAnnotation>>>,
    /// Set of `(page_num, annotation_idx)` occlusion tapes currently peeked/revealed
    revealed_tapes: std::collections::HashSet<(u32, usize)>,
    /// Temporary laser pointer points: `(normalized_pos, timestamp)`.
    laser_trail: Vec<((f32, f32), std::time::Instant)>,
    /// Active rectangle `[start_norm, cur_norm]` while dragging with the Snip tool
    active_snip_rect: Option<(u32, (f32, f32), (f32, f32))>,
    show_tools_rail: bool,
    radial_menu_pos: Option<egui::Pos2>,
    show_flashcards_modal: bool,
    flashcards_deck: Vec<flashcards::Flashcard>,
    flashcard_idx: usize,
    flashcard_revealed: bool,
    flashcard_sync_slide: bool,
    /// Session score counters: `(again_hard, good, easy)`
    flashcard_stats: (u32, u32, u32),
    show_command_palette: bool,
    command_palette_query: String,
    command_palette_selected: usize,
    book_cover_mode: bool,
    zoom_anchor: Option<egui::Pos2>,
    zoom_track: (ZoomMode, f32),
    last_layout: (f32, f32, f32, f32),
    pdf_scroll_offset: egui::Vec2,
    pdf_panning: bool,
    show_audio_handle: bool,
    show_memo_fab: bool,
    rename_memo_target: Option<(u32, usize)>,
    rename_memo_text: String,
    shots: ShotJobs,
    last_slide_rects: Vec<(u32, egui::Rect)>,
    selected_image: Option<(u32, usize)>,
    image_drag: Option<ImageDrag>,
    home_thumb_tx: Sender<(PathBuf, egui::ColorImage)>,
    home_thumb_rx: Receiver<(PathBuf, egui::ColorImage)>,
    home_thumb_cache: HashMap<PathBuf, (egui::TextureHandle, (usize, usize))>,
    home_thumb_pending: HashSet<PathBuf>,
    audio_dock_height: f32,
    jump_lock_y: Option<f32>,
    continuous_anchor_page: u32,
    continuous_anchor_frac: f32,
    continuous_prev_layout: (f32, f32, f32),
    jump_lock_frame: u64,
    reanchor_left: u16,
    last_effective_zoom: f32,
    reanchor_page: u32,
    reanchor_frac: f32,
    reanchor_ref_y: f32,
    scroll_epoch: u32,
    doc_lock: Option<doc_lock::DocLock>,
    /// Fullscreen requested by the user (F11 / command palette), independent from slideshow.
    user_fullscreen: bool,
    /// Last fullscreen state sent to the window system. The command is sent only when the wanted
    /// state changes, so a compositor that is slow (or refuses) is never fought every frame.
    fullscreen_sent: Option<bool>,
    fullscreen_cmd_at: Option<std::time::Instant>,
}

impl Lapis {
    pub(crate) fn new(cc: &eframe::CreationContext<'_>, initial_path: Option<PathBuf>) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        cc.egui_ctx.options_mut(|opt| {
            opt.zoom_with_keyboard = false;
        });

        setup_nerd_fonts(&cc.egui_ctx);
        let theme = SuiteTheme::load_or_init();
        theme.apply(&cc.egui_ctx);
        let app_state = AppState::load();
        theme.apply_density(&cc.egui_ctx, app_state.settings.ui_density);

        let (export_tx, export_rx) = channel::<Result<PathBuf, String>>();
        let asr_rx = channel::<audio::AsrJobResult>().1;
        let (home_thumb_tx, home_thumb_rx) = channel::<(PathBuf, egui::ColorImage)>();
        let spellchecker = spellcheck::SpellChecker::with_preferences(
            &app_state.settings.extra_dictionaries,
            app_state.settings.english_dictionary_enabled,
            &app_state.settings.disabled_dictionaries,
        );

        let mut app = Self {
            current_page: 1,
            target_page: 1,
            total_pages: 1,
            current_note: String::new(),
            notes: HashMap::new(),
            study_revision: 0,
            checkpointed_revision: None,
            session_save_due: None,
            pending_signed_page_op: None,
            allow_signature_loss: false,
            checkpoint_failed_at: None,
            last_external_merge: None,
            queued_pdf_save: None,
            pdf_save_job: None,
            pdf_save_failure: None,
            allow_window_close: false,
            pdf_path: PathBuf::new(),
            pdf_name: String::new(),
            texture_handle: None,
            page_dims: (0, 0),
            show_notes: false,
            focus_note_editor: false,
            zoom_scale: 1.0,
            zoom_mode: FitWidth,
            view_mode: ViewMode::SinglePage,
            prev_view_mode: ViewMode::SinglePage,
            saved_view_mode: ViewMode::SinglePage,
            topbar_hold_until: 0.0,
            topbar_overlay_h: 0.0,
            topbar_dock_h: 0.0,
            subbar_h: 0.0,
            topbar_popup_open: false,
            panel_sizes: PanelSizes::default(),
            panel_apply: [true; 3],
            panel_seen: [None; 3],
            panel_sizes_dirty: false,
            notes_panel_resizing: false,
            fit_notes_layout_pending: false,
            continuous_scroll_to_page: None,
            mode_switch_offset: None,
            single_page_wheel: SinglePageWheel::default(),
            continuous_scroll_delta: 0.0,
            viewer: Viewer::default(),
            is_rendering: false,
            page_pt: (0.0, 0.0),
            display_w_pt: 0.0,
            visible_pages: (0, 0),
            show_toc: false,
            outline: Vec::new(),
            note_mode: NoteMode::Edit,
            commonmark_cache: egui_commonmark::CommonMarkCache::default(),
            export_rx,
            export_tx,
            is_exporting: false,
            toast_message: None,
            clipboard: SystemClipboard::new(),
            paste_handled_on_press: false,
            radial_clipboard_image_available: false,
            theme,
            dialogs: Dialogs::default(),
            pending_delete_page: None,
            app_state,
            sidebar_tab: SidebarTab::Thumbnails,
            user_bookmarks: Vec::new(),
            bookmark_input: String::new(),
            last_synced_sidebar_page: 1,
            last_synced_sidebar_tab: SidebarTab::Thumbnails,
            annotation_tool: AnnotationTool::Cursor,
            pending_annotation_color: None,
            last_annotation_color: ANNOTATION_PALETTE[0].0,
            annotations: HashMap::new(),
            active_stroke: None,
            stroke_width: 2.5,
            selected_textbox: None,
            note_editor_epoch: 0,
            textbox_epoch: 0,
            textbox_font_size: 16.0,
            textbox_bold: false,
            textbox_italic: false,
            pdf_selection: None,
            selecting: false,
            slide_rects: Vec::new(),
            canvas_rect: egui::Rect::NOTHING,
            show_settings: false,
            settings_tab: SettingsTab::AppearanceAndTheme,
            rebinding_action: None,
            show_search: false,
            focus_search_input: false,
            search_query: String::new(),
            search_last_query: String::new(),
            search_hits: Vec::new(),
            search_sel: 0,
            search_key: 0,
            search_scroll_to_sel: false,
            search_note_scroll: false,
            pdf_dark_mode: false,
            hidden_pages: HashSet::new(),
            shape_kind: ShapeKind::Rect,
            trackpad_drawing_active: false,
            audio_engine: AudioEngine::new(),
            memo_audio_engine: AudioEngine::new(),
            audio_index: AudioDocIndex::default(),
            recording_timeline: Vec::new(),
            asr_rx,
            audio_index_pending: false,
            asr_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            asr_progress: None,
            asr_model_dir: None,
            is_transcribing: false,
            audio_dock_open: false,
            confirm_remove_lecture: false,
            transcript_all: false,
            transcript_last_seg: None,
            show_align_panel: false,
            align_from: 1,
            align_to: 1,
            note_page: 1,
            lock_note_page: true,
            show_audio_trimmer: false,
            trim_start_ms: 0,
            trim_end_ms: 0,
            trim_memo_target: None,
            pdf_search_index: PdfSearchIndex::default(),
            spellchecker,
            blank_pages: HashSet::new(),
            sketches: HashMap::new(),
            sketch: sketch::SketchState::default(),
            split_view_enabled: false,
            split_secondary_page: 1,
            split_secondary_zoom: 1.0,
            undo_stack: HashMap::new(),
            redo_stack: HashMap::new(),
            revealed_tapes: HashSet::new(),
            laser_trail: Vec::new(),
            active_snip_rect: None,
            show_tools_rail: false,
            radial_menu_pos: None,
            show_flashcards_modal: false,
            flashcards_deck: Vec::new(),
            flashcard_idx: 0,
            flashcard_revealed: false,
            flashcard_sync_slide: false,
            flashcard_stats: (0, 0, 0),
            show_command_palette: false,
            command_palette_query: String::new(),
            command_palette_selected: 0,
            book_cover_mode: true,
            zoom_anchor: None,
            zoom_track: (ZoomMode::FitWidth, 1.0),
            last_layout: (0.0, 0.0, 0.0, 0.0),
            pdf_scroll_offset: egui::Vec2::ZERO,
            pdf_panning: false,
            show_audio_handle: true,
            show_memo_fab: true,
            rename_memo_target: None,
            rename_memo_text: String::new(),
            shots: ShotJobs::default(),
            last_slide_rects: Vec::new(),
            selected_image: None,
            image_drag: None,
            home_thumb_tx,
            home_thumb_rx,
            home_thumb_cache: HashMap::new(),
            home_thumb_pending: HashSet::new(),
            audio_dock_height: 180.0,
            jump_lock_y: None,
            continuous_anchor_page: 1,
            continuous_anchor_frac: 0.0,
            continuous_prev_layout: (0.0, 0.0, 0.0),
            jump_lock_frame: 0,
            reanchor_left: 0,
            last_effective_zoom: 1.0,
            reanchor_page: 1,
            reanchor_frac: 0.0,
            reanchor_ref_y: 0.0,
            scroll_epoch: 0,
            doc_lock: None,
            user_fullscreen: false,
            fullscreen_sent: None,
            fullscreen_cmd_at: None,
        };

        if let Some(path) = initial_path {
            app.open_pdf(path, &cc.egui_ctx);
        }

        app.apply_custom_font(&cc.egui_ctx);

        app
    }

    #[inline]
    fn has_pdf(&self) -> bool {
        !self.pdf_path.as_os_str().is_empty()
    }

    fn fitted_notes_width(
        width: f32,
        height: f32,
        aspect: f32,
        count: usize,
        min: f32,
        max: f32,
    ) -> f32 {
        let gap = if count == 2 { 2.0 } else { 0.0 };
        let pdf_width = (height - 2.0).max(1.0) * aspect * count as f32 + gap + 2.0;
        (width - pdf_width).clamp(min, max)
    }

    fn fit_notes_target_width(&self, available: egui::Rect, min_w: f32, max_w: f32) -> f32 {
        let (w, h) = self
            .viewer
            .page(self.current_page)
            .map(|(_, dims)| *dims)
            .unwrap_or(self.page_dims);
        let aspect = if w > 0 && h > 0 {
            w as f32 / h as f32
        } else {
            4.0 / 3.0
        };
        let count = if self.view_mode == ViewMode::TwoPage {
            let (left, right) = self.current_book_spread();
            (left.iter().count() + right.iter().count()).max(1)
        } else {
            1
        };
        Self::fitted_notes_width(
            available.width(),
            available.height(),
            aspect,
            count,
            min_w,
            max_w,
        )
    }

    fn notes_layout_needs_fit(zoom: ZoomMode, current: f32, target: f32) -> bool {
        // Ignore small differences from panel borders and texture rounding.
        zoom != ZoomMode::FitPage || (current - target).abs() > 3.0
    }

    fn handle_fit_notes_divider_double_click(
        &mut self,
        ui: &egui::Ui,
        panel: &egui::Response,
        available: egui::Rect,
        min_w: f32,
        max_w: f32,
    ) {
        if !self.can_fit_notes_layout()
            || self.notes_panel_resizing
            || self.pdf_panning
            || self.show_settings
            || self.show_command_palette
            || self.show_flashcards_modal
            || self.show_search
            || self.radial_menu_pos.is_some()
            || !ui.input(|i| {
                i.pointer
                    .button_double_clicked(egui::PointerButton::Primary)
            })
        {
            return;
        }
        let Some(pos) = ui.input(|i| i.pointer.hover_pos()) else {
            return;
        };
        let rect = panel.rect;
        let near_divider =
            (pos.x - rect.left()).abs() <= 5.0 && pos.y >= rect.top() && pos.y <= rect.bottom();
        let ctx = ui.ctx();
        if !near_divider || ctx.layer_id_at(pos) != Some(panel.layer_id) {
            return;
        }
        let target = self.fit_notes_target_width(available, min_w, max_w);
        if Self::notes_layout_needs_fit(self.zoom_mode, rect.width(), target) {
            self.request_fit_notes_layout(ctx);
        }
    }

    fn can_fit_notes_layout(&self) -> bool {
        self.has_pdf()
            && self.show_notes
            && !self.split_view_enabled
            && !matches!(
                self.view_mode,
                ViewMode::Slideshow | ViewMode::SlideshowWithNotes
            )
    }

    fn request_fit_notes_layout(&mut self, ctx: &egui::Context) {
        if self.can_fit_notes_layout() {
            self.fit_notes_layout_pending = true;
            ctx.request_repaint();
        }
    }

    /// Makes `panel` take the document's saved size the first time it is shown.
    fn restore_panel_size(
        &mut self,
        panel: egui::Panel,
        slot: PanelSlot,
        default: f32,
        range: (f32, f32),
    ) -> egui::Panel {
        let idx = slot as usize;
        if !self.panel_apply[idx] {
            return panel;
        }
        self.panel_apply[idx] = false;
        self.panel_seen[idx] = None;
        let size = self.panel_sizes.get(slot).unwrap_or(default);
        panel.exact_size(size.clamp(range.0, range.1))
    }

    /// Stores a size changed by dragging the panel's resize handle.
    fn track_panel_size(&mut self, ui: &egui::Ui, slot: PanelSlot, size: f32) {
        let idx = slot as usize;
        let pointer_active = ui.input(|inp| inp.pointer.any_down() || inp.pointer.any_released());
        if pointer_active && self.panel_seen[idx].is_some_and(|seen| (seen - size).abs() > 0.5) {
            self.panel_sizes.set(slot, size);
            self.panel_sizes_dirty = true;
            if ui.input(|inp| inp.pointer.any_down()) {
                self.notes_panel_resizing = true;
            } else {
                ui.ctx().request_repaint();
            }
        }
        self.panel_seen[idx] = Some(size);
    }

    /// Finish a text box that is still being typed in the Sketch tab (before the page
    /// structure changes, the document is closed, ...).
    fn finish_sketch_edit(&mut self) {
        if self.sketch.commit_pending(&mut self.sketches) {
            self.save_study_edit();
        }
    }

    /// The Sketch tab can be turned off in the advanced settings.
    fn sketch_enabled(&self) -> bool {
        self.app_state.settings.sketch_enabled
    }

    /// Show the Sketch tab (opening the notes panel if needed), or go back to Edit.
    fn toggle_sketch_tab(&mut self) {
        if !self.sketch_enabled() {
            return;
        }
        if self.show_notes && self.note_mode == NoteMode::Sketch {
            self.note_mode = NoteMode::Edit;
            self.focus_note_editor = true;
        } else {
            self.show_notes = true;
            self.note_mode = NoteMode::Sketch;
        }
    }

    fn note_editor_id(&self) -> egui::Id {
        egui::Id::unique(("lapis_note_editor", self.note_editor_epoch, self.note_page))
    }

    fn apply_history_shortcut(&mut self, redo: bool) {
        if self.sketch_enabled()
            && self.show_notes
            && self.note_mode == NoteMode::Sketch
            && self.sketch.has_focus(self.note_page)
        {
            let changed = if redo {
                self.sketch.redo(self.note_page, &mut self.sketches)
            } else {
                self.sketch.undo(self.note_page, &mut self.sketches)
            };
            if changed {
                self.save_study_edit();
                let message = if redo {
                    "󰑎 Redo sketch"
                } else {
                    "󰕌 Undo sketch"
                };
                self.toast_message = Some((message.into(), std::time::Instant::now()));
            }
        } else if redo {
            self.redo_annotation(self.current_page);
        } else {
            self.undo_annotation(self.current_page);
        }
    }

    fn start_export(&mut self, ctx: &egui::Context) {
        if self.is_exporting {
            return;
        }

        let source_stamp = match notes::pdf_stamp(&self.pdf_path) {
            Ok(stamp) => stamp,
            Err(err) => {
                self.audio_toast(format!("Could not read the PDF for export: {err}"));
                return;
            }
        };
        self.finish_sketch_edit();
        self.sync_and_save();
        self.is_exporting = true;
        self.toast_message = Some((" Exporting...".to_string(), std::time::Instant::now()));

        let out_path = self.pdf_path.with_file_name(format!(
            "{}{}.pdf",
            self.pdf_path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy(),
            self.app_state.export_settings.content.file_suffix()
        ));

        let source_path = self.pdf_path.clone();
        let observed_digest = notes::observed_pdf_digest(&source_path);
        let notes_snapshot = self.notes.clone();
        let tx = self.export_tx.clone();
        let export_ctx = ctx.clone();
        let annotations = self.annotations.clone();
        let export_cfg = self.app_state.export_settings.clone();
        let hidden_cfg = self.hidden_pages.clone();
        // Sketches go into the export with the notes, unless the feature is turned off.
        let sketches_snapshot = if self.sketch_enabled() {
            self.sketches.clone()
        } else {
            HashMap::new()
        };

        thread::spawn(move || {
            let res = export::publish_export(
                &source_path,
                &out_path,
                &source_stamp,
                observed_digest.as_deref(),
                |staged| {
                    export::export_slides_with_notes(
                        &source_path,
                        staged,
                        &notes_snapshot,
                        &annotations,
                        &sketches_snapshot,
                        &export_cfg,
                        &hidden_cfg,
                    )
                },
            );
            match res {
                Ok(_) => {
                    let _ = tx.send(Ok(out_path));
                }
                Err(e) => {
                    let _ = tx.send(Err(e.to_string()));
                }
            }
            export_ctx.request_repaint();
        });
    }

    fn import_notes_dialog(&mut self, ctx: &egui::Context) {
        self.sync_and_save();
        self.dialogs.open(
            ctx,
            DialogKind::ImportNotes,
            DialogSpec::open_file().filter("Notes (.md, .txt)", &["md", "txt", "markdown"]),
        );
    }

    fn finish_import_notes(&mut self, path: PathBuf) {
        if !self.has_pdf() {
            return;
        }
        self.sync_and_save();

        match notes::import_notes_from_file(&path, self.total_pages) {
            Ok(imported) => {
                let count = imported.len();
                if count == 0 {
                    self.toast_message = Some((
                        "󰅚 No slide headers found".to_string(),
                        std::time::Instant::now(),
                    ));
                    return;
                }

                self.current_note = merge_imported_notes(&mut self.notes, imported, self.note_page);
                self.note_editor_epoch = self.note_editor_epoch.wrapping_add(1);
                self.mark_study_changed();
                self.show_notes = true;
                if let Err(err) = self.sync_local_checkpoint() {
                    self.audio_toast(format!("Notes imported, but local backup failed: {err}"));
                    return;
                }

                self.toast_message = Some((
                    format!("󰄬 Notes imported for {} pages", count),
                    std::time::Instant::now(),
                ));
            }
            Err(e) => {
                self.toast_message =
                    Some((format!("󰅚 Import error: {}", e), std::time::Instant::now()));
            }
        }
    }

    fn export_raw_notes_dialog(&mut self, ctx: &egui::Context) {
        self.sync_and_save();

        if self.notes.is_empty() {
            self.toast_message = Some((
                "󰅚 No notes to export".to_string(),
                std::time::Instant::now(),
            ));
            return;
        }

        let default_name = format!(
            "{}_notes.md",
            self.pdf_path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
        );

        let mut spec = DialogSpec::save_file()
            .filter("Markdown", &["md"])
            .filter("Text", &["txt"])
            .file_name(default_name);
        if let Some(parent) = self.pdf_path.parent() {
            spec = spec.directory(parent);
        }
        self.dialogs.open(ctx, DialogKind::ExportNotes, spec);
    }

    fn finish_export_notes(&mut self, out_path: PathBuf) {
        if !self.has_pdf() {
            return;
        }
        match notes::export_raw_notes_md(&out_path, &self.pdf_name, &self.notes) {
            Ok(_) => {
                let fname = out_path.file_name().unwrap_or_default().to_string_lossy();
                self.toast_message = Some((
                    format!("󰄬 Saved raw notes: {}", fname),
                    std::time::Instant::now(),
                ));
            }
            Err(e) => {
                self.toast_message =
                    Some((format!("󰅚 Export error: {}", e), std::time::Instant::now()));
            }
        }
    }

    fn import_theme_dialog(&mut self, ctx: &egui::Context) {
        self.dialogs.open(
            ctx,
            DialogKind::ImportTheme,
            DialogSpec::open_file().filter("Suite Theme (.json)", &["json"]),
        );
    }

    fn finish_import_theme(&mut self, path: PathBuf, ctx: &egui::Context) {
        match SuiteTheme::load_from_file(&path) {
            Ok(loaded) => {
                loaded.apply(ctx);
                let _ = loaded.save_to_file(&SuiteTheme::default_config_path());
                self.toast_message = Some((
                    format!("󰄬 Loaded theme: {}", loaded.name),
                    std::time::Instant::now(),
                ));
                self.theme = loaded;
            }
            Err(e) => {
                self.toast_message = Some((
                    format!("󰅚 Invalid theme file: {}", e),
                    std::time::Instant::now(),
                ));
            }
        }
    }

    fn export_theme_dialog(&mut self, ctx: &egui::Context) {
        let mut spec = DialogSpec::save_file()
            .filter("Suite Theme (.json)", &["json"])
            .file_name("suite_theme.json");

        if let Some(parent) = SuiteTheme::default_config_path().parent() {
            spec = spec.directory(parent);
        }

        self.dialogs.open(ctx, DialogKind::ExportTheme, spec);
    }

    fn finish_export_theme(&mut self, path: PathBuf) {
        match self.theme.save_to_file(&path) {
            Ok(_) => {
                self.toast_message =
                    Some(("󰄬 Theme exported".to_string(), std::time::Instant::now()));
            }
            Err(e) => {
                self.toast_message = Some((
                    format!("󰅚 Theme export error: {}", e),
                    std::time::Instant::now(),
                ));
            }
        }
    }

    /// Handle the result of a non-blocking dialog.
    fn handle_dialog_result(
        &mut self,
        kind: DialogKind,
        result: Option<Vec<PathBuf>>,
        ctx: &egui::Context,
    ) {
        if matches!(kind, DialogKind::AddDictionary) {
            if let Some(paths) = result {
                let msg = match self.add_dictionary_files(&paths) {
                    Ok(msg) => {
                        self.app_state.save();
                        msg
                    }
                    Err(err) => format!("󰅚 {err}"),
                };
                self.toast_message = Some((msg, std::time::Instant::now()));
            }
            return;
        }
        let result = result.and_then(|paths| paths.into_iter().next());
        match kind {
            DialogKind::PickAsrModelDir { retry } => match result {
                Some(dir) => {
                    if audio::is_asr_model_dir(&dir) {
                        self.asr_model_dir = Some(dir);
                        match retry {
                            AsrRetry::Lecture { wav_path } => {
                                self.start_lecture_transcription(wav_path, ctx)
                            }
                            AsrRetry::Memo {
                                page,
                                memo_idx,
                                wav_path,
                            } => self.start_memo_transcription(page, memo_idx, wav_path, ctx),
                        }
                    } else {
                        self.audio_toast("󰅚 Not a Parakeet model folder");
                    }
                }
                None => {
                    self.audio_toast(format!(
                        "󰅚 Put parakeet-tdt-0.6b-v3-int8 in {}",
                        paths::models_dir().display()
                    ));
                }
            },
            kind => {
                let Some(path) = result else {
                    return; // annullato dall'utente
                };
                match kind {
                    DialogKind::OpenPdf => self.open_pdf_user(path, ctx),
                    DialogKind::NewNotebook => {
                        if self.has_pdf()
                            && doc_lock::canonical(&path) == doc_lock::canonical(&self.pdf_path)
                        {
                            self.toast_message = Some((
                                "Cannot overwrite the open document".to_string(),
                                std::time::Instant::now(),
                            ));
                            return;
                        }
                        match notes::create_blank_study_notebook(&path) {
                            Ok(()) => self.open_pdf_user(path, ctx),
                            Err(e) => {
                                self.toast_message = Some((
                                    format!("󰅚 Notebook creation failed: {}", e),
                                    std::time::Instant::now(),
                                ));
                            }
                        }
                    }
                    DialogKind::ExportOriginalPdf => self.finish_download_original_pdf(path),
                    DialogKind::ExportAnki => self.finish_export_anki(path),
                    DialogKind::PickAsrModelDir { .. } => {}
                    DialogKind::ImportLectureMedia { media_dir } => {
                        self.finish_import_lecture_media(path, &media_dir)
                    }
                    DialogKind::AddDictionary => unreachable!(),
                    DialogKind::ExportTheme => self.finish_export_theme(path),
                    DialogKind::ImportTheme => self.finish_import_theme(path, ctx),
                    DialogKind::ExportNotes => self.finish_export_notes(path),
                    DialogKind::ImportNotes => self.finish_import_notes(path),
                }
            }
        }
    }

    fn page_has_study_content(&self, page: u32) -> bool {
        let has_note = if page == self.note_page {
            !self.current_note.trim().is_empty()
        } else {
            self.notes.get(&page).is_some_and(|n| !n.trim().is_empty())
        };

        let has_anns = self
            .annotations
            .get(&page)
            .map(|a| !a.is_empty())
            .unwrap_or(false);

        let has_transcript = self.audio_index.segments.iter().any(|s| s.page == page)
            || self
                .audio_index
                .other_lectures
                .iter()
                .any(|l| l.segments.iter().any(|s| s.page == page));

        has_note
            || has_anns
            || has_transcript
            || has_extra_study_content(
                self.sketches.get(&page),
                self.audio_index.page_memos.get(&page),
            )
    }

    /// Searches backwards from `page - 1` down to `1` for the most recent non-empty note.
    fn find_previous_note(&self, page: u32) -> Option<(u32, String)> {
        let mut p = page.saturating_sub(1);
        while p >= 1 {
            if let Some(n) = self.notes.get(&p)
                && !n.trim().is_empty()
            {
                return Some((p, n.clone()));
            }
            p -= 1;
        }
        None
    }

    /// Searches forwards from `page + 1` up to `total_pages` for the next non-empty note.
    fn find_next_note(&self, page: u32) -> Option<(u32, String)> {
        for p in (page + 1)..=self.total_pages {
            if let Some(n) = self.notes.get(&p)
                && !n.trim().is_empty()
            {
                return Some((p, n.clone()));
            }
        }
        None
    }
    fn switch_note_page(&mut self, target_note_page: u32) {
        let p = target_note_page.clamp(1, self.total_pages.max(1));
        if p == self.note_page {
            return;
        }
        self.sync_and_save();
        self.note_page = p;
        self.current_note = self.notes.get(&p).cloned().unwrap_or_default();
    }
    fn apply_custom_font(&self, ctx: &egui::Context) {
        let mut fonts = egui::FontDefinitions::default();

        // Always keep JetBrainsMono Nerd Font as base/fallback for Nerd Icons!
        fonts.font_data.insert(
            "JetBrainsMono".to_owned(),
            std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
                "../../assets/JetBrainsMonoNerdFont-Regular.ttf"
            ))),
        );

        let mut primary_loaded = false;
        if let Some(custom_path) = &self.app_state.settings.custom_font_path
            && let Ok(bytes) = std::fs::read(custom_path)
        {
            fonts.font_data.insert(
                "UserCustomFont".to_owned(),
                std::sync::Arc::new(egui::FontData::from_owned(bytes)),
            );
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "UserCustomFont".to_owned());
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .insert(0, "UserCustomFont".to_owned());
            primary_loaded = true;
        }

        // Put JetBrainsMono right after UserCustomFont (or first if no custom font) so icons always render!
        let pos = if primary_loaded { 1 } else { 0 };
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(pos, "JetBrainsMono".to_owned());
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .insert(pos, "JetBrainsMono".to_owned());

        ctx.set_fonts(fonts);
    }
    fn rebuild_spellchecker(&mut self) {
        let mut sc = spellcheck::SpellChecker::with_preferences(
            &self.app_state.settings.extra_dictionaries,
            self.app_state.settings.english_dictionary_enabled,
            &self.app_state.settings.disabled_dictionaries,
        );
        sc.enabled = self.app_state.settings.spellcheck_enabled;
        self.spellchecker = sc;
    }
    /// Import multiple Hunspell files. Incomplete pairs remain installed but inactive,
    /// so the missing partner can be provided in a later selection.
    fn add_dictionary_files(&mut self, paths: &[std::path::PathBuf]) -> Result<String, String> {
        use std::collections::BTreeMap;
        let dir = spellcheck::dict_dir();
        let mut sources: BTreeMap<
            String,
            (Option<std::path::PathBuf>, Option<std::path::PathBuf>),
        > = BTreeMap::new();
        let mut text_files = Vec::new();
        for src in paths {
            let ext = src
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if ext == "txt" {
                text_files.push(src.clone());
                continue;
            }
            if ext != "aff" && ext != "dic" {
                return Err(format!("Unsupported dictionary file: {}", src.display()));
            }
            let stem = src
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| format!("Invalid dictionary name: {}", src.display()))?;
            let pair = sources.entry(stem.to_string()).or_default();
            if ext == "aff" {
                pair.0 = Some(src.clone());
            } else {
                pair.1 = Some(src.clone());
            }
        }
        if sources.is_empty() && text_files.is_empty() {
            return Err("Select a .dic, .aff, or .txt dictionary file".into());
        }
        let mut added = Vec::new();
        let mut warnings = Vec::new();
        for src in text_files {
            let filename = src.file_name().ok_or("Invalid filename")?;
            let dest = dir.join(filename);
            if !dest.exists() {
                std::fs::copy(&src, &dest).map_err(|e| format!("Copy failed: {e}"))?;
            } else if src != dest && std::fs::read(&src).ok() != std::fs::read(&dest).ok() {
                return Err(format!(
                    "{} already exists with different contents",
                    dest.display()
                ));
            }
            if !self.app_state.settings.extra_dictionaries.contains(&dest) {
                self.spellchecker.load_dictionary_file(&dest)?;
                self.app_state.settings.extra_dictionaries.push(dest);
                added.push(filename.to_string_lossy().to_string());
            }
        }
        for (stem, (mut aff, mut dic)) in sources {
            // The picker may include just one file when its partner is next to it.
            if aff.is_none() {
                if let Some(path) = &dic {
                    let sibling = path.with_extension("aff");
                    if sibling.is_file() {
                        aff = Some(sibling);
                    }
                }
            }
            if dic.is_none() {
                if let Some(path) = &aff {
                    let sibling = path.with_extension("dic");
                    if sibling.is_file() {
                        dic = Some(sibling);
                    }
                }
            }
            let dest_aff = dir.join(format!("{stem}.aff"));
            let dest_dic = dir.join(format!("{stem}.dic"));
            let mut copied = Vec::new();
            for (src, dest) in [(aff, &dest_aff), (dic, &dest_dic)] {
                if let Some(src) = src {
                    if dest.exists() {
                        if src.as_path() != dest.as_path()
                            && std::fs::read(&src).ok() != std::fs::read(dest).ok()
                        {
                            warnings.push(format!(
                                "{stem}: {} already exists with different contents",
                                dest.display()
                            ));
                            continue;
                        }
                    } else {
                        if let Err(err) = std::fs::copy(&src, dest) {
                            for file in &copied {
                                let _ = std::fs::remove_file(file);
                            }
                            return Err(format!("Could not import {stem}: {err}"));
                        }
                        copied.push(dest.to_path_buf());
                    }
                }
            }
            if !self
                .app_state
                .settings
                .extra_dictionaries
                .contains(&dest_aff)
            {
                self.app_state
                    .settings
                    .extra_dictionaries
                    .push(dest_aff.clone());
            }
            if dest_aff.is_file() && dest_dic.is_file() {
                // Validate before enabling; retain already installed halves on failure.
                match spellcheck::SpellChecker::new(&[dest_aff.clone()])
                    .loaded_languages
                    .iter()
                    .any(|name| name == &stem)
                {
                    true => {
                        added.push(stem);
                    }
                    false => warnings.push(format!("{stem}: dictionary pair could not be loaded")),
                }
            } else {
                let missing = if !dest_aff.is_file() { ".aff" } else { ".dic" };
                warnings.push(format!(
                    "{stem}: missing {missing} — select it later to finish installation"
                ));
            }
        }
        self.rebuild_spellchecker();
        if warnings.is_empty() {
            Ok(format!("󰄬 Dictionaries installed: {}", added.join(", ")))
        } else {
            Ok(format!("⚠ {}", warnings.join("; ")))
        }
    }

    // Screenshots from the clipboard or files become image annotations or notes.

    /// Return the page under `pos` and its normalized point.
    /// Fall back to the current page center.
    fn page_at_pos(&self, pos: Option<egui::Pos2>) -> (u32, (f32, f32)) {
        if let Some(p) = pos
            && let Some((page, r)) = self.last_slide_rects.iter().find(|(_, r)| r.contains(p))
        {
            let nx = ((p.x - r.left()) / r.width()).clamp(0.0, 1.0);
            let ny = ((p.y - r.top()) / r.height()).clamp(0.0, 1.0);
            return (*page, (nx, ny));
        }
        (self.current_page, (0.5, 0.5))
    }

    /// Call each frame after text-drag and focus updates.
    fn process_screenshots(&mut self, ctx: &egui::Context, any_focused: bool, in_slideshow: bool) {
        // 1. Completed job result.
        if let Some((target, shot)) = self.shots.poll() {
            self.finish_screenshot(target, shot);
        }
        if !self.has_pdf() || in_slideshow {
            return;
        }
        let pointer = ctx.input(|i| i.pointer.hover_pos().or(i.pointer.latest_pos()));

        // 2. Ctrl+V on the PDF.
        let over_canvas = pointer.is_some_and(|p| {
            self.canvas_rect.contains(p) || self.last_slide_rects.iter().any(|(_, r)| r.contains(p))
        });

        // Clear note focus when the PDF canvas is clicked.
        if over_canvas
            && ctx.input(|i| i.pointer.primary_pressed())
            && let Some(focused_id) = ctx.memory(|m| m.focused())
        {
            ctx.memory_mut(|m| m.surrender_focus(focused_id));
        }

        let ctrl_recent = screenshots::ctrl_recently_held(ctx);
        let ctrl_v = ctx.input(|i| {
            let ctrl = i.modifiers.ctrl || i.modifiers.command || ctrl_recent;
            let paste_ev = i.events.iter().any(|e| matches!(e, egui::Event::Paste(_)));
            let v_press = ctrl && i.key_pressed(egui::Key::V);
            let v_release = i.events.iter().any(|e| {
                matches!(
                    e,
                    egui::Event::Key { key: egui::Key::V, pressed: false, modifiers, .. }
                        if modifiers.ctrl || modifiers.command || ctrl
                )
            });
            paste_ev || v_press || v_release
        });

        let over_pending_primary = self.view_mode == ViewMode::SinglePage
            && pointer.is_some_and(|p| self.canvas_rect.contains(p))
            && self.viewer.page_or_thumbnail(self.current_page).is_none();
        if ctrl_v && !any_focused && over_canvas && !over_pending_primary {
            let (page, center) = self.page_at_pos(pointer);
            self.shots
                .request_clipboard(ctx, ShotTarget::Pdf { page, center });
        }

        // 3. Dropped image files go to the PDF page or current note.
        let dropped: Option<PathBuf> = ctx.input(|i: &InputState| {
            i.raw
                .dropped_files
                .iter()
                .find(|f| screenshots::is_image_path(f.path())) // Call path() to get &Path
                .map(|f| f.path().to_path_buf()) // Convert &Path to owned PathBuf
        });
        if let Some(path) = dropped {
            if over_pending_primary {
                self.toast_message = Some((
                    "Wait for the current page before dropping an image".into(),
                    std::time::Instant::now(),
                ));
                return;
            }
            let over_page =
                pointer.is_some_and(|p| self.last_slide_rects.iter().any(|(_, r)| r.contains(p)));
            let target = if over_page {
                let (page, center) = self.page_at_pos(pointer);
                ShotTarget::Pdf { page, center }
            } else {
                ShotTarget::Note {
                    page: self.note_page,
                    cursor: self.current_note.chars().count(),
                }
            };
            self.shots.request_file(ctx, target, path);
        }
    }

    fn finish_screenshot(&mut self, target: ShotTarget, shot: Option<screenshots::Shot>) {
        // Ignore clipboard values that are not images.
        let Some(shot) = shot.filter(|s| !s.is_empty()) else {
            return;
        };
        match target {
            ShotTarget::Pdf { page, center } => self.add_screenshot_annotation(page, center, shot),
            ShotTarget::Note { page, cursor } => self.insert_screenshot_in_note(page, cursor, shot),
        }
    }

    /// Create an image annotation using normalized top-left and bottom-right points.
    /// `text` stores the PNG filename in `<media dir>/screenshots/`.
    fn add_screenshot_annotation(
        &mut self,
        page: u32,
        center: (f32, f32),
        shot: screenshots::Shot,
    ) {
        if !self.has_pdf() {
            return;
        }
        let dir = audio::AudioDocIndex::media_dir_for_pdf(&self.pdf_path).join("screenshots");
        let shot = shot.downscaled(1600);
        let (iw, ih) = (shot.width as f32, shot.height as f32);
        let png = match screenshots::save_png(&shot, &dir) {
            Ok(p) => p,
            Err(e) => {
                self.toast_message =
                    Some((format!("Screenshot failed: {e}"), std::time::Instant::now()));
                return;
            }
        };
        let Some(name) = png.file_name().map(|n| n.to_string_lossy().to_string()) else {
            return;
        };

        // Use 40% of page width and preserve the image aspect ratio.
        let page_aspect = self
            .last_slide_rects
            .iter()
            .find(|(p, _)| *p == page)
            .map(|(_, r)| r.width() / r.height())
            .or_else(|| (self.page_pt.1 > 0.0).then(|| self.page_pt.0 / self.page_pt.1))
            .unwrap_or(4.0 / 3.0);
        let mut nw = 0.4_f32;
        let mut nh = nw * page_aspect * ih / iw;
        if nh > 0.9 {
            nw *= 0.9 / nh;
            nh = 0.9;
        }
        let x0 = (center.0 - nw / 2.0).clamp(0.0, 1.0 - nw);
        let y0 = (center.1 - nh / 2.0).clamp(0.0, 1.0 - nh);

        self.push_undo_snapshot(page);
        let list = self.annotations.entry(page).or_default();
        list.push(annotations::PageAnnotation {
            kind: annotations::AnnotationKind::Image,
            color: [255, 255, 255],
            points: vec![(x0, y0), (x0 + nw, y0 + nh)],
            text: name,
            font_size: self.textbox_font_size,
            bold: false,
            italic: false,
            stroke_width: self.stroke_width,
            native_source: None,
        });
        self.selected_image = Some((page, list.len() - 1));
        self.save_study_edit();
        self.toast_message = Some((
            format!("Screenshot added to page {page}"),
            std::time::Instant::now(),
        ));
    }

    fn insert_screenshot_in_note(&mut self, page: u32, cursor: usize, shot: screenshots::Shot) {
        if !self.has_pdf() {
            return;
        }
        let dir = audio::AudioDocIndex::media_dir_for_pdf(&self.pdf_path).join("screenshots");
        let msg = match screenshots::save_png(&shot.downscaled(1600), &dir) {
            Ok(png) => {
                let md = format!("\n{}\n", screenshots::markdown_image(&png));
                if page == self.note_page {
                    let at = self
                        .current_note
                        .char_indices()
                        .nth(cursor)
                        .map(|(i, _)| i)
                        .unwrap_or(self.current_note.len());
                    self.current_note.insert_str(at, &md);
                } else {
                    self.notes.entry(page).or_default().push_str(&md);
                    self.mark_study_changed();
                    self.sync_and_save();
                }
                "Screenshot added to the note".to_string()
            }
            Err(e) => format!("Screenshot failed: {e}"),
        };
        self.toast_message = Some((msg, std::time::Instant::now()));
    }
}

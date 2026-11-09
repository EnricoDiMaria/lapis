//! Per-page scratchpad ("Sketch") shown in the notes panel next to Edit and Preview.
//!
//! It is just a background plus strokes on an unbounded canvas that fills the whole panel (below
//! the toolbar). Coordinates are in sketch units, measured from the top-left corner, where one
//! unit is one pixel at 100 % zoom. The drawing therefore does not depend on the size or the
//! position of the panel: a wide bottom panel simply shows more of the same canvas than a narrow
//! side panel. The scrollable area grows as you draw, so there is always room to continue.

use crate::notes::{self, AnnotationKind, BlankPageStyle, PageAnnotation, SketchPage};
use egui::{
    Color32, CornerRadius, FontId, PointerButton, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2, vec2,
};
use std::collections::HashMap;

/// Mouse wheel / trackpad scrolling speed of the canvas (the egui default is 1.0).
const WHEEL_SPEED: f32 = 3.0;
const PAN_SPEED: f32 = 3.0;
/// While drawing, the canvas scrolls by itself when the pointer is this close to an edge...
const AUTOSCROLL_EDGE_PX: f32 = 40.0;
/// ...at up to this many pixels per frame.
const AUTOSCROLL_MAX_PX: f32 = 20.0;
const MIN_ZOOM: f32 = 0.25;
const MAX_ZOOM: f32 = 4.0;
const UNDO_LIMIT: usize = 40;
/// Width of a new text box, in sketch units.
const TEXT_BOX_W: f32 = 240.0;
/// Free space kept below and to the right of the drawing, in sketch units.
const GROW: f32 = 400.0;
/// The toolbar puts colors and sizes in a popup below this width.
const COMPACT_TOOLBAR_W: f32 = 150.0;
const ERASER_RADIUS_PX: f32 = 7.0;

/// Colors readable on the (always light) paper.
const PALETTE: [[u8; 3]; 6] = [
    [17, 24, 39],
    [220, 38, 38],
    [37, 99, 235],
    [22, 163, 74],
    [234, 88, 12],
    [147, 51, 234],
];
/// Stroke widths in sketch units.
const STROKE_PRESETS: [f32; 4] = [1.5, 3.0, 5.5, 10.0];
/// Text sizes in sketch units, matching the stroke presets.
const FONT_PRESETS: [f32; 4] = [12.0, 16.0, 22.0, 32.0];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SketchTool {
    /// Moves the canvas instead of drawing.
    Hand,
    Pen,
    Eraser,
    Line,
    Arrow,
    Rect,
    Circle,
    Text,
}

impl SketchTool {
    const ALL: [(SketchTool, &'static str, &'static str); 8] = [
        (SketchTool::Hand, "\u{F0E47}", "Move the canvas (hand)"),
        (SketchTool::Pen, "󱦹", "Pen"),
        (SketchTool::Eraser, "󰇾", "Eraser"),
        (SketchTool::Line, "󰕞", "Line"),
        (SketchTool::Arrow, "󰁔", "Arrow"),
        (SketchTool::Rect, "󰝣", "Rectangle"),
        (SketchTool::Circle, "󰝥", "Circle"),
        (SketchTool::Text, "󰊄", "Text"),
    ];

    fn shape_kind(self) -> Option<AnnotationKind> {
        match self {
            SketchTool::Line => Some(AnnotationKind::Line),
            SketchTool::Arrow => Some(AnnotationKind::Arrow),
            SketchTool::Rect => Some(AnnotationKind::Rect),
            SketchTool::Circle => Some(AnnotationKind::Circle),
            _ => None,
        }
    }
}

/// A text box being edited in place.
struct TextEditState {
    page: u32,
    index: usize,
    buf: String,
    original: String,
    /// Strokes before the edit started (what Undo restores).
    before: Vec<PageAnnotation>,
    focus_requested: bool,
}

#[derive(Clone, Copy)]
enum ZoomRequest {
    /// Multiply the zoom (toolbar + and -).
    Step(f32),
    /// Back to 100 %.
    Reset,
}

pub struct SketchState {
    pub tool: SketchTool,
    pub color: [u8; 3],
    pub size_idx: usize,
    /// 1.0 = 100 %.
    pub zoom: f32,
    /// Background used when a page has no sketch yet (remembers the last choice).
    pub new_paper: BlankPageStyle,
    /// The pointer is over the canvas (used to route Ctrl + wheel to the canvas zoom).
    pub hovered: bool,
    /// The sketch is the last thing that was drawn on (Ctrl+Z then undoes the sketch).
    pub focus: bool,
    active: Option<PageAnnotation>,
    undo: HashMap<u32, Vec<Vec<PageAnnotation>>>,
    redo: HashMap<u32, Vec<Vec<PageAnnotation>>>,
    erase_snapshot_taken: bool,
    edit: Option<TextEditState>,
    zoom_request: Option<ZoomRequest>,
    /// Scroll offset to apply on the next frame (keeps the point under the cursor on zoom).
    pending_scroll: Option<Vec2>,
    /// Where the canvas is scrolled to (from the last frame).
    pub scroll_offset: Vec2,
}

impl Default for SketchState {
    fn default() -> Self {
        Self {
            tool: SketchTool::Pen,
            color: PALETTE[0],
            size_idx: 1,
            zoom: 1.0,
            new_paper: BlankPageStyle::Blank,
            hovered: false,
            focus: false,
            active: None,
            undo: HashMap::new(),
            redo: HashMap::new(),
            erase_snapshot_taken: false,
            edit: None,
            zoom_request: None,
            pending_scroll: None,
            scroll_offset: Vec2::ZERO,
        }
    }
}

/// What the toolbar asks for; applied after the toolbar is drawn.
enum Action {
    Undo,
    Redo,
    Clear,
    SetPaper(BlankPageStyle),
    Zoom(ZoomRequest),
}

impl SketchState {
    /// Forget undo history and any in-progress input (document or page structure changed).
    pub fn reset(&mut self) {
        self.active = None;
        self.edit = None;
        self.undo.clear();
        self.redo.clear();
        self.erase_snapshot_taken = false;
        self.hovered = false;
        self.focus = false;
        self.zoom_request = None;
        self.pending_scroll = None;
        self.scroll_offset = Vec2::ZERO;
    }

    fn can_undo(&self, page: u32) -> bool {
        self.undo.get(&page).is_some_and(|s| !s.is_empty())
    }

    fn can_redo(&self, page: u32) -> bool {
        self.redo.get(&page).is_some_and(|s| !s.is_empty())
    }

    fn push_undo(&mut self, page: u32, before: Vec<PageAnnotation>) {
        let stack = self.undo.entry(page).or_default();
        if stack.len() >= UNDO_LIMIT {
            stack.remove(0);
        }
        stack.push(before);
        self.redo.remove(&page);
    }

    fn sheet_mut<'a>(
        &self,
        sketches: &'a mut HashMap<u32, SketchPage>,
        page: u32,
    ) -> &'a mut SketchPage {
        sketches
            .entry(page)
            .or_insert_with(|| SketchPage::new(self.new_paper))
    }

    /// Undo the last change on `page`. Returns true if something changed.
    pub fn undo(&mut self, page: u32, sketches: &mut HashMap<u32, SketchPage>) -> bool {
        self.edit = None;
        self.active = None;
        let Some(prev) = self.undo.get_mut(&page).and_then(|s| s.pop()) else {
            return false;
        };
        let current = sketches
            .get(&page)
            .map(|s| s.strokes.clone())
            .unwrap_or_default();
        self.redo.entry(page).or_default().push(current);
        self.sheet_mut(sketches, page).strokes = prev;
        true
    }

    /// Redo the last undone change on `page`. Returns true if something changed.
    pub fn redo(&mut self, page: u32, sketches: &mut HashMap<u32, SketchPage>) -> bool {
        self.edit = None;
        self.active = None;
        let Some(next) = self.redo.get_mut(&page).and_then(|s| s.pop()) else {
            return false;
        };
        let current = sketches
            .get(&page)
            .map(|s| s.strokes.clone())
            .unwrap_or_default();
        self.undo.entry(page).or_default().push(current);
        self.sheet_mut(sketches, page).strokes = next;
        true
    }

    /// Finish a text edit that is still open (leaving the tab, changing page, ...).
    /// Returns true if the stored sketch changed.
    pub fn commit_pending(&mut self, sketches: &mut HashMap<u32, SketchPage>) -> bool {
        self.active = None;
        let Some(edit) = self.edit.take() else {
            return false;
        };
        let text = edit.buf.trim_end().to_string();
        let Some(sheet) = sketches.get_mut(&edit.page) else {
            return false;
        };
        let valid = sheet
            .strokes
            .get(edit.index)
            .is_some_and(|a| a.kind == AnnotationKind::TextBox);
        if !valid {
            return false;
        }
        if text.trim().is_empty() {
            sheet.strokes.remove(edit.index);
            // A box that never had text leaves no trace; an existing one was deleted.
            if !edit.original.trim().is_empty() {
                self.push_undo(edit.page, edit.before);
            }
            return !edit.original.trim().is_empty();
        }
        if text == edit.original {
            return false;
        }
        sheet.strokes[edit.index].text = text;
        self.push_undo(edit.page, edit.before);
        true
    }

    fn font_size(&self) -> f32 {
        FONT_PRESETS[self.size_idx.min(FONT_PRESETS.len() - 1)]
    }

    fn stroke_width(&self) -> f32 {
        STROKE_PRESETS[self.size_idx.min(STROKE_PRESETS.len() - 1)]
    }
}

/// Draw the sketch tab for `page`. Returns true when the stored data changed (the caller saves).
pub fn show(
    ui: &mut egui::Ui,
    st: &mut SketchState,
    page: u32,
    sketches: &mut HashMap<u32, SketchPage>,
) -> bool {
    let mut changed = false;

    // The note page changed while a text box was open: finish it on its own page first.
    if st.edit.as_ref().is_some_and(|e| e.page != page) {
        changed |= st.commit_pending(sketches);
    }

    let action = egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(4, 2))
        .show(ui, |ui| toolbar(ui, st, page, sketches))
        .inner;

    if let Some(action) = action {
        st.commit_pending(sketches);
        st.focus = true;
        match action {
            Action::Undo => changed |= st.undo(page, sketches),
            Action::Redo => changed |= st.redo(page, sketches),
            Action::Clear => {
                let has = sketches.get(&page).is_some_and(|s| !s.strokes.is_empty());
                if has {
                    let before = sketches[&page].strokes.clone();
                    st.push_undo(page, before);
                    st.sheet_mut(sketches, page).strokes.clear();
                    changed = true;
                }
            }
            Action::SetPaper(p) => {
                st.new_paper = p;
                st.sheet_mut(sketches, page).paper = p;
                changed = true;
            }
            Action::Zoom(req) => st.zoom_request = Some(req),
        }
    }

    // No edge shadows (they are annoying in a narrow panel); a thin scroll bar that stays
    // visible while there is more canvas to see tells where you are, and whether it ends.
    changed |= ui
        .scope(|ui| {
            let scroll = &mut ui.spacing_mut().scroll;
            *scroll = egui::style::ScrollStyle::floating();
            scroll.fade.strength = 0.0;
            scroll.floating_width = 4.0;
            scroll.dormant_background_opacity = 0.0;
            scroll.dormant_handle_opacity = 0.6;
            canvas_area(ui, st, page, sketches)
        })
        .inner;
    changed
}

// ---------------------------------------------------------------------------------------------
// Toolbar
// ---------------------------------------------------------------------------------------------

fn icon_button(
    ui: &mut egui::Ui,
    icon: &str,
    selected: bool,
    enabled: bool,
    tip: &str,
) -> egui::Response {
    let button = egui::Button::new(egui::RichText::new(icon).size(13.0))
        .selected(selected)
        .min_size(vec2(24.0, 22.0));
    ui.add_enabled(enabled, button).on_hover_text(tip)
}

fn color_swatches(ui: &mut egui::Ui, st: &mut SketchState) {
    for col in PALETTE {
        let selected = st.color == col;
        let (rect, resp) = ui.allocate_exact_size(vec2(18.0, 20.0), Sense::click());
        ui.painter().circle_filled(
            rect.center(),
            6.5,
            Color32::from_rgb(col[0], col[1], col[2]),
        );
        if selected || resp.hovered() {
            ui.painter().circle_stroke(
                rect.center(),
                8.5,
                Stroke::new(if selected { 2.0 } else { 1.0 }, ui.visuals().text_color()),
            );
        }
        if resp.clicked() {
            st.color = col;
        }
    }
}

fn size_presets(ui: &mut egui::Ui, st: &mut SketchState) {
    for i in 0..STROKE_PRESETS.len() {
        let selected = st.size_idx == i;
        let (rect, resp) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
        let r = 1.5 + i as f32 * 1.4;
        ui.painter()
            .circle_filled(rect.center(), r, ui.visuals().text_color());
        if selected || resp.hovered() {
            ui.painter().circle_stroke(
                rect.center(),
                9.0,
                Stroke::new(if selected { 1.8 } else { 1.0 }, ui.visuals().text_color()),
            );
        }
        if resp.clicked() {
            st.size_idx = i;
        }
    }
}

fn toolbar(
    ui: &mut egui::Ui,
    st: &mut SketchState,
    page: u32,
    sketches: &HashMap<u32, SketchPage>,
) -> Option<Action> {
    let mut action = None;
    let compact = ui.available_width() < COMPACT_TOOLBAR_W;
    let (paper, has_strokes) = sketches
        .get(&page)
        .map(|s| (s.paper, !s.strokes.is_empty()))
        .unwrap_or((st.new_paper, false));

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(2.0, 3.0);

        for (tool, icon, tip) in SketchTool::ALL {
            if icon_button(ui, icon, st.tool == tool, true, tip).clicked() {
                // Switching tool drops a half-drawn shape; an open text box commits by itself
                // when it loses the focus.
                st.active = None;
                st.tool = tool;
            }
        }

        if compact {
            // Narrow panel: one button that shows the current color and opens colors + sizes.
            let col = Color32::from_rgb(st.color[0], st.color[1], st.color[2]);
            let (rect, resp) = ui.allocate_exact_size(vec2(24.0, 22.0), Sense::click());
            ui.painter().circle_filled(rect.center(), 7.0, col);
            ui.painter().circle_stroke(
                rect.center(),
                9.0,
                Stroke::new(1.0, ui.visuals().weak_text_color()),
            );
            let popup_resp = resp.on_hover_text("Color and size");
            egui::containers::Popup::menu(&popup_resp)
                .close_behavior(egui::containers::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| {
                    ui.horizontal(|ui| color_swatches(ui, st));
                    ui.horizontal(|ui| size_presets(ui, st));
                });
        } else {
            ui.add_space(4.0);
            color_swatches(ui, st);
            ui.add_space(4.0);
            size_presets(ui, st);
            ui.add_space(4.0);
        }

        if icon_button(ui, "󰕌", false, st.can_undo(page), "Undo (Ctrl+Z)").clicked() {
            action = Some(Action::Undo);
        }
        if icon_button(ui, "󰑎", false, st.can_redo(page), "Redo").clicked() {
            action = Some(Action::Redo);
        }

        // Stays open while adjusting (paper, zoom); closes on a click outside.
        let menu_config = egui::containers::menu::MenuConfig::new()
            .close_behavior(egui::containers::PopupCloseBehavior::CloseOnClickOutside);
        egui::containers::menu::MenuButton::new(egui::RichText::new("󰇘").size(13.0))
            .config(menu_config)
            .ui(ui, |ui| {
                ui.label(egui::RichText::new("Background").weak());
                for (style, label) in [
                    (BlankPageStyle::Blank, "Blank"),
                    (BlankPageStyle::Ruled, "Ruled"),
                    (BlankPageStyle::Grid, "Grid"),
                    (BlankPageStyle::Dotted, "Dotted"),
                ] {
                    if ui.selectable_label(paper == style, label).clicked() {
                        action = Some(Action::SetPaper(style));
                    }
                }

                ui.separator();
                ui.label(egui::RichText::new("Zoom").weak());
                ui.horizontal(|ui| {
                    if ui.small_button("−").clicked() {
                        action = Some(Action::Zoom(ZoomRequest::Step(1.0 / 1.2)));
                    }
                    if ui
                        .small_button(format!("{:.0}%", st.zoom * 100.0))
                        .on_hover_text("Back to 100 %")
                        .clicked()
                    {
                        action = Some(Action::Zoom(ZoomRequest::Reset));
                    }
                    if ui.small_button("+").clicked() {
                        action = Some(Action::Zoom(ZoomRequest::Step(1.2)));
                    }
                });

                ui.separator();
                if ui
                    .add_enabled(has_strokes, egui::Button::new("󰆴  Clear sketch"))
                    .clicked()
                {
                    action = Some(Action::Clear);
                    ui.close();
                }
            });
    });
    action
}

// ---------------------------------------------------------------------------------------------
// Canvas
// ---------------------------------------------------------------------------------------------

/// Screen position of a sketch point.
fn to_screen(origin: Pos2, zoom: f32, (x, y): (f32, f32)) -> Pos2 {
    Pos2::new(origin.x + x * zoom, origin.y + y * zoom)
}

/// Sketch point under a screen position (the canvas starts at the top-left corner).
fn to_unit(origin: Pos2, zoom: f32, p: Pos2) -> (f32, f32) {
    (
        ((p.x - origin.x) / zoom).max(0.0),
        ((p.y - origin.y) / zoom).max(0.0),
    )
}

fn ann_color(ann: &PageAnnotation) -> Color32 {
    Color32::from_rgb(ann.color[0], ann.color[1], ann.color[2])
}

fn text_font(ann: &PageAnnotation, zoom: f32) -> FontId {
    FontId::proportional((ann.font_size * zoom).max(6.0))
}

fn text_wrap_width(ann: &PageAnnotation, zoom: f32) -> f32 {
    let x1 = ann.points.first().map_or(0.0, |p| p.0);
    let x2 = ann.points.get(1).map_or(x1 + TEXT_BOX_W, |p| p.0);
    ((x2 - x1) * zoom).max(24.0)
}

/// Bottom-right extent of the drawing, in sketch units.
fn extent_of(strokes: &[PageAnnotation]) -> Vec2 {
    let mut ext = Vec2::ZERO;
    for ann in strokes {
        for &(x, y) in &ann.points {
            ext.x = ext.x.max(x);
            ext.y = ext.y.max(y);
        }
        if ann.kind == AnnotationKind::TextBox {
            let lines = ann.text.lines().count().max(1) as f32;
            if let Some(&(x, y)) = ann.points.first() {
                ext.y = ext.y.max(y + lines * ann.font_size * 1.5);
                ext.x = ext.x.max(x + TEXT_BOX_W);
            }
        }
    }
    ext
}

fn paint_paper(
    painter: &egui::Painter,
    visible: Rect,
    origin: Pos2,
    zoom: f32,
    style: BlankPageStyle,
) {
    painter.rect_filled(
        visible,
        CornerRadius::ZERO,
        Color32::from_rgb(253, 253, 251),
    );

    let step = notes::SKETCH_GRID_STEP * zoom;
    if step < 7.0 || style == BlankPageStyle::Blank {
        return; // too dense to be useful
    }
    let line_w = (0.7 * zoom).clamp(0.5, 1.5);
    let first = |from: f32, o: f32| ((from - o) / step).floor().max(0.0) as i32;
    let rows =
        first(visible.top(), origin.y)..=((visible.bottom() - origin.y) / step).ceil() as i32;
    let cols =
        first(visible.left(), origin.x)..=((visible.right() - origin.x) / step).ceil() as i32;

    match style {
        BlankPageStyle::Blank => {}
        BlankPageStyle::Ruled => {
            let stroke = Stroke::new(line_w, Color32::from_rgb(214, 224, 238));
            for r in rows.filter(|&r| r >= 1) {
                painter.hline(visible.x_range(), origin.y + r as f32 * step, stroke);
            }
        }
        BlankPageStyle::Grid => {
            let stroke = Stroke::new(line_w, Color32::from_rgb(226, 230, 238));
            for r in rows.filter(|&r| r >= 1) {
                painter.hline(visible.x_range(), origin.y + r as f32 * step, stroke);
            }
            for c in cols.filter(|&c| c >= 1) {
                painter.vline(origin.x + c as f32 * step, visible.y_range(), stroke);
            }
        }
        BlankPageStyle::Dotted => {
            let radius = (1.1 * zoom).clamp(0.6, 2.5);
            let col = Color32::from_rgb(176, 184, 197);
            for r in rows.filter(|&r| r >= 1) {
                for c in cols.clone().filter(|&c| c >= 1) {
                    painter.circle_filled(
                        Pos2::new(origin.x + c as f32 * step, origin.y + r as f32 * step),
                        radius,
                        col,
                    );
                }
            }
        }
    }
}

fn paint_stroke(painter: &egui::Painter, origin: Pos2, zoom: f32, ann: &PageAnnotation) {
    let col = ann_color(ann);
    let width = (ann.stroke_width * zoom).max(0.75);
    let stroke = Stroke::new(width, col);
    let corners = |ann: &PageAnnotation| -> Option<(Pos2, Pos2)> {
        (ann.points.len() >= 2).then(|| {
            (
                to_screen(origin, zoom, ann.points[0]),
                to_screen(origin, zoom, *ann.points.last().unwrap()),
            )
        })
    };

    match ann.kind {
        AnnotationKind::Freehand => {
            if ann.points.is_empty() {
                return;
            }
            let pts: Vec<Pos2> = ann
                .points
                .iter()
                .map(|&p| to_screen(origin, zoom, p))
                .collect();
            let first = pts[0];
            if pts.iter().all(|p| p.distance(first) < 0.5) {
                painter.circle_filled(first, width * 0.5, col);
            } else {
                painter.add(egui::Shape::line(pts, stroke));
            }
        }
        AnnotationKind::Line => {
            if let Some((a, b)) = corners(ann) {
                painter.line_segment([a, b], stroke);
            }
        }
        AnnotationKind::Arrow => {
            if let Some((a, b)) = corners(ann) {
                painter.line_segment([a, b], stroke);
                let dir = b - a;
                let len = dir.length();
                if len > 4.0 {
                    let u = dir / len;
                    let perp = vec2(-u.y, u.x);
                    let head = (width * 4.2).clamp(8.0 * zoom, 18.0 * zoom.max(1.0));
                    painter.line_segment([b, b - u * head + perp * (head * 0.55)], stroke);
                    painter.line_segment([b, b - u * head - perp * (head * 0.55)], stroke);
                }
            }
        }
        AnnotationKind::Rect => {
            if let Some((a, b)) = corners(ann) {
                painter.rect_stroke(
                    Rect::from_two_pos(a, b),
                    CornerRadius::same(2),
                    stroke,
                    StrokeKind::Middle,
                );
            }
        }
        AnnotationKind::Circle => {
            if let Some((a, b)) = corners(ann) {
                let r = Rect::from_two_pos(a, b);
                painter.circle_stroke(r.center(), r.width().min(r.height()) * 0.5, stroke);
            }
        }
        AnnotationKind::TextBox => {
            if ann.text.is_empty() {
                return;
            }
            let Some(&top_left) = ann.points.first() else {
                return;
            };
            let galley = painter.layout(
                ann.text.clone(),
                text_font(ann, zoom),
                col,
                text_wrap_width(ann, zoom),
            );
            painter.galley(to_screen(origin, zoom, top_left), galley, col);
        }
        // Not available in the sketch.
        AnnotationKind::Highlight
        | AnnotationKind::Underline
        | AnnotationKind::Tape
        | AnnotationKind::Image => {}
    }
}

fn dist_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_sq();
    if len2 < 1e-6 {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Whether the eraser at `p` (screen space) touches `ann`.
fn eraser_hits(
    painter: &egui::Painter,
    origin: Pos2,
    zoom: f32,
    ann: &PageAnnotation,
    p: Pos2,
    tol: f32,
) -> bool {
    let two = |ann: &PageAnnotation| -> Option<(Pos2, Pos2)> {
        (ann.points.len() >= 2).then(|| {
            (
                to_screen(origin, zoom, ann.points[0]),
                to_screen(origin, zoom, *ann.points.last().unwrap()),
            )
        })
    };
    match ann.kind {
        AnnotationKind::Freehand => {
            let pts: Vec<Pos2> = ann
                .points
                .iter()
                .map(|&q| to_screen(origin, zoom, q))
                .collect();
            match pts.len() {
                0 => false,
                1 => pts[0].distance(p) <= tol,
                _ => pts
                    .windows(2)
                    .any(|w| dist_to_segment(p, w[0], w[1]) <= tol),
            }
        }
        AnnotationKind::Line | AnnotationKind::Arrow => {
            two(ann).is_some_and(|(a, b)| dist_to_segment(p, a, b) <= tol)
        }
        AnnotationKind::Rect => two(ann).is_some_and(|(a, b)| {
            let r = Rect::from_two_pos(a, b);
            let (tl, tr, br, bl) = (
                r.left_top(),
                r.right_top(),
                r.right_bottom(),
                r.left_bottom(),
            );
            [(tl, tr), (tr, br), (br, bl), (bl, tl)]
                .iter()
                .any(|&(u, v)| dist_to_segment(p, u, v) <= tol)
        }),
        AnnotationKind::Circle => two(ann).is_some_and(|(a, b)| {
            let r = Rect::from_two_pos(a, b);
            let radius = r.width().min(r.height()) * 0.5;
            (p.distance(r.center()) - radius).abs() <= tol
        }),
        AnnotationKind::TextBox => {
            text_rect(painter, origin, zoom, ann).is_some_and(|r| r.expand(tol).contains(p))
        }
        _ => false,
    }
}

/// Screen rectangle of a text box, from its laid-out size.
fn text_rect(
    painter: &egui::Painter,
    origin: Pos2,
    zoom: f32,
    ann: &PageAnnotation,
) -> Option<Rect> {
    let &top_left = ann.points.first()?;
    let galley = painter.layout(
        if ann.text.is_empty() {
            " ".to_string()
        } else {
            ann.text.clone()
        },
        text_font(ann, zoom),
        Color32::BLACK,
        text_wrap_width(ann, zoom),
    );
    Some(Rect::from_min_size(
        to_screen(origin, zoom, top_left),
        galley.size(),
    ))
}

fn canvas_area(
    ui: &mut egui::Ui,
    st: &mut SketchState,
    page: u32,
    sketches: &mut HashMap<u32, SketchPage>,
) -> bool {
    let paper = sketches.get(&page).map(|s| s.paper).unwrap_or(st.new_paper);
    let extent = sketches
        .get(&page)
        .map(|s| extent_of(&s.strokes))
        .unwrap_or(Vec2::ZERO);
    let view = ui.available_size();

    let mut scroll = egui::ScrollArea::both()
        .id_salt(("sketch_scroll", page))
        .auto_shrink([false, false])
        .wheel_scroll_multiplier(vec2(WHEEL_SPEED, WHEEL_SPEED))
        .scroll_source(
            egui::containers::scroll_area::ScrollSource::MOUSE_WHEEL
                | egui::containers::scroll_area::ScrollSource::SCROLL_BAR,
        );
    if let Some(offset) = st.pending_scroll.take() {
        scroll = scroll.scroll_offset(offset);
    }

    let output = scroll.show(ui, |ui| {
        let mut changed = false;
        let zoom = st.zoom;
        let viewport = ui.clip_rect();

        // The canvas is at least as big as the panel, and keeps free room around the drawing.
        let content = vec2(
            view.x.max((extent.x + GROW) * zoom),
            view.y.max((extent.y + GROW) * zoom),
        );
        let (outer, _) = ui.allocate_exact_size(content, Sense::hover());
        let origin = outer.min;
        let resp = ui.interact(
            outer,
            ui.make_persistent_id(("sketch_canvas", page)),
            Sense::click_and_drag(),
        );
        st.hovered = resp.hovered() || resp.dragged();

        // Zoom: toolbar request, or pinch / Ctrl + wheel while the pointer is over the canvas.
        let pointer = ui.input(|i| i.pointer.hover_pos().or_else(|| i.pointer.interact_pos()));
        let mut factor = 1.0_f32;
        let mut anchor = viewport.center();
        match st.zoom_request.take() {
            Some(ZoomRequest::Step(f)) => factor = f,
            Some(ZoomRequest::Reset) => factor = 1.0 / zoom,
            None => {}
        }
        if resp.hovered() {
            let (z, scroll_y, ctrl) = ui.input(|i| {
                (
                    i.zoom_delta(),
                    i.smooth_scroll_delta.y,
                    i.modifiers.command || i.modifiers.ctrl,
                )
            });
            if (z - 1.0).abs() > 1e-4 {
                factor *= z;
            } else if ctrl && scroll_y.abs() > 0.0 {
                factor *= 1.0 + scroll_y * 0.002;
            }
            if (factor - 1.0).abs() > 1e-4 {
                anchor = pointer.unwrap_or(anchor);
            }
        }
        if (factor - 1.0).abs() > 1e-4 {
            let new_zoom = (zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
            let ratio = new_zoom / zoom;
            if (ratio - 1.0).abs() > 1e-4 {
                // Keep the point under the anchor where it is.
                let offset = viewport.min - origin;
                let anchor_in_view = anchor - viewport.min;
                let next = (offset + anchor_in_view) * ratio - anchor_in_view;
                st.zoom = new_zoom;
                st.pending_scroll = Some(vec2(next.x.max(0.0), next.y.max(0.0)));
                ui.ctx().request_repaint();
            }
        }

        // Painting.
        let painter = ui.painter_at(viewport.intersect(outer));
        paint_paper(&painter, outer.intersect(viewport), origin, zoom, paper);

        let editing_idx = st.edit.as_ref().filter(|e| e.page == page).map(|e| e.index);
        if let Some(sheet) = sketches.get(&page) {
            for (i, ann) in sheet.strokes.iter().enumerate() {
                if Some(i) == editing_idx {
                    continue;
                }
                paint_stroke(&painter, origin, zoom, ann);
            }
        }
        if let Some(active) = &st.active {
            paint_stroke(&painter, origin, zoom, active);
        }

        // In-place text editing.
        let mut commit_edit = false;
        if let Some(edit) = st.edit.as_mut().filter(|e| e.page == page) {
            if let Some(ann) = sketches.get(&page).and_then(|s| s.strokes.get(edit.index)) {
                let font = text_font(ann, zoom);
                let top_left = to_screen(
                    origin,
                    zoom,
                    ann.points.first().copied().unwrap_or((0.0, 0.0)),
                );
                let width = text_wrap_width(ann, zoom);
                let box_rect = Rect::from_min_size(top_left, vec2(width, font.size * 1.4));
                let id = ui.make_persistent_id(("sketch_text_edit", page, edit.index));
                let te = egui::TextEdit::multiline(&mut edit.buf)
                    .id(id)
                    .font(font)
                    .text_color(ann_color(ann))
                    .frame(egui::Frame::NONE)
                    .margin(egui::Margin::ZERO)
                    .desired_width(width)
                    .desired_rows(1)
                    .lock_focus(true);
                let out = ui.put(box_rect, te);
                ui.painter().rect_stroke(
                    out.rect.expand(2.0),
                    CornerRadius::same(2),
                    Stroke::new(1.0, ui.visuals().selection.stroke.color),
                    StrokeKind::Outside,
                );
                if edit.focus_requested {
                    out.request_focus();
                    edit.focus_requested = false;
                } else if out.lost_focus() {
                    commit_edit = true;
                }
            } else {
                commit_edit = true;
            }
        }
        if commit_edit {
            changed |= st.commit_pending(sketches);
        }

        // Cursor and eraser preview.
        if resp.hovered() {
            ui.ctx().set_cursor_icon(match st.tool {
                SketchTool::Hand if resp.dragged() => egui::CursorIcon::Grabbing,
                SketchTool::Hand => egui::CursorIcon::Grab,
                SketchTool::Text => egui::CursorIcon::Text,
                _ => egui::CursorIcon::Crosshair,
            });
            if st.tool == SketchTool::Eraser
                && let Some(p) = pointer
            {
                painter.circle_stroke(
                    p,
                    ERASER_RADIUS_PX,
                    Stroke::new(1.0, Color32::from_black_alpha(140)),
                );
            }
        }

        // The hand tool, or a middle-button drag, moves the canvas.
        let panning = resp.dragged_by(PointerButton::Middle)
            || (st.tool == SketchTool::Hand && resp.dragged_by(PointerButton::Primary));
        if panning {
            ui.scroll_with_delta(resp.drag_delta() * PAN_SPEED);
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }

        changed |= handle_input(ui, st, page, sketches, &resp, origin, zoom, &painter);
        changed
    });
    st.scroll_offset = output.state.clamped_offset();
    output.inner
}

#[allow(clippy::too_many_arguments)]
fn handle_input(
    ui: &egui::Ui,
    st: &mut SketchState,
    page: u32,
    sketches: &mut HashMap<u32, SketchPage>,
    resp: &egui::Response,
    origin: Pos2,
    zoom: f32,
    painter: &egui::Painter,
) -> bool {
    let mut changed = false;
    let primary = PointerButton::Primary;
    let press = ui
        .input(|i| i.pointer.press_origin())
        .or(resp.interact_pointer_pos());
    let pos = resp.interact_pointer_pos().or(resp.hover_pos());

    if (resp.is_pointer_button_down_on() && ui.input(|i| i.pointer.button_down(primary)))
        || resp.clicked_by(primary)
    {
        st.focus = true;
    }

    // Drawing near the edge of the panel scrolls the canvas along, so a stroke can go on.
    if st.active.is_some()
        && resp.dragged_by(primary)
        && let Some(p) = pos
    {
        let view = ui.clip_rect();
        let speed = |dist_to_edge: f32| {
            ((AUTOSCROLL_EDGE_PX - dist_to_edge).clamp(0.0, AUTOSCROLL_EDGE_PX)
                / AUTOSCROLL_EDGE_PX)
                * AUTOSCROLL_MAX_PX
        };
        let delta = vec2(
            speed(p.x - view.left()) - speed(view.right() - p.x),
            speed(p.y - view.top()) - speed(view.bottom() - p.y),
        );
        if delta != Vec2::ZERO {
            ui.scroll_with_delta(delta);
            ui.ctx().request_repaint();
        }
    }

    match st.tool {
        SketchTool::Hand => {} // handled with the middle button, see `canvas_area`
        SketchTool::Pen => {
            if resp.drag_started_by(primary) {
                if let Some(p) = press {
                    let pt = to_unit(origin, zoom, p);
                    st.active = Some(new_annotation(st, AnnotationKind::Freehand, vec![pt]));
                }
            } else if resp.dragged_by(primary)
                && let (Some(p), Some(active)) = (pos, st.active.as_mut())
            {
                let pt = to_unit(origin, zoom, p);
                if active.points.last() != Some(&pt) {
                    active.points.push(pt);
                }
            }
            if resp.drag_stopped_by(primary) {
                if let Some(mut stroke) = st.active.take() {
                    stroke.points = smooth(&stroke.points, zoom);
                    changed |= commit_stroke(st, page, sketches, stroke);
                }
            } else if resp.clicked_by(primary)
                && let Some(p) = pos
            {
                let pt = to_unit(origin, zoom, p);
                let dot = new_annotation(st, AnnotationKind::Freehand, vec![pt, pt]);
                changed |= commit_stroke(st, page, sketches, dot);
            }
        }
        SketchTool::Line | SketchTool::Arrow | SketchTool::Rect | SketchTool::Circle => {
            let kind = st.tool.shape_kind().unwrap_or(AnnotationKind::Line);
            if resp.drag_started_by(primary) {
                if let Some(p) = press {
                    let pt = to_unit(origin, zoom, p);
                    st.active = Some(new_annotation(st, kind, vec![pt, pt]));
                }
            } else if resp.dragged_by(primary)
                && let (Some(p), Some(active)) = (pos, st.active.as_mut())
            {
                let pt = to_unit(origin, zoom, p);
                if let Some(last) = active.points.last_mut() {
                    *last = pt;
                }
            }
            if resp.drag_stopped_by(primary)
                && let Some(shape) = st.active.take()
            {
                let (a, b) = (shape.points[0], *shape.points.last().unwrap());
                let size_px = ((b.0 - a.0) * zoom).hypot((b.1 - a.1) * zoom);
                if size_px >= 3.0 {
                    changed |= commit_stroke(st, page, sketches, shape);
                }
            }
        }
        SketchTool::Eraser => {
            let erasing = resp.drag_started_by(primary)
                || resp.dragged_by(primary)
                || resp.clicked_by(primary);
            if erasing && let Some(p) = pos {
                let hit: Vec<usize> = sketches
                    .get(&page)
                    .map(|s| {
                        s.strokes
                            .iter()
                            .enumerate()
                            .filter(|(_, a)| {
                                eraser_hits(painter, origin, zoom, a, p, ERASER_RADIUS_PX)
                            })
                            .map(|(i, _)| i)
                            .collect()
                    })
                    .unwrap_or_default();
                if !hit.is_empty() {
                    st.commit_pending(sketches);
                    let before = sketches[&page].strokes.clone();
                    if !st.erase_snapshot_taken {
                        st.push_undo(page, before);
                        st.erase_snapshot_taken = true;
                    }
                    let sheet = st.sheet_mut(sketches, page);
                    for i in hit.into_iter().rev() {
                        sheet.strokes.remove(i);
                    }
                    changed = true;
                }
            }
            if !resp.dragged_by(primary) {
                st.erase_snapshot_taken = false;
            }
        }
        SketchTool::Text => {
            if resp.clicked_by(primary)
                && let Some(p) = pos
            {
                changed |= st.commit_pending(sketches);
                let existing = sketches.get(&page).and_then(|s| {
                    s.strokes.iter().position(|a| {
                        a.kind == AnnotationKind::TextBox
                            && text_rect(painter, origin, zoom, a)
                                .is_some_and(|r| r.expand(3.0).contains(p))
                    })
                });
                let before = sketches
                    .get(&page)
                    .map(|s| s.strokes.clone())
                    .unwrap_or_default();
                match existing {
                    Some(index) => {
                        let text = sketches[&page].strokes[index].text.clone();
                        st.edit = Some(TextEditState {
                            page,
                            index,
                            buf: text.clone(),
                            original: text,
                            before,
                            focus_requested: true,
                        });
                    }
                    None => {
                        let (x, y) = to_unit(origin, zoom, p);
                        let mut ann = new_annotation(
                            st,
                            AnnotationKind::TextBox,
                            vec![(x, y), (x + TEXT_BOX_W, y)],
                        );
                        ann.font_size = st.font_size();
                        let sheet = st.sheet_mut(sketches, page);
                        sheet.strokes.push(ann);
                        let index = sheet.strokes.len() - 1;
                        st.edit = Some(TextEditState {
                            page,
                            index,
                            buf: String::new(),
                            original: String::new(),
                            before,
                            focus_requested: true,
                        });
                    }
                }
            }
        }
    }
    changed
}

/// Smooths a freehand stroke with the same filter as the slide annotations. That filter works on
/// normalized coordinates, so the points are scaled to make its tolerance about one screen pixel.
fn smooth(points: &[(f32, f32)], zoom: f32) -> Vec<(f32, f32)> {
    let k = (zoom / 450.0).max(1e-4);
    let scaled: Vec<(f32, f32)> = points.iter().map(|&(x, y)| (x * k, y * k)).collect();
    notes::smooth_freehand_points(&scaled)
        .into_iter()
        .map(|(x, y)| (x / k, y / k))
        .collect()
}

fn new_annotation(
    st: &SketchState,
    kind: AnnotationKind,
    points: Vec<(f32, f32)>,
) -> PageAnnotation {
    PageAnnotation {
        kind,
        color: st.color,
        points,
        text: String::new(),
        font_size: st.font_size(),
        bold: false,
        italic: false,
        stroke_width: st.stroke_width(),
    }
}

fn commit_stroke(
    st: &mut SketchState,
    page: u32,
    sketches: &mut HashMap<u32, SketchPage>,
    stroke: PageAnnotation,
) -> bool {
    let before = sketches
        .get(&page)
        .map(|s| s.strokes.clone())
        .unwrap_or_default();
    st.push_undo(page, before);
    st.sheet_mut(sketches, page).strokes.push(stroke);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Modifiers, PointerButton, RawInput};

    struct Harness {
        ctx: egui::Context,
        st: SketchState,
        sketches: HashMap<u32, SketchPage>,
        size: egui::Vec2,
        time: f64,
        changed: bool,
    }

    impl Harness {
        fn new(width: f32, height: f32) -> Self {
            let mut h = Self {
                ctx: egui::Context::default(),
                st: SketchState::default(),
                sketches: HashMap::new(),
                size: vec2(width, height),
                time: 0.0,
                changed: false,
            };
            // Widgets are hit-tested against the previous frame: let the layout settle first.
            h.frame(vec![]);
            h.frame(vec![]);
            h
        }

        fn frame(&mut self, events: Vec<Event>) {
            let raw = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            self.time += 0.05;
            let (st, sketches, changed) = (&mut self.st, &mut self.sketches, &mut self.changed);
            let mut out = self.ctx.run_ui(raw, |ui| {
                *changed |= show(ui, st, 1, sketches);
            });
            out.textures_delta.clear(); // not rendered in tests
        }

        fn moved(&mut self, x: f32, y: f32) {
            self.frame(vec![Event::PointerMoved(Pos2::new(x, y))]);
        }

        fn button(&mut self, x: f32, y: f32, pressed: bool) {
            self.frame(vec![Event::PointerButton {
                pos: Pos2::new(x, y),
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            }]);
        }

        fn drag(&mut self, from: (f32, f32), to: (f32, f32)) {
            self.moved(from.0, from.1);
            self.button(from.0, from.1, true);
            for i in 1..=4 {
                let t = i as f32 / 4.0;
                self.moved(from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
            }
            self.button(to.0, to.1, false);
            self.frame(vec![]);
        }

        fn click(&mut self, x: f32, y: f32) {
            self.moved(x, y);
            self.button(x, y, true);
            self.button(x, y, false);
            self.frame(vec![]);
        }

        fn strokes(&self) -> &[PageAnnotation] {
            self.sketches.get(&1).map_or(&[][..], |s| &s.strokes)
        }
    }

    #[test]
    fn pen_draws_a_smoothed_stroke_on_the_canvas() {
        let mut h = Harness::new(400.0, 600.0);
        h.drag((150.0, 250.0), (250.0, 350.0));
        assert_eq!(h.strokes().len(), 1, "one stroke committed");
        let s = &h.strokes()[0];
        assert_eq!(s.kind, AnnotationKind::Freehand);
        assert!(s.points.len() >= 2);
        assert!(s.points.iter().all(|&(x, y)| x >= 0.0 && y >= 0.0));
        assert!(h.changed);
    }

    #[test]
    fn a_click_with_the_pen_leaves_a_dot() {
        let mut h = Harness::new(400.0, 600.0);
        h.click(200.0, 300.0);
        assert_eq!(h.strokes().len(), 1);
        assert_eq!(h.strokes()[0].points.len(), 2);
    }

    #[test]
    fn shapes_need_a_minimum_size_and_keep_their_kind() {
        let mut h = Harness::new(400.0, 600.0);
        for (tool, kind) in [
            (SketchTool::Line, AnnotationKind::Line),
            (SketchTool::Arrow, AnnotationKind::Arrow),
            (SketchTool::Rect, AnnotationKind::Rect),
            (SketchTool::Circle, AnnotationKind::Circle),
        ] {
            h.st.tool = tool;
            h.sketches.clear();
            h.drag((120.0, 220.0), (260.0, 360.0));
            assert_eq!(h.strokes().len(), 1, "{tool:?}");
            assert_eq!(h.strokes()[0].kind, kind);
        }
        // A tap does not create a zero-size shape.
        h.sketches.clear();
        h.st.tool = SketchTool::Rect;
        h.click(200.0, 300.0);
        assert!(h.strokes().is_empty());
    }

    #[test]
    fn eraser_removes_the_touched_stroke_and_undo_brings_it_back() {
        let mut h = Harness::new(400.0, 600.0);
        h.st.tool = SketchTool::Line;
        h.drag((100.0, 300.0), (300.0, 300.0));
        h.drag((100.0, 450.0), (300.0, 450.0));
        assert_eq!(h.strokes().len(), 2);

        h.st.tool = SketchTool::Eraser;
        h.click(200.0, 300.0);
        assert_eq!(h.strokes().len(), 1, "only the touched line is erased");
        assert!(h.st.can_undo(1));

        assert!(h.st.undo(1, &mut h.sketches));
        assert_eq!(h.strokes().len(), 2);
        assert!(h.st.redo(1, &mut h.sketches));
        assert_eq!(h.strokes().len(), 1);
    }

    #[test]
    fn dragging_the_eraser_is_a_single_undo_step() {
        let mut h = Harness::new(400.0, 600.0);
        h.st.tool = SketchTool::Line;
        h.drag((100.0, 300.0), (300.0, 300.0));
        h.drag((100.0, 320.0), (300.0, 320.0));
        h.st.tool = SketchTool::Eraser;
        h.drag((200.0, 280.0), (200.0, 340.0));
        assert!(h.strokes().is_empty());
        assert!(h.st.undo(1, &mut h.sketches));
        assert_eq!(
            h.strokes().len(),
            2,
            "one undo restores everything the drag erased"
        );
    }

    #[test]
    fn text_box_is_edited_in_place_and_kept() {
        let mut h = Harness::new(400.0, 600.0);
        h.st.tool = SketchTool::Text;
        h.click(120.0, 200.0);
        assert!(
            h.st.edit.is_some(),
            "clicking with the text tool opens a box"
        );
        h.frame(vec![]);
        h.frame(vec![Event::Text("hello".into())]);
        h.frame(vec![]);
        // Switching tool and finishing the edit stores the text.
        let changed = h.st.commit_pending(&mut h.sketches);
        assert!(changed);
        assert_eq!(h.strokes().len(), 1);
        assert_eq!(h.strokes()[0].kind, AnnotationKind::TextBox);
        assert_eq!(h.strokes()[0].text, "hello");
        assert!(h.st.can_undo(1));
    }

    #[test]
    fn an_empty_text_box_leaves_no_trace() {
        let mut h = Harness::new(400.0, 600.0);
        h.st.tool = SketchTool::Text;
        h.click(120.0, 200.0);
        assert_eq!(h.strokes().len(), 1, "box exists while editing");
        let changed = h.st.commit_pending(&mut h.sketches);
        assert!(!changed);
        assert!(h.strokes().is_empty());
        assert!(!h.st.can_undo(1));
    }

    #[test]
    fn clicking_elsewhere_commits_the_text_and_starts_a_new_box() {
        let mut h = Harness::new(400.0, 600.0);
        h.st.tool = SketchTool::Text;
        h.click(120.0, 200.0);
        h.frame(vec![]);
        h.frame(vec![Event::Text("one".into())]);
        h.frame(vec![]);
        h.click(150.0, 450.0);
        h.frame(vec![]);
        h.st.commit_pending(&mut h.sketches); // closes the second, empty box
        let texts: Vec<&str> = h.strokes().iter().map(|a| a.text.as_str()).collect();
        assert_eq!(texts, vec!["one"]);
    }

    #[test]
    fn works_in_a_very_narrow_and_a_very_wide_panel() {
        for (w, hgt) in [
            (96.0_f32, 400.0_f32),
            (140.0, 300.0),
            (1600.0, 500.0),
            (1800.0, 160.0),
        ] {
            let mut h = Harness::new(w, hgt);
            let cx = w * 0.5;
            // The sheet is centered (or starts at the left edge when it is wider than the panel).
            h.st.tool = SketchTool::Pen;
            h.drag((cx - 20.0, hgt * 0.62), (cx + 20.0, hgt * 0.78));
            h.st.tool = SketchTool::Rect;
            h.drag((cx - 30.0, hgt * 0.6), (cx + 30.0, hgt * 0.85));
            assert_eq!(h.strokes().len(), 2, "drawing works in a {w}x{hgt} panel");
            for s in h.strokes() {
                assert!(s.points.iter().all(|&(x, y)| x >= 0.0 && y >= 0.0));
            }
        }
    }

    #[test]
    fn paper_is_remembered_for_new_sheets() {
        let mut h = Harness::new(400.0, 600.0);
        h.st.new_paper = BlankPageStyle::Grid;
        h.drag((150.0, 250.0), (250.0, 300.0));
        assert_eq!(h.sketches[&1].paper, BlankPageStyle::Grid);
    }

    #[test]
    fn strokes_are_in_sketch_units_and_do_not_depend_on_the_panel_size() {
        let mut spans = Vec::new();
        for w in [160.0_f32, 400.0, 1400.0] {
            let mut h = Harness::new(w, 600.0);
            h.st.tool = SketchTool::Line;
            h.drag((60.0, 200.0), (150.0, 320.0));
            let s = &h.strokes()[0];
            let (a, b) = (s.points[0], *s.points.last().unwrap());
            spans.push((b.0 - a.0, b.1 - a.1));
        }
        for (dx, dy) in spans {
            assert!(
                (dx - 90.0).abs() < 1.0 && (dy - 120.0).abs() < 1.0,
                "{dx} x {dy}"
            );
        }
    }

    #[test]
    fn zoom_changes_the_screen_size_but_not_the_stored_units() {
        let mut h = Harness::new(400.0, 600.0);
        h.st.zoom = 2.0;
        h.st.tool = SketchTool::Line;
        h.drag((60.0, 200.0), (160.0, 300.0));
        let s = &h.strokes()[0];
        let (a, b) = (s.points[0], *s.points.last().unwrap());
        assert!(
            ((b.0 - a.0) - 50.0).abs() < 1.0,
            "100 px at 200 % are 50 units"
        );
    }

    #[test]
    fn zoom_requests_are_clamped_and_keep_the_view_anchored() {
        let mut h = Harness::new(400.0, 600.0);
        for _ in 0..40 {
            h.st.zoom_request = Some(ZoomRequest::Step(1.2));
            h.frame(vec![]);
            h.frame(vec![]);
        }
        assert!((h.st.zoom - MAX_ZOOM).abs() < 1e-3);
        h.st.zoom_request = Some(ZoomRequest::Reset);
        h.frame(vec![]);
        h.frame(vec![]);
        assert!((h.st.zoom - 1.0).abs() < 1e-3);
    }

    #[test]
    fn the_canvas_grows_with_the_drawing() {
        let e = extent_of(&[]);
        assert_eq!(e, Vec2::ZERO);
        let mut h = Harness::new(300.0, 400.0);
        h.st.tool = SketchTool::Line;
        h.drag((100.0, 300.0), (250.0, 380.0));
        let ext = extent_of(h.strokes());
        assert!(ext.x > 200.0 && ext.y > 300.0);
        // Still drawable after the content grew (and scrolled areas exist).
        h.drag((100.0, 100.0), (200.0, 120.0));
        assert_eq!(h.strokes().len(), 2);
    }

    /// A line low on the canvas, so that there is something to scroll to.
    fn make_scrollable(h: &mut Harness) {
        let tool = h.st.tool;
        h.st.tool = SketchTool::Line;
        h.drag((100.0, 560.0), (240.0, 590.0));
        h.st.tool = tool;
        h.frame(vec![]);
        h.frame(vec![]);
    }

    #[test]
    fn the_hand_tool_moves_the_canvas_without_drawing() {
        let mut h = Harness::new(400.0, 600.0);
        make_scrollable(&mut h);
        let before = h.strokes().len();
        let base = h.st.scroll_offset.y;

        h.st.tool = SketchTool::Hand;
        h.drag((200.0, 400.0), (200.0, 300.0));
        h.frame(vec![]);
        h.frame(vec![]);
        assert!(
            h.st.scroll_offset.y > base + 50.0,
            "scrolled from {base} to {:?}",
            h.st.scroll_offset
        );
        assert_eq!(h.strokes().len(), before, "the hand never draws");
    }

    #[test]
    fn the_middle_button_also_moves_the_canvas() {
        let mut h = Harness::new(400.0, 600.0);
        make_scrollable(&mut h);
        let base = h.st.scroll_offset.y;
        h.moved(200.0, 400.0);
        h.frame(vec![Event::PointerButton {
            pos: Pos2::new(200.0, 400.0),
            button: PointerButton::Middle,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        for i in 1..=4 {
            h.moved(200.0, 400.0 - 25.0 * i as f32);
        }
        h.frame(vec![Event::PointerButton {
            pos: Pos2::new(200.0, 300.0),
            button: PointerButton::Middle,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);
        h.frame(vec![]);
        h.frame(vec![]);
        assert!(
            h.st.scroll_offset.y > base + 50.0,
            "scrolled from {base} to {:?}",
            h.st.scroll_offset
        );
        assert_eq!(
            h.strokes().len(),
            1,
            "only the line made by make_scrollable"
        );
    }

    #[test]
    fn drawing_near_the_edge_scrolls_the_canvas_along() {
        let mut h = Harness::new(400.0, 600.0);
        make_scrollable(&mut h);
        let base = h.st.scroll_offset.y;
        h.st.tool = SketchTool::Pen;
        h.moved(200.0, 300.0);
        h.button(200.0, 300.0, true);
        h.moved(200.0, 450.0);
        // Keep the pointer at the bottom edge: the canvas keeps scrolling.
        for _ in 0..12 {
            h.moved(200.0, 596.0);
        }
        let offset = h.st.scroll_offset.y;
        h.button(200.0, 596.0, false);
        assert!(offset > base + 20.0, "scrolled from {base} to {offset}");
        let stroke = h.strokes().last().unwrap();
        assert!(
            stroke.points.last().unwrap().1 > stroke.points[0].1 + 200.0,
            "the stroke follows the scroll"
        );
    }

    #[test]
    fn only_the_primary_button_draws() {
        let mut h = Harness::new(400.0, 600.0);
        h.moved(150.0, 250.0);
        h.frame(vec![Event::PointerButton {
            pos: Pos2::new(150.0, 250.0),
            button: PointerButton::Secondary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        h.moved(220.0, 320.0);
        h.frame(vec![Event::PointerButton {
            pos: Pos2::new(220.0, 320.0),
            button: PointerButton::Secondary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);
        h.frame(vec![]);
        assert!(h.strokes().is_empty());
    }

    #[test]
    fn all_paper_styles_paint() {
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(RawInput::default(), |ui| {
            let painter = ui.painter().clone();
            for style in [
                BlankPageStyle::Blank,
                BlankPageStyle::Ruled,
                BlankPageStyle::Grid,
                BlankPageStyle::Dotted,
            ] {
                let area = Rect::from_min_size(Pos2::new(5.0, 5.0), vec2(300.0, 424.0));
                paint_paper(&painter, area, area.min, 1.0, style);
                paint_paper(&painter, area, area.min, 0.3, style);
            }
        });
        out.textures_delta.clear();
    }
}

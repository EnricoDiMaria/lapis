//! Per-page scratchpad ("Sketch") shown in the notes panel next to Edit and Preview.
//!
//! The sheet is page-like: a fixed aspect ratio, strokes stored in normalized 0..1 coordinates
//! relative to the sheet, and stroke widths / font sizes expressed for a reference sheet width
//! ([`REF_W`]). Everything scales with the sheet, so a drawing looks the same whether the panel
//! is very narrow or very wide. The sheet fits the panel; when the panel is narrower than the
//! minimum readable sheet it scrolls horizontally instead of shrinking further.

use crate::notes::{self, AnnotationKind, BlankPageStyle, PageAnnotation, SketchPage};
use egui::{Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, vec2};
use std::collections::HashMap;

/// Sheet width (px) that the stored stroke widths and font sizes refer to.
const REF_W: f32 = 400.0;
/// Narrowest sheet we draw; a narrower panel scrolls instead of shrinking the sheet further.
const MIN_PAPER_W: f32 = 180.0;
const MARGIN: f32 = 6.0;
const MIN_ZOOM: f32 = 0.4;
const MAX_ZOOM: f32 = 4.0;
const UNDO_LIMIT: usize = 40;
/// Width of a new text box, as a fraction of the sheet width.
const TEXT_BOX_W: f32 = 0.45;
/// The toolbar puts colors and sizes in a popup below this width.
const COMPACT_TOOLBAR_W: f32 = 150.0;

/// Colors readable on the (always light) sheet.
const PALETTE: [[u8; 3]; 6] = [
    [17, 24, 39],
    [220, 38, 38],
    [37, 99, 235],
    [22, 163, 74],
    [234, 88, 12],
    [147, 51, 234],
];
/// Stroke widths for [`REF_W`]-wide sheets.
const STROKE_PRESETS: [f32; 4] = [1.5, 3.0, 5.5, 10.0];
/// Text sizes (same units) matching the stroke presets.
const FONT_PRESETS: [f32; 4] = [12.0, 16.0, 22.0, 32.0];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SketchTool {
    Pen,
    Eraser,
    Line,
    Arrow,
    Rect,
    Circle,
    Text,
}

impl SketchTool {
    const ALL: [(SketchTool, &'static str, &'static str); 7] = [
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

pub struct SketchState {
    pub tool: SketchTool,
    pub color: [u8; 3],
    pub size_idx: usize,
    /// 1.0 = the sheet fits the panel.
    pub zoom: f32,
    /// Fit the sheet to the panel width instead of the whole panel.
    pub fit_width: bool,
    /// Sheet used when a page has no sketch yet (remembers the last choice).
    pub new_paper: BlankPageStyle,
    pub new_aspect: f32,
    /// The pointer is over the sheet (used to route Ctrl + wheel to the sheet zoom).
    pub hovered: bool,
    /// The sketch is the last thing that was drawn on (Ctrl+Z then undoes the sketch).
    pub focus: bool,
    active: Option<PageAnnotation>,
    undo: HashMap<u32, Vec<Vec<PageAnnotation>>>,
    redo: HashMap<u32, Vec<Vec<PageAnnotation>>>,
    erase_snapshot_taken: bool,
    edit: Option<TextEditState>,
}

impl Default for SketchState {
    fn default() -> Self {
        Self {
            tool: SketchTool::Pen,
            color: PALETTE[0],
            size_idx: 1,
            zoom: 1.0,
            fit_width: false,
            new_paper: BlankPageStyle::Blank,
            new_aspect: notes::SKETCH_DEFAULT_ASPECT,
            hovered: false,
            focus: false,
            active: None,
            undo: HashMap::new(),
            redo: HashMap::new(),
            erase_snapshot_taken: false,
            edit: None,
        }
    }
}

/// What the toolbar asks for; applied after the toolbar is drawn.
enum Action {
    Undo,
    Redo,
    Clear,
    SetPaper(BlankPageStyle),
    SetAspect(f32),
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
            .or_insert_with(|| SketchPage::new(self.new_paper, self.new_aspect))
    }

    /// Undo the last change on `page`. Returns true if something changed.
    pub fn undo(&mut self, page: u32, sketches: &mut HashMap<u32, SketchPage>) -> bool {
        self.edit = None;
        self.active = None;
        let Some(prev) = self.undo.get_mut(&page).and_then(|s| s.pop()) else {
            return false;
        };
        let current = sketches.get(&page).map(|s| s.strokes.clone()).unwrap_or_default();
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
        let current = sketches.get(&page).map(|s| s.strokes.clone()).unwrap_or_default();
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

    if let Some(action) = toolbar(ui, st, page, sketches) {
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
            Action::SetAspect(a) => {
                st.new_aspect = a;
                st.sheet_mut(sketches, page).aspect = a;
                changed = true;
            }
        }
    }
    ui.add_space(2.0);

    changed |= paper_area(ui, st, page, sketches);
    changed
}

// ---------------------------------------------------------------------------------------------
// Toolbar
// ---------------------------------------------------------------------------------------------

fn icon_button(ui: &mut egui::Ui, icon: &str, selected: bool, enabled: bool, tip: &str) -> egui::Response {
    let button = egui::Button::new(egui::RichText::new(icon).size(13.0))
        .selected(selected)
        .min_size(vec2(24.0, 22.0));
    ui.add_enabled(enabled, button).on_hover_text(tip)
}

fn color_swatches(ui: &mut egui::Ui, st: &mut SketchState) {
    for col in PALETTE {
        let selected = st.color == col;
        let (rect, resp) = ui.allocate_exact_size(vec2(18.0, 20.0), Sense::click());
        ui.painter()
            .circle_filled(rect.center(), 6.5, Color32::from_rgb(col[0], col[1], col[2]));
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
        ui.painter().circle_filled(rect.center(), r, ui.visuals().text_color());
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
    let (paper, aspect, has_strokes) = sketches
        .get(&page)
        .map(|s| (s.paper, s.aspect, !s.strokes.is_empty()))
        .unwrap_or((st.new_paper, st.new_aspect, false));

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(2.0, 3.0);

        for (tool, icon, tip) in SketchTool::ALL {
            if icon_button(ui, icon, st.tool == tool, true, tip).clicked() {
                st.commit_pending_tool_switch();
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
            ui.label(egui::RichText::new("Paper").weak());
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
            ui.label(egui::RichText::new("Orientation").weak());
            let portrait = aspect >= 1.0;
            let note = if has_strokes { " (empty sheet only)" } else { "" };
            ui.add_enabled_ui(!has_strokes, |ui| {
                if ui
                    .selectable_label(portrait, format!("Portrait{note}"))
                    .clicked()
                    && !portrait
                {
                    action = Some(Action::SetAspect(notes::SKETCH_DEFAULT_ASPECT));
                }
                if ui
                    .selectable_label(!portrait, format!("Landscape{note}"))
                    .clicked()
                    && portrait
                {
                    action = Some(Action::SetAspect(1.0 / notes::SKETCH_DEFAULT_ASPECT));
                }
            });

            ui.separator();
            ui.label(egui::RichText::new("View").weak());
            ui.horizontal(|ui| {
                if ui.small_button("−").clicked() {
                    st.zoom = (st.zoom / 1.2).max(MIN_ZOOM);
                }
                if ui
                    .small_button(format!("{:.0}%", st.zoom * 100.0))
                    .on_hover_text("Back to fit")
                    .clicked()
                {
                    st.zoom = 1.0;
                }
                if ui.small_button("+").clicked() {
                    st.zoom = (st.zoom * 1.2).min(MAX_ZOOM);
                }
            });
            if ui
                .checkbox(&mut st.fit_width, "Fit width")
                .on_hover_text("Fit the sheet to the panel width and scroll vertically")
                .changed()
            {
                st.zoom = 1.0;
            }

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

impl SketchState {
    /// Changing tool closes an open text box so its text is kept.
    fn commit_pending_tool_switch(&mut self) {
        // The text edit loses focus when the toolbar is clicked and commits on its own; this only
        // drops a half-drawn shape.
        self.active = None;
    }
}

// ---------------------------------------------------------------------------------------------
// Sheet
// ---------------------------------------------------------------------------------------------

fn to_screen(rect: Rect, (x, y): (f32, f32)) -> Pos2 {
    Pos2::new(rect.left() + x * rect.width(), rect.top() + y * rect.height())
}

fn to_norm(rect: Rect, p: Pos2) -> (f32, f32) {
    (
        ((p.x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0),
        ((p.y - rect.top()) / rect.height().max(1.0)).clamp(0.0, 1.0),
    )
}

fn ann_color(ann: &PageAnnotation) -> Color32 {
    Color32::from_rgb(ann.color[0], ann.color[1], ann.color[2])
}

fn text_font(ann: &PageAnnotation, scale: f32) -> FontId {
    FontId::proportional((ann.font_size * scale).max(6.0))
}

fn text_wrap_width(ann: &PageAnnotation, rect: Rect) -> f32 {
    let x1 = ann.points.first().map_or(0.0, |p| p.0);
    let x2 = ann.points.get(1).map_or(x1 + TEXT_BOX_W, |p| p.0);
    ((x2 - x1) * rect.width()).max(24.0)
}

fn paint_paper(painter: &egui::Painter, rect: Rect, style: BlankPageStyle) {
    // A soft shadow, then the sheet.
    painter.rect_filled(
        rect.translate(vec2(0.0, 1.5)).expand(0.5),
        CornerRadius::same(3),
        Color32::from_black_alpha(45),
    );
    painter.rect_filled(rect, CornerRadius::same(2), Color32::from_rgb(253, 253, 251));
    painter.rect_stroke(
        rect,
        CornerRadius::same(2),
        Stroke::new(1.0, Color32::from_rgb(205, 209, 216)),
        StrokeKind::Inside,
    );

    let w = rect.width();
    let s = w / REF_W;
    let line_w = (0.7 * s).max(0.5);
    let painter = painter.with_clip_rect(rect);
    match style {
        BlankPageStyle::Blank => {}
        BlankPageStyle::Ruled => {
            let step = w / 21.0;
            let stroke = Stroke::new(line_w, Color32::from_rgb(214, 224, 238));
            let mut y = rect.top() + step * 2.0;
            while y < rect.bottom() - step * 0.5 {
                painter.hline(rect.left() + w * 0.04..=rect.right() - w * 0.04, y, stroke);
                y += step;
            }
        }
        BlankPageStyle::Grid => {
            let step = w / 32.0;
            let stroke = Stroke::new(line_w, Color32::from_rgb(226, 230, 238));
            let mut x = rect.left() + step;
            while x < rect.right() {
                painter.vline(x, rect.top()..=rect.bottom(), stroke);
                x += step;
            }
            let mut y = rect.top() + step;
            while y < rect.bottom() {
                painter.hline(rect.left()..=rect.right(), y, stroke);
                y += step;
            }
        }
        BlankPageStyle::Dotted => {
            let step = w / 32.0;
            let r = (1.1 * s).max(0.6);
            let col = Color32::from_rgb(176, 184, 197);
            let mut y = rect.top() + step;
            while y < rect.bottom() {
                let mut x = rect.left() + step;
                while x < rect.right() {
                    painter.circle_filled(Pos2::new(x, y), r, col);
                    x += step;
                }
                y += step;
            }
        }
    }
}

fn paint_stroke(painter: &egui::Painter, rect: Rect, ann: &PageAnnotation) {
    let s = rect.width() / REF_W;
    let col = ann_color(ann);
    let width = (ann.stroke_width * s).max(0.75);
    let stroke = Stroke::new(width, col);
    let corners = |ann: &PageAnnotation| -> Option<(Pos2, Pos2)> {
        if ann.points.len() >= 2 {
            Some((
                to_screen(rect, ann.points[0]),
                to_screen(rect, *ann.points.last().unwrap()),
            ))
        } else {
            None
        }
    };

    match ann.kind {
        AnnotationKind::Freehand => {
            if ann.points.is_empty() {
                return;
            }
            let pts: Vec<Pos2> = ann.points.iter().map(|&p| to_screen(rect, p)).collect();
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
                    let head = (width * 4.2).clamp(8.0 * s, 18.0 * s.max(1.0));
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
                text_font(ann, s),
                col,
                text_wrap_width(ann, rect),
            );
            painter.galley(to_screen(rect, top_left), galley, col);
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
fn eraser_hits(painter: &egui::Painter, rect: Rect, ann: &PageAnnotation, p: Pos2, tol: f32) -> bool {
    let two = |ann: &PageAnnotation| -> Option<(Pos2, Pos2)> {
        (ann.points.len() >= 2).then(|| {
            (
                to_screen(rect, ann.points[0]),
                to_screen(rect, *ann.points.last().unwrap()),
            )
        })
    };
    match ann.kind {
        AnnotationKind::Freehand => {
            let pts: Vec<Pos2> = ann.points.iter().map(|&q| to_screen(rect, q)).collect();
            match pts.len() {
                0 => false,
                1 => pts[0].distance(p) <= tol,
                _ => pts.windows(2).any(|w| dist_to_segment(p, w[0], w[1]) <= tol),
            }
        }
        AnnotationKind::Line | AnnotationKind::Arrow => {
            two(ann).is_some_and(|(a, b)| dist_to_segment(p, a, b) <= tol)
        }
        AnnotationKind::Rect => two(ann).is_some_and(|(a, b)| {
            let r = Rect::from_two_pos(a, b);
            let (tl, tr, br, bl) = (r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom());
            [(tl, tr), (tr, br), (br, bl), (bl, tl)]
                .iter()
                .any(|&(u, v)| dist_to_segment(p, u, v) <= tol)
        }),
        AnnotationKind::Circle => two(ann).is_some_and(|(a, b)| {
            let r = Rect::from_two_pos(a, b);
            let radius = r.width().min(r.height()) * 0.5;
            (p.distance(r.center()) - radius).abs() <= tol
        }),
        AnnotationKind::TextBox => text_rect(painter, rect, ann).is_some_and(|r| r.expand(tol).contains(p)),
        _ => false,
    }
}

/// Screen rectangle of a text box, from its laid-out size.
fn text_rect(painter: &egui::Painter, rect: Rect, ann: &PageAnnotation) -> Option<Rect> {
    let &top_left = ann.points.first()?;
    let s = rect.width() / REF_W;
    let galley = painter.layout(
        if ann.text.is_empty() { " ".to_string() } else { ann.text.clone() },
        text_font(ann, s),
        Color32::BLACK,
        text_wrap_width(ann, rect),
    );
    let min = to_screen(rect, top_left);
    Some(Rect::from_min_size(min, galley.size()))
}

fn paper_area(
    ui: &mut egui::Ui,
    st: &mut SketchState,
    page: u32,
    sketches: &mut HashMap<u32, SketchPage>,
) -> bool {
    let mut changed = false;
    let (paper, aspect) = sketches
        .get(&page)
        .map(|s| (s.paper, s.aspect))
        .unwrap_or((st.new_paper, st.new_aspect));
    let aspect = aspect.clamp(0.3, 4.0);

    let view = ui.available_size();
    let scroll = egui::ScrollArea::both()
        .id_salt("sketch_scroll")
        .auto_shrink([false, false])
        .scroll_source(egui::containers::scroll_area::ScrollSource::MOUSE_WHEEL | egui::containers::scroll_area::ScrollSource::SCROLL_BAR);

    scroll.show(ui, |ui| {
        // Sheet size: fit the panel (whole page, or just its width), never below the minimum.
        let room_w = (view.x - 2.0 * MARGIN).max(1.0);
        let room_h = (view.y - 2.0 * MARGIN).max(1.0);
        let fit_w = if st.fit_width { room_w } else { room_w.min(room_h / aspect) };
        let paper_w = (fit_w.max(MIN_PAPER_W) * st.zoom).max(60.0);
        let paper_h = paper_w * aspect;

        let content_w = (paper_w + 2.0 * MARGIN).max(view.x);
        let content_h = paper_h + 2.0 * MARGIN;
        let (outer, _) = ui.allocate_exact_size(vec2(content_w, content_h), Sense::hover());
        // Centered horizontally; at fit-to-page it is also centered vertically.
        let off_x = ((content_w - paper_w) * 0.5).max(MARGIN);
        let off_y = ((view.y - paper_h) * 0.5).max(MARGIN);
        let rect = Rect::from_min_size(outer.min + vec2(off_x, off_y.min(content_h - paper_h)), vec2(paper_w, paper_h));
        let scale = paper_w / REF_W;

        let resp = ui.interact(rect, ui.make_persistent_id(("sketch_paper", page)), Sense::click_and_drag());
        st.hovered = resp.hovered() || resp.dragged();

        // Pinch / Ctrl + wheel zoom while the pointer is over the sheet.
        if resp.hovered() {
            let (z, scroll_y, ctrl) = ui.input(|i| {
                (i.zoom_delta(), i.smooth_scroll_delta.y, i.modifiers.command || i.modifiers.ctrl)
            });
            if (z - 1.0).abs() > 1e-4 {
                st.zoom = (st.zoom * z).clamp(MIN_ZOOM, MAX_ZOOM);
            } else if ctrl && scroll_y.abs() > 0.0 {
                st.zoom = (st.zoom * (1.0 + scroll_y * 0.002)).clamp(MIN_ZOOM, MAX_ZOOM);
            }
        }

        let painter = ui.painter_at(outer.intersect(ui.clip_rect()));
        paint_paper(&painter, rect, paper);
        let sheet_painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));

        let editing_idx = st.edit.as_ref().filter(|e| e.page == page).map(|e| e.index);
        if let Some(sheet) = sketches.get(&page) {
            for (i, ann) in sheet.strokes.iter().enumerate() {
                if Some(i) == editing_idx {
                    continue;
                }
                paint_stroke(&sheet_painter, rect, ann);
            }
        }
        if let Some(active) = &st.active {
            paint_stroke(&sheet_painter, rect, active);
        }

        // In-place text editing.
        let mut commit_edit = false;
        if let Some(edit) = st.edit.as_mut().filter(|e| e.page == page) {
            if let Some(ann) = sketches.get(&page).and_then(|s| s.strokes.get(edit.index)) {
                let font = text_font(ann, scale);
                let origin = to_screen(rect, ann.points.first().copied().unwrap_or((0.0, 0.0)));
                let width = text_wrap_width(ann, rect);
                let box_rect = Rect::from_min_size(origin, vec2(width, font.size * 1.4));
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
        let pointer = ui.input(|i| i.pointer.hover_pos().or_else(|| i.pointer.interact_pos()));
        if resp.hovered() {
            match st.tool {
                SketchTool::Text => ui.ctx().set_cursor_icon(egui::CursorIcon::Text),
                _ => ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair),
            }
            if st.tool == SketchTool::Eraser {
                if let Some(p) = pointer {
                    sheet_painter.circle_stroke(
                        p,
                        eraser_tolerance(),
                        Stroke::new(1.0, Color32::from_black_alpha(140)),
                    );
                }
            }
        }

        changed |= handle_input(ui, st, page, sketches, &resp, rect, &sheet_painter);
        changed
    })
    .inner
}

fn eraser_tolerance() -> f32 {
    8.0
}

fn handle_input(
    ui: &egui::Ui,
    st: &mut SketchState,
    page: u32,
    sketches: &mut HashMap<u32, SketchPage>,
    resp: &egui::Response,
    rect: Rect,
    painter: &egui::Painter,
) -> bool {
    let mut changed = false;
    let press = ui
        .input(|i| i.pointer.press_origin())
        .or(resp.interact_pointer_pos());
    let pos = resp.interact_pointer_pos().or(resp.hover_pos());

    if resp.is_pointer_button_down_on() || resp.clicked() {
        st.focus = true;
    }

    match st.tool {
        SketchTool::Pen => {
            if resp.drag_started() {
                if let Some(p) = press {
                    let pt = to_norm(rect, p);
                    st.active = Some(new_annotation(st, AnnotationKind::Freehand, vec![pt]));
                }
            } else if resp.dragged() {
                if let (Some(p), Some(active)) = (pos, st.active.as_mut()) {
                    let pt = to_norm(rect, p);
                    if active.points.last() != Some(&pt) {
                        active.points.push(pt);
                    }
                }
            }
            if resp.drag_stopped() {
                if let Some(mut stroke) = st.active.take() {
                    stroke.points = notes::smooth_freehand_points(&stroke.points);
                    changed |= commit_stroke(st, page, sketches, stroke);
                }
            } else if resp.clicked() {
                if let Some(p) = pos {
                    let pt = to_norm(rect, p);
                    let dot = new_annotation(st, AnnotationKind::Freehand, vec![pt, pt]);
                    changed |= commit_stroke(st, page, sketches, dot);
                }
            }
        }
        SketchTool::Line | SketchTool::Arrow | SketchTool::Rect | SketchTool::Circle => {
            let kind = st.tool.shape_kind().unwrap_or(AnnotationKind::Line);
            if resp.drag_started() {
                if let Some(p) = press {
                    let pt = to_norm(rect, p);
                    st.active = Some(new_annotation(st, kind, vec![pt, pt]));
                }
            } else if resp.dragged() {
                if let (Some(p), Some(active)) = (pos, st.active.as_mut()) {
                    let pt = to_norm(rect, p);
                    if let Some(last) = active.points.last_mut() {
                        *last = pt;
                    }
                }
            }
            if resp.drag_stopped() {
                if let Some(shape) = st.active.take() {
                    let (a, b) = (shape.points[0], *shape.points.last().unwrap());
                    let size = ((b.0 - a.0) * rect.width()).hypot((b.1 - a.1) * rect.height());
                    if size >= 3.0 {
                        changed |= commit_stroke(st, page, sketches, shape);
                    }
                }
            }
        }
        SketchTool::Eraser => {
            if resp.drag_started() || resp.dragged() || resp.clicked() {
                if let Some(p) = pos {
                    let tol = eraser_tolerance();
                    let hit: Vec<usize> = sketches
                        .get(&page)
                        .map(|s| {
                            s.strokes
                                .iter()
                                .enumerate()
                                .filter(|(_, a)| eraser_hits(painter, rect, a, p, tol))
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
            }
            if !resp.dragged() {
                st.erase_snapshot_taken = false;
            }
        }
        SketchTool::Text => {
            if resp.clicked() {
                if let Some(p) = pos {
                    changed |= st.commit_pending(sketches);
                    let existing = sketches.get(&page).and_then(|s| {
                        s.strokes.iter().position(|a| {
                            a.kind == AnnotationKind::TextBox
                                && text_rect(painter, rect, a).is_some_and(|r| r.expand(3.0).contains(p))
                        })
                    });
                    let before = sketches.get(&page).map(|s| s.strokes.clone()).unwrap_or_default();
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
                            let (x, y) = to_norm(rect, p);
                            let x = x.min(1.0 - TEXT_BOX_W).max(0.0);
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
    }
    changed
}

fn new_annotation(st: &SketchState, kind: AnnotationKind, points: Vec<(f32, f32)>) -> PageAnnotation {
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
    let before = sketches.get(&page).map(|s| s.strokes.clone()).unwrap_or_default();
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
    fn pen_draws_a_smoothed_stroke_inside_the_sheet() {
        let mut h = Harness::new(400.0, 600.0);
        h.drag((150.0, 250.0), (250.0, 350.0));
        assert_eq!(h.strokes().len(), 1, "one stroke committed");
        let s = &h.strokes()[0];
        assert_eq!(s.kind, AnnotationKind::Freehand);
        assert!(s.points.len() >= 2);
        assert!(s.points.iter().all(|&(x, y)| (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y)));
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
        assert_eq!(h.strokes().len(), 2, "one undo restores everything the drag erased");
    }

    #[test]
    fn text_box_is_edited_in_place_and_kept() {
        let mut h = Harness::new(400.0, 600.0);
        h.st.tool = SketchTool::Text;
        h.click(120.0, 200.0);
        assert!(h.st.edit.is_some(), "clicking with the text tool opens a box");
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
        for (w, hgt) in [(96.0_f32, 400.0_f32), (140.0, 300.0), (1600.0, 500.0), (1800.0, 160.0)] {
            let mut h = Harness::new(w, hgt);
            let cx = w * 0.5;
            // The sheet is centered (or starts at the left edge when it is wider than the panel).
            h.st.tool = SketchTool::Pen;
            h.drag((cx - 20.0, hgt * 0.62), (cx + 20.0, hgt * 0.78));
            h.st.tool = SketchTool::Rect;
            h.drag((cx - 30.0, hgt * 0.6), (cx + 30.0, hgt * 0.85));
            assert_eq!(h.strokes().len(), 2, "drawing works in a {w}x{hgt} panel");
            for s in h.strokes() {
                assert!(s.points.iter().all(|&(x, y)| (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y)));
            }
        }
    }

    #[test]
    fn orientation_and_paper_are_remembered_for_new_sheets() {
        let mut h = Harness::new(400.0, 600.0);
        h.st.new_paper = BlankPageStyle::Grid;
        h.st.new_aspect = 1.0 / notes::SKETCH_DEFAULT_ASPECT;
        h.drag((150.0, 250.0), (250.0, 300.0));
        let sheet = &h.sketches[&1];
        assert_eq!(sheet.paper, BlankPageStyle::Grid);
        assert!(sheet.aspect < 1.0);
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
                paint_paper(&painter, Rect::from_min_size(Pos2::new(5.0, 5.0), vec2(300.0, 424.0)), style);
            }
        });
        out.textures_delta.clear();
    }
}

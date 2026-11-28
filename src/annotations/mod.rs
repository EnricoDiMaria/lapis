//! Annotation data and drawing tools shared by the viewer, sketchpad, and export.

use crate::shortcuts::ShortcutsConfig;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum AnnotationKind {
    Highlight,
    Underline,
    Freehand,
    TextBox,
    Rect,
    Circle,
    Line,
    Arrow,
    Tape,
    Image,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageAnnotation {
    pub kind: AnnotationKind,
    pub color: [u8; 3],
    pub points: Vec<(f32, f32)>,
    #[serde(default)]
    pub text: String,
    #[serde(default = "default_textbox_size")]
    pub font_size: f32,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default = "default_annotation_stroke_width")]
    pub stroke_width: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) native_source: Option<NativeAnnotationSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct NativeAnnotationSource {
    pub(crate) key: String,
    pub(crate) part: usize,
    pub(crate) projection: String,
}

fn default_textbox_size() -> f32 {
    18.0
}

pub const DEFAULT_ANNOTATION_STROKE_WIDTH: f32 = 2.5;

fn default_annotation_stroke_width() -> f32 {
    DEFAULT_ANNOTATION_STROKE_WIDTH
}

/// Decimates jittery micro-points and applies one iteration of Chaikin's corner-cutting
/// B-spline smoothing so trackpad/mouse freehand strokes look crisp and calligraphic.
pub fn smooth_freehand_points(raw: &[(f32, f32)]) -> Vec<(f32, f32)> {
    if raw.len() <= 2 {
        return raw.to_vec();
    }

    // 1. Radial distance decimation (removes stationary trackpad jitter)
    let mut filtered = Vec::with_capacity(raw.len());
    filtered.push(raw[0]);
    for &pt in &raw[1..] {
        let &(lx, ly) = filtered.last().unwrap();
        if (pt.0 - lx).hypot(pt.1 - ly) >= 0.0022 {
            filtered.push(pt);
        }
    }
    if let Some(&last) = raw.last()
        && filtered.last() != Some(&last)
    {
        filtered.push(last);
    }
    if filtered.len() <= 2 {
        return filtered;
    }

    // 2. Chaikin 1/4 - 3/4 corner cutting
    let mut smoothed = Vec::with_capacity(filtered.len() * 2);
    smoothed.push(filtered[0]);
    for pair in filtered.windows(2) {
        let (x0, y0) = pair[0];
        let (x1, y1) = pair[1];
        let q = (0.75 * x0 + 0.25 * x1, 0.75 * y0 + 0.25 * y1);
        let r = (0.25 * x0 + 0.75 * x1, 0.25 * y0 + 0.75 * y1);
        smoothed.push(q);
        smoothed.push(r);
    }
    smoothed.push(*filtered.last().unwrap());
    smoothed
}

/// Annotation tools anchored at the bottom of the Notes panel.
#[derive(PartialEq, Clone, Copy)]
pub(crate) enum AnnotationTool {
    Cursor,
    Highlight,
    Underline,
    Draw,
    Shape,
    TextBox,
    Eraser,
    Tape,
    Snip,
    Laser,
}

impl AnnotationTool {
    pub(crate) fn color_slot(self) -> Option<usize> {
        match self {
            Self::Draw => Some(0),
            Self::Highlight => Some(1),
            Self::TextBox => Some(2),
            Self::Underline => Some(3),
            Self::Shape => Some(4),
            Self::Tape => Some(5),
            _ => None,
        }
    }
}

pub(crate) fn annotation_shortcut_hint(
    sc: &ShortcutsConfig,
    tool: AnnotationTool,
    label: &str,
) -> String {
    let binding = match tool {
        AnnotationTool::Cursor => &sc.select_cursor,
        AnnotationTool::Draw => &sc.select_pen,
        AnnotationTool::Highlight => &sc.select_highlighter,
        AnnotationTool::TextBox => &sc.select_textbox,
        AnnotationTool::Underline => &sc.select_underline,
        AnnotationTool::Eraser => &sc.select_eraser,
        AnnotationTool::Snip => &sc.select_snip,
        AnnotationTool::Tape => &sc.select_tape,
        AnnotationTool::Shape | AnnotationTool::Laser => return label.to_owned(),
    };
    binding.hint(label)
}

pub(crate) fn shape_shortcut_hint(sc: &ShortcutsConfig, shape: ShapeKind, label: &str) -> String {
    match shape {
        ShapeKind::Rect => &sc.select_rectangle,
        ShapeKind::Circle => &sc.select_circle,
        ShapeKind::Line => &sc.select_line,
        ShapeKind::Arrow => &sc.select_arrow,
    }
    .hint(label)
}

pub(crate) fn annotation_kind_color_slot(kind: AnnotationKind) -> Option<usize> {
    match kind {
        AnnotationKind::Freehand => AnnotationTool::Draw.color_slot(),
        AnnotationKind::Highlight => AnnotationTool::Highlight.color_slot(),
        AnnotationKind::TextBox => AnnotationTool::TextBox.color_slot(),
        AnnotationKind::Underline => AnnotationTool::Underline.color_slot(),
        AnnotationKind::Rect
        | AnnotationKind::Circle
        | AnnotationKind::Line
        | AnnotationKind::Arrow => AnnotationTool::Shape.color_slot(),
        AnnotationKind::Tape => AnnotationTool::Tape.color_slot(),
        AnnotationKind::Image => None,
    }
}

#[derive(PartialEq, Clone, Copy)]
pub(crate) enum ShapeKind {
    Rect,
    Circle,
    Line,
    Arrow,
}
pub(crate) const ANNOTATION_PALETTE: [([u8; 3], &str); 8] = [
    ([17, 24, 39], "Black"),
    ([156, 163, 175], "Gray"),
    ([255, 255, 255], "White"),
    ([250, 204, 21], "Yellow"),
    ([220, 38, 38], "Red"),
    ([22, 163, 74], "Green"),
    ([37, 99, 235], "Blue"),
    ([192, 132, 252], "Lavender"),
];

/// Freehand strokes within this distance of the eraser are removed (fraction of the page).
pub(crate) const ERASER_REACH: f32 = 0.025;

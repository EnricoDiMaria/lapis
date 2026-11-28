//! Shared annotation controls for the horizontal toolbar and vertical rail.

use super::super::*;
use state::{RailMetrics, SubBarMetrics};

const TOOLS: &[(AnnotationTool, &str, &str)] = &[
    (AnnotationTool::Cursor, "󰇀", "Cursor / Select PDF Text"),
    (AnnotationTool::Highlight, "", "Highlighter"),
    (AnnotationTool::Underline, "󰊇", "Underline"),
    (AnnotationTool::Draw, "󱦹", "Pen"),
    (AnnotationTool::Shape, "󰠱", "Shapes"),
    (AnnotationTool::TextBox, "󰊄", "Text Box"),
    (AnnotationTool::Tape, "󰝤", "Occlusion Tape"),
    (AnnotationTool::Snip, "󰆞", "Snip Region to Notes"),
    (AnnotationTool::Laser, "󰐾", "Laser Pointer"),
    (AnnotationTool::Eraser, "󰇾", "Eraser"),
];

const SHAPES: &[(ShapeKind, &str, &str)] = &[
    (ShapeKind::Rect, "󰝣", "Rectangle"),
    (ShapeKind::Circle, "󰝥", "Circle"),
    (ShapeKind::Line, "󰕞", "Line"),
    (ShapeKind::Arrow, "󰁔", "Arrow"),
];

const SHAPE_BUTTON_WIDTH: f32 = 72.0;
const STROKE_CONTROLS_WIDTH: f32 = 300.0;

#[derive(Debug, PartialEq, Eq)]
struct BarLayout {
    visible_tools: usize,
    inline_colors: bool,
    inline_shapes: bool,
    inline_thickness: bool,
}

impl BarLayout {
    fn for_width(width: f32, metrics: &SubBarMetrics, tool: AnnotationTool) -> Self {
        let button = metrics.btn[0] + metrics.spacing;
        let core = (TOOLS.len() + 2) as f32 * button - metrics.spacing;
        let colors = ANNOTATION_PALETTE.len() as f32 * (metrics.swatch_box + metrics.spacing);
        let shapes = SHAPES.len() as f32 * (SHAPE_BUTTON_WIDTH + metrics.spacing);
        let has_thickness = uses_stroke_width(tool);
        let inline_colors = width >= core + colors + if has_thickness { button } else { 0.0 };
        let inline_thickness =
            has_thickness && inline_colors && width >= core + colors + STROKE_CONTROLS_WIDTH;
        let inline_shapes = tool == AnnotationTool::Shape
            && inline_thickness
            && width >= core + colors + STROKE_CONTROLS_WIDTH + shapes;
        let visible_tools = if width >= core + button {
            TOOLS.len()
        } else {
            let slots = ((width + metrics.spacing) / button).floor() as usize;
            slots.saturating_sub(4).min(TOOLS.len())
        };
        Self {
            visible_tools,
            inline_colors,
            inline_shapes,
            inline_thickness,
        }
    }
}

fn uses_stroke_width(tool: AnnotationTool) -> bool {
    matches!(
        tool,
        AnnotationTool::Draw
            | AnnotationTool::Underline
            | AnnotationTool::Shape
            | AnnotationTool::Tape
            | AnnotationTool::Laser
    )
}

impl Lapis {
    pub(in crate::workspace) fn draw_sub_topbar_tools(&mut self, ui: &mut egui::Ui, bottom: bool) {
        let metrics = self.app_state.settings.ui_density.sub_bar();
        let panel = if bottom {
            egui::Panel::bottom("sub_topbar_tools")
        } else {
            egui::Panel::top("sub_topbar_tools")
        };
        let panel_out = panel
            .exact_size(metrics.height)
            .frame(
                egui::Frame::NONE
                    .fill(rgb(self.theme.bg_elevated))
                    .inner_margin(egui::Margin::symmetric(12, metrics.margin_y))
                    .stroke(egui::Stroke::new(1.0, rgb(self.theme.border))),
            )
            .show(ui, |ui| {
                let align = if bottom {
                    egui::RectAlign::TOP_START
                } else {
                    egui::RectAlign::BOTTOM_START
                };
                self.horizontal_annotation_controls(ui, &metrics, align);
            });

        self.subbar_h = panel_out.response.rect.height();
    }

    fn horizontal_annotation_controls(
        &mut self,
        ui: &mut egui::Ui,
        metrics: &SubBarMetrics,
        align: egui::RectAlign,
    ) -> egui::Response {
        let width = ui.available_width();
        let shape_layout = BarLayout::for_width(width, metrics, AnnotationTool::Shape);
        let button_size = egui::Vec2::from(metrics.btn);
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = metrics.spacing;
            ui.spacing_mut().button_padding.y = metrics.pad_y;
            for &tool in &TOOLS[..shape_layout.visible_tools] {
                self.annotation_tool_control(
                    ui,
                    tool,
                    button_size,
                    metrics.icon,
                    (!shape_layout.inline_shapes).then_some(align),
                );
            }
            if shape_layout.visible_tools < TOOLS.len() {
                let more = ui
                    .add_sized(button_size, egui::Button::new("⋯"))
                    .on_hover_text("More annotation tools");
                egui::Popup::menu(&more)
                    .align(align)
                    .style(self.menu_glass().modifier())
                    .show(|ui| {
                        ui.set_min_width(170.0);
                        for &(tool, _, label) in &TOOLS[shape_layout.visible_tools..] {
                            if ui
                                .add(theme::selected_button(
                                    label,
                                    self.annotation_tool == tool,
                                    ui.visuals(),
                                ))
                                .on_hover_text(annotation_shortcut_hint(
                                    &self.app_state.shortcuts,
                                    tool,
                                    label,
                                ))
                                .clicked()
                            {
                                self.annotation_tool = tool;
                                ui.close();
                            }
                        }
                    });
            }

            self.annotation_history_buttons(ui, button_size);
            let layout = BarLayout::for_width(width, metrics, self.annotation_tool);
            if layout.inline_colors {
                self.annotation_swatches(ui, metrics);
            }
            if layout.inline_shapes {
                self.annotation_shapes(
                    ui,
                    egui::vec2(SHAPE_BUTTON_WIDTH, metrics.shape_btn_h),
                    true,
                );
            }
            if layout.inline_thickness {
                self.annotation_thickness(ui);
            }
            let popup_thickness =
                uses_stroke_width(self.annotation_tool) && !layout.inline_thickness;
            if !layout.inline_colors || popup_thickness {
                self.annotation_options(
                    ui,
                    button_size,
                    align,
                    !layout.inline_colors,
                    popup_thickness,
                );
            }
        })
        .response
    }

    pub(in crate::workspace) fn render_vertical_tools_rail(&mut self, ui: &mut egui::Ui) {
        let metrics = self.app_state.settings.ui_density.rail();
        egui::Panel::right("vertical_tools_rail")
            .resizable(false)
            .exact_size(metrics.width)
            .frame(
                egui::Frame::NONE
                    .fill(rgb(self.theme.bg_panel))
                    .inner_margin(egui::Margin::symmetric(
                        metrics.margin[0],
                        metrics.margin[1],
                    ))
                    .stroke(egui::Stroke::new(1.0, rgb(self.theme.border))),
            )
            .show(ui, |ui| {
                self.vertical_annotation_controls(ui, &metrics);
            });
    }

    fn vertical_annotation_controls(
        &mut self,
        ui: &mut egui::Ui,
        metrics: &RailMetrics,
    ) -> egui::scroll_area::ScrollAreaOutput<()> {
        ui.spacing_mut().scroll = egui::style::ScrollStyle::floating();
        ui.spacing_mut().scroll.floating_width = 4.0;
        egui::ScrollArea::vertical()
            .id_salt("annotation_rail_scroll")
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.spacing_mut().item_spacing.y = metrics.spacing;
                    ui.spacing_mut().button_padding.y = metrics.pad_y;
                    let button_size = egui::Vec2::from(metrics.btn);
                    let align = egui::RectAlign::LEFT_START;
                    for &tool in TOOLS {
                        self.annotation_tool_control(
                            ui,
                            tool,
                            button_size,
                            metrics.icon,
                            Some(align),
                        );
                    }
                    self.annotation_options(
                        ui,
                        button_size,
                        align,
                        true,
                        uses_stroke_width(self.annotation_tool),
                    );
                    self.annotation_history_buttons(ui, button_size);
                });
            })
    }

    fn annotation_tool_control(
        &mut self,
        ui: &mut egui::Ui,
        (tool, icon, label): (AnnotationTool, &str, &str),
        size: egui::Vec2,
        icon_size: f32,
        shape_popup: Option<egui::RectAlign>,
    ) {
        let button = theme::selected_button(
            egui::RichText::new(icon).size(icon_size),
            self.annotation_tool == tool,
            ui.visuals(),
        );
        let tip = annotation_shortcut_hint(&self.app_state.shortcuts, tool, label);
        let response = ui.add_sized(size, button).on_hover_text(tip);
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
        if response.clicked() {
            self.annotation_tool = tool;
        }
        if tool == AnnotationTool::Shape
            && let Some(align) = shape_popup
        {
            egui::Popup::menu(&response)
                .align(align)
                .style(self.menu_glass().modifier())
                .show(|ui| {
                    ui.horizontal(|ui| {
                        self.annotation_shapes(ui, egui::vec2(34.0, size.y), false);
                    });
                });
        }
    }

    fn annotation_options(
        &mut self,
        ui: &mut egui::Ui,
        size: egui::Vec2,
        align: egui::RectAlign,
        colors: bool,
        thickness: bool,
    ) {
        let response = ui
            .add_sized(size, egui::Button::new("󰇘"))
            .on_hover_text("Annotation options");
        egui::Popup::menu(&response)
            .align(align)
            .style(self.menu_glass().modifier())
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                if colors {
                    ui.label("Annotation color");
                    ui.horizontal(|ui| {
                        self.annotation_swatches(ui, &self.app_state.settings.ui_density.sub_bar());
                    });
                    if self.annotation_tool == AnnotationTool::Shape {
                        ui.separator();
                        ui.horizontal(|ui| {
                            self.annotation_shapes(ui, egui::vec2(34.0, size.y), false)
                        });
                    }
                }
                if thickness {
                    if colors {
                        ui.separator();
                    }
                    ui.horizontal(|ui| self.annotation_thickness(ui));
                }
            });
    }

    fn annotation_swatches(&mut self, ui: &mut egui::Ui, metrics: &SubBarMetrics) {
        let current = self.annotation_color();
        for (color, name) in ANNOTATION_PALETTE {
            let (rect, response) =
                ui.allocate_exact_size(egui::Vec2::splat(metrics.swatch_box), egui::Sense::click());
            let selected = current == color;
            ui.painter()
                .circle_filled(rect.center(), metrics.swatch_fill, rgb(color));
            if selected || response.hovered() || response.has_focus() {
                ui.painter().circle_stroke(
                    rect.center(),
                    metrics.swatch_ring,
                    egui::Stroke::new(if selected { 2.0 } else { 1.2 }, rgb(self.theme.fg_primary)),
                );
            }
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name));
            if response.on_hover_text(name).clicked() {
                self.pending_annotation_color = Some(color);
            }
        }
    }

    fn annotation_shapes(&mut self, ui: &mut egui::Ui, size: egui::Vec2, show_labels: bool) {
        for &(shape, icon, label) in SHAPES {
            let text = if show_labels { label } else { icon };
            let button =
                theme::selected_button(text, self.shape_kind == shape, ui.visuals()).truncate();
            if ui
                .add_sized(size, button)
                .on_hover_text(shape_shortcut_hint(&self.app_state.shortcuts, shape, label))
                .clicked()
            {
                self.shape_kind = shape;
                if !show_labels {
                    ui.close();
                }
            }
        }
    }

    fn annotation_thickness(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().slider_width = 86.0;
        ui.label("Stroke width");
        ui.add(egui::Slider::new(&mut self.stroke_width, 1.0..=18.0).suffix(" px"));
        if ui
            .small_button("↺")
            .on_hover_text("Reset stroke width to 2.5 px")
            .clicked()
        {
            self.stroke_width = annotations::DEFAULT_ANNOTATION_STROKE_WIDTH;
        }
    }

    fn annotation_history_buttons(&mut self, ui: &mut egui::Ui, size: egui::Vec2) {
        let page = self.current_page;
        if ui
            .add_enabled(
                self.can_undo_annotation(page),
                egui::Button::new("󰕌").min_size(size),
            )
            .on_hover_text("Undo PDF Annotation [Ctrl+Z]")
            .clicked()
        {
            self.undo_annotation(page);
        }

        if ui
            .add_enabled(
                self.can_redo_annotation(page),
                egui::Button::new("󰑎").min_size(size),
            )
            .on_hover_text("Redo PDF Annotation [Ctrl+Y]")
            .clicked()
        {
            self.redo_annotation(page);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::isolated_dir;

    #[test]
    fn annotation_controls_fit_narrow_and_wide_rows_at_each_density() {
        let _root = isolated_dir!(annotation_controls_fit_narrow_and_wide_rows_at_each_density);
        let ctx = egui::Context::default();
        let mut app = Lapis::new(&eframe::CreationContext::_new_kittest(ctx.clone()), None);
        app.annotation_tool = AnnotationTool::Shape;
        for density in [UiDensity::Standard, UiDensity::Compact, UiDensity::Ultra] {
            app.theme.apply_density(&ctx, density);
            let metrics = density.sub_bar();
            for (width, colors, thickness, shapes) in [
                (320.0, false, false, false),
                (640.0, true, false, false),
                (800.0, true, density == UiDensity::Ultra, false),
                (900.0, true, true, false),
                (1400.0, true, true, true),
            ] {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, metrics.btn[1]),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        let width = ui.available_width();
                        let layout = BarLayout::for_width(width, &metrics, app.annotation_tool);
                        assert_eq!(layout.inline_colors, colors, "{density:?} at {width} pt");
                        assert_eq!(
                            layout.inline_thickness, thickness,
                            "{density:?} at {width} pt"
                        );
                        assert_eq!(layout.inline_shapes, shapes, "{density:?} at {width} pt");
                        assert_eq!(layout.visible_tools == TOOLS.len(), width >= 640.0);
                        let bounds = ui.max_rect();
                        let response = app.horizontal_annotation_controls(
                            ui,
                            &metrics,
                            egui::RectAlign::TOP_START,
                        );
                        assert!(
                            response.rect.right() <= bounds.right() + 0.5,
                            "{density:?}: {response:?} exceeds {bounds:?}"
                        );
                    },
                );
                output.textures_delta.clear();
            }
        }
    }

    #[test]
    fn swatches_fill_the_space_left_by_tools_without_stroke_width() {
        let metrics = UiDensity::Standard.sub_bar();
        let layout = BarLayout::for_width(580.0, &metrics, AnnotationTool::Highlight);
        assert!(layout.inline_colors);
        assert!(!layout.inline_thickness);
    }

    #[test]
    fn short_annotation_rail_can_scroll_to_its_last_controls() {
        let _root = isolated_dir!(short_annotation_rail_can_scroll_to_its_last_controls);
        let ctx = egui::Context::default();
        let mut app = Lapis::new(&eframe::CreationContext::_new_kittest(ctx.clone()), None);
        let metrics = UiDensity::Standard.rail();
        let draw = |app: &mut Lapis, events| {
            let mut scroll = (0.0, 0.0);
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(48.0, 320.0),
                )),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(raw, |ui| {
                let area = app.vertical_annotation_controls(ui, &metrics);
                scroll = (
                    area.state.clamped_offset().y,
                    area.content_size.y - area.inner_rect.height(),
                );
            });
            output.textures_delta.clear();
            scroll
        };
        let (top, overflow) = draw(&mut app, vec![]);
        assert_eq!(top, 0.0);
        assert!(overflow > 0.0);
        let (offset, _) = draw(
            &mut app,
            vec![
                egui::Event::PointerMoved(egui::pos2(20.0, 120.0)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -500.0),
                    phase: egui::TouchPhase::Move,
                    source: egui::MouseWheelSource::Unknown,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        assert!(offset > 0.0);
    }
}

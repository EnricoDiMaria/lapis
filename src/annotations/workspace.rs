use super::*;

impl Lapis {
    pub(super) fn annotation_color(&self) -> [u8; 3] {
        self.pending_annotation_color.unwrap_or_else(|| {
            self.annotation_tool
                .color_slot()
                .map_or(self.last_annotation_color, |slot| {
                    self.app_state.annotation_colors[slot]
                })
        })
    }

    pub(super) fn remember_annotation_color(
        &mut self,
        kind: annotations::AnnotationKind,
        color: [u8; 3],
    ) {
        if let Some(slot) = annotation_kind_color_slot(kind)
            && self.app_state.annotation_colors[slot] != color
        {
            self.app_state.annotation_colors[slot] = color;
            self.app_state.save();
        }
    }

    pub(super) fn commit_annotation_color(
        &mut self,
        kind: annotations::AnnotationKind,
        color: [u8; 3],
    ) {
        if self.pending_annotation_color == Some(color) {
            self.pending_annotation_color = None;
        }
        self.last_annotation_color = color;
        self.remember_annotation_color(kind, color);
    }

    pub(super) fn paint_annotation(
        painter: &egui::Painter,
        slide_rect: egui::Rect,
        ann: &annotations::PageAnnotation,
    ) {
        let to_screen = |(nx, ny): (f32, f32)| -> egui::Pos2 {
            egui::pos2(
                slide_rect.left() + nx * slide_rect.width(),
                slide_rect.top() + ny * slide_rect.height(),
            )
        };

        let base_col = rgb(ann.color);

        // Unified stroke thickness for all annotations.
        let thickness = ann.stroke_width.clamp(1.0, 18.0);

        match ann.kind {
            // ============================================================
            // HIGHLIGHT
            // Filled rectangle: no adjustable stroke.
            // ============================================================
            annotations::AnnotationKind::Highlight => {
                if ann.points.len() >= 2 {
                    let p1 = to_screen(ann.points[0]);
                    let p2 = to_screen(*ann.points.last().unwrap());
                    let rect = egui::Rect::from_two_pos(p1, p2);

                    painter.rect_filled(
                        rect,
                        egui::CornerRadius::same(2),
                        egui::Color32::from_rgba_unmultiplied(
                            base_col.r(),
                            base_col.g(),
                            base_col.b(),
                            95,
                        ),
                    );
                }
            }

            // ============================================================
            // UNDERLINE
            // ============================================================
            annotations::AnnotationKind::Underline => {
                if ann.points.len() >= 2 {
                    let p1 = to_screen(ann.points[0]);
                    let p2 = to_screen(*ann.points.last().unwrap());

                    let start = egui::pos2(p1.x.min(p2.x), p1.y);
                    let end = egui::pos2(p1.x.max(p2.x), p1.y);

                    painter.line_segment([start, end], egui::Stroke::new(thickness, base_col));
                }
            }

            // ============================================================
            // FREEHAND / DRAW
            // ============================================================
            annotations::AnnotationKind::Freehand => {
                if ann.points.len() >= 2 {
                    let pts: Vec<egui::Pos2> = ann.points.iter().copied().map(to_screen).collect();

                    painter.add(egui::Shape::line(
                        pts,
                        egui::Stroke::new(thickness, base_col),
                    ));
                }
            }

            // ============================================================
            // TEXT BOX
            // ============================================================
            annotations::AnnotationKind::TextBox => {}

            // ============================================================
            // RECTANGLE
            // ============================================================
            annotations::AnnotationKind::Rect => {
                if ann.points.len() >= 2 {
                    let p1 = to_screen(ann.points[0]);
                    let p2 = to_screen(*ann.points.last().unwrap());
                    let rect = egui::Rect::from_two_pos(p1, p2);

                    painter.rect_stroke(
                        rect,
                        egui::CornerRadius::same(2),
                        egui::Stroke::new(thickness, base_col),
                        egui::StrokeKind::Middle,
                    );
                }
            }

            // ============================================================
            // CIRCLE
            // ============================================================
            annotations::AnnotationKind::Circle => {
                if ann.points.len() >= 2 {
                    let p1 = to_screen(ann.points[0]);
                    let p2 = to_screen(*ann.points.last().unwrap());
                    let rect = egui::Rect::from_two_pos(p1, p2);

                    let radius = rect.width().min(rect.height()) * 0.5;

                    painter.circle_stroke(
                        rect.center(),
                        radius,
                        egui::Stroke::new(thickness, base_col),
                    );
                }
            }

            // ============================================================
            // LINE
            // ============================================================
            annotations::AnnotationKind::Line => {
                if ann.points.len() >= 2 {
                    let p1 = to_screen(ann.points[0]);
                    let p2 = to_screen(*ann.points.last().unwrap());

                    painter.line_segment([p1, p2], egui::Stroke::new(thickness, base_col));
                }
            }

            // ============================================================
            // ARROW
            // ============================================================
            annotations::AnnotationKind::Arrow => {
                if ann.points.len() >= 2 {
                    let p1 = to_screen(ann.points[0]);
                    let p2 = to_screen(*ann.points.last().unwrap());

                    // Main arrow line
                    painter.line_segment([p1, p2], egui::Stroke::new(thickness, base_col));

                    // Arrow head
                    let dir = p2 - p1;
                    let len = dir.length();

                    if len > 4.0 {
                        let u = dir / len;
                        let perp = egui::vec2(-u.y, u.x);

                        let head_len = (thickness * 4.2).clamp(9.0, 18.0);

                        let wing1 = p2 - u * head_len + perp * (head_len * 0.55);

                        let wing2 = p2 - u * head_len - perp * (head_len * 0.55);

                        painter.line_segment([p2, wing1], egui::Stroke::new(thickness, base_col));

                        painter.line_segment([p2, wing2], egui::Stroke::new(thickness, base_col));
                    }
                }
            }

            // ============================================================
            // OCCLUSION TAPE
            // ============================================================
            annotations::AnnotationKind::Tape => {
                if ann.points.len() >= 2 {
                    let p1 = to_screen(ann.points[0]);
                    let p2 = to_screen(*ann.points.last().unwrap());
                    let rect = egui::Rect::from_two_pos(p1, p2);

                    painter.rect_filled(
                        rect,
                        egui::CornerRadius::same(4),
                        egui::Color32::from_rgba_unmultiplied(
                            ann.color[0],
                            ann.color[1],
                            ann.color[2],
                            248,
                        ),
                    );

                    painter.rect_stroke(
                        rect,
                        egui::CornerRadius::same(4),
                        egui::Stroke::new(thickness, egui::Color32::from_white_alpha(180)),
                        egui::StrokeKind::Inside,
                    );

                    if rect.width() > 45.0 && rect.height() > 16.0 {
                        painter.text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "󰈈 Peek",
                            egui::FontId::proportional(11.5),
                            egui::Color32::from_black_alpha(190),
                        );
                    }
                }
            }

            // ============================================================
            // IMAGE
            // ============================================================
            annotations::AnnotationKind::Image => {
                // Images are rendered separately by image_annotations_ui.
            }
        }
    }

    fn render_page_textboxes(&mut self, ui: &mut egui::Ui, page: u32, page_rect: egui::Rect) {
        const MIN_W: f32 = 0.04; // Page-width fraction.
        const MIN_H: f32 = 0.02;
        const DEF_W: f32 = 0.16;
        const DEF_H: f32 = 0.04;
        const HIT_PAD: f32 = 6.0; // Text hit-area padding.
        const RING: f32 = 10.0; // Drag handle size.

        struct Geo {
            idx: usize,
            p0: (f32, f32),
            p1: (f32, f32),
            text: String,
            pad_i8: i8,
            pad: f32,
            box_w: f32,
            font_id: egui::FontId,
            col: egui::Color32,
            bold: bool,
            italic: bool,
            fs: f32,
            color: [u8; 3],
            bg_mode: u8,
            galley: std::sync::Arc<egui::Galley>,
            rect: egui::Rect,
            hit: egui::Rect,
        }

        // Costruisce il LayoutJob supportando corsivo globale e mini-formattazione inline:
        // - Replace a leading "- " with "• ".
        // - __text__ -> underline
        // - ~~text~~ -> strikethrough
        fn build_text_job(
            raw: &str,
            font_id: egui::FontId,
            col: egui::Color32,
            italic: bool,
            wrap_w: f32,
            render_mode: bool,
        ) -> egui::text::LayoutJob {
            let raw = raw
                .strip_prefix('\u{200B}')
                .or_else(|| raw.strip_prefix('\u{200C}'))
                .unwrap_or(raw);
            let mut job = egui::text::LayoutJob::default();
            job.wrap.max_width = wrap_w;

            let base_fmt = egui::TextFormat {
                font_id,
                color: col,
                italics: italic,
                ..Default::default()
            };

            // Preserve character positions while editing.
            // Replace a leading "- " with "• " in static mode.
            let processed: std::borrow::Cow<'_, str> = if render_mode {
                let mut out = String::with_capacity(raw.len());
                for (li, line) in raw.split('\n').enumerate() {
                    if li > 0 {
                        out.push('\n');
                    }
                    if let Some(rest) = line.strip_prefix("- ") {
                        out.push_str("• ");
                        out.push_str(rest);
                    } else {
                        out.push_str(line);
                    }
                }
                std::borrow::Cow::Owned(out)
            } else {
                std::borrow::Cow::Borrowed(raw)
            };

            // Parse underline and strikethrough without changing text length.
            // Keep block layout unchanged.
            let s = processed.as_ref();
            let mut i = 0;
            let bytes = s.as_bytes();
            let mut cur_start = 0;
            let mut underline = false;
            let mut strikethrough = false;

            let make_fmt = |u: bool, st: bool| {
                let mut f = base_fmt.clone();
                if u {
                    f.underline = egui::Stroke::new(1.2, col);
                }
                if st {
                    f.strikethrough = egui::Stroke::new(1.2, col);
                }
                f
            };

            while i + 1 < bytes.len() {
                let is_u = bytes[i] == b'_' && bytes[i + 1] == b'_';
                let is_s = bytes[i] == b'~' && bytes[i + 1] == b'~';
                if is_u || is_s {
                    if cur_start < i {
                        job.append(&s[cur_start..i], 0.0, make_fmt(underline, strikethrough));
                    }
                    if render_mode {
                        // Hide __ and ~~ delimiters in static mode.
                        if is_u {
                            underline = !underline;
                        } else {
                            strikethrough = !strikethrough;
                        }
                        i += 2;
                        cur_start = i;
                    } else {
                        // Dim delimiters while editing to preserve cursor indices.
                        let mut delim_fmt = base_fmt.clone();
                        delim_fmt.color = col.gamma_multiply(0.45);
                        if is_u {
                            underline = !underline;
                        } else {
                            strikethrough = !strikethrough;
                        }
                        job.append(&s[i..i + 2], 0.0, delim_fmt);
                        i += 2;
                        cur_start = i;
                    }
                } else {
                    // Avanza al prossimo char boundary UTF-8
                    i += s[i..].chars().next().map_or(1, |c| c.len_utf8());
                }
            }
            if cur_start < s.len() {
                job.append(&s[cur_start..], 0.0, make_fmt(underline, strikethrough));
            }
            if s.is_empty() {
                job.append("", 0.0, base_fmt);
            }
            job
        }

        let interactive = matches!(
            self.annotation_tool,
            AnnotationTool::Cursor | AnnotationTool::TextBox
        );
        let accent = rgb(self.theme.accent);
        let fg = rgb(self.theme.fg_primary);

        let pw = page_rect.width().max(1.0);
        let ph = page_rect.height().max(1.0);
        let font_scale = ph / 800.0;
        let to_screen =
            |p: (f32, f32)| egui::pos2(page_rect.left() + p.0 * pw, page_rect.top() + p.1 * ph);

        let mut dirty = false;
        let mut just_created_this_frame = false;

        // Selection state: page, index, and editing status.
        let sel_key = ui.make_persistent_id(("tb_sel_state", self.textbox_epoch));
        let edit_prev_key = ui.make_persistent_id(("tb_prev_edit", self.textbox_epoch));
        let recorded_edit_key = edit_prev_key.with("recorded");
        let mut sel: Option<(u32, usize, bool)> = ui.data(|d| d.get_temp(sel_key));

        if let Some((sp, si, _)) = sel {
            let ok = self
                .annotations
                .get(&sp)
                .and_then(|v| v.get(si))
                .is_some_and(|a| a.kind == annotations::AnnotationKind::TextBox);
            if !ok {
                sel = None;
            }
        }
        if !interactive {
            sel = None;
        }

        // Remove empty, unselected blocks.
        {
            let sel_here = sel.filter(|s| s.0 == page).map(|s| s.1);
            let empties: Vec<usize> = self
                .annotations
                .get(&page)
                .map(|v| {
                    v.iter()
                        .enumerate()
                        .filter(|(i, a)| {
                            a.kind == annotations::AnnotationKind::TextBox
                                && a.text
                                    .trim_matches(['\u{200B}', '\u{200C}'])
                                    .trim()
                                    .is_empty()
                                && Some(*i) != sel_here
                        })
                        .map(|(i, _)| i)
                        .collect()
                })
                .unwrap_or_default();
            if !empties.is_empty() {
                if let Some(list) = self.annotations.get_mut(&page) {
                    for &i in empties.iter().rev() {
                        list.remove(i);
                    }
                }
                if let Some((sp, si, ed)) = sel
                    && sp == page
                {
                    let shift = empties.iter().filter(|&&e| e < si).count();
                    sel = Some((sp, si - shift, ed));
                }
                dirty = true;
            }
        }

        // ───────── 1. Creazione (strumento Text) ─────────
        if interactive && self.annotation_tool == AnnotationTool::TextBox {
            let bg = ui.interact(
                page_rect,
                ui.make_persistent_id(("tb_create", page)),
                egui::Sense::click_and_drag(),
            );
            if bg.hovered() || bg.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
            let start_id = ui.make_persistent_id(("tb_create_start", page));

            if bg.drag_started()
                && let Some(p) = bg.interact_pointer_pos()
            {
                ui.data_mut(|d| d.insert_temp(start_id, p));
            }
            let start: Option<egui::Pos2> = ui.data(|d| d.get_temp(start_id));

            if bg.dragged()
                && let (Some(a), Some(b)) = (start, bg.interact_pointer_pos())
            {
                ui.painter().rect_stroke(
                    egui::Rect::from_two_pos(a, b),
                    egui::CornerRadius::same(3),
                    egui::Stroke::new(1.5, accent),
                    egui::StrokeKind::Outside,
                );
            }

            let created: Option<egui::Rect> = if bg.drag_stopped() {
                ui.data_mut(|d| d.remove::<egui::Pos2>(start_id));
                match (start, bg.interact_pointer_pos()) {
                    (Some(a), Some(b)) => Some(egui::Rect::from_two_pos(a, b)),
                    _ => None,
                }
            } else if bg.clicked() {
                bg.interact_pointer_pos()
                    .map(|p| egui::Rect::from_min_size(p, egui::vec2(0.0, 0.0)))
            } else {
                None
            };

            if let Some(mut r) = created {
                if r.width() < 24.0 || r.height() < 16.0 {
                    r = egui::Rect::from_min_size(r.min, egui::vec2(DEF_W * pw, DEF_H * ph));
                }
                let w = (r.width() / pw).clamp(MIN_W, 1.0);
                let h = (r.height() / ph).clamp(MIN_H, 1.0);
                let nx = ((r.left() - page_rect.left()) / pw).clamp(0.0, 1.0 - w);
                let ny = ((r.top() - page_rect.top()) / ph).clamp(0.0, 1.0 - h);

                self.push_undo_snapshot(page);
                let ann = annotations::PageAnnotation {
                    kind: annotations::AnnotationKind::TextBox,
                    color: self.annotation_color(),
                    points: vec![(nx, ny), (nx + w, ny + h)],
                    text: String::new(),
                    font_size: self.textbox_font_size,
                    bold: self.textbox_bold,
                    italic: self.textbox_italic,
                    stroke_width: self.stroke_width,
                    native_source: None,
                };
                self.commit_annotation_color(ann.kind, ann.color);
                let list = self.annotations.entry(page).or_default();
                list.push(ann);
                let new_idx = list.len() - 1;
                sel = Some((page, new_idx, true)); // Enter edit mode immediately.
                ui.data_mut(|d| d.remove::<(u32, usize)>(edit_prev_key)); // Focus immediately.
                just_created_this_frame = true;
                self.annotation_tool = AnnotationTool::Cursor;
                dirty = true;
            }
        }

        // 2. Geometry for all page blocks.
        let n = self.annotations.get(&page).map(|v| v.len()).unwrap_or(0);
        let mut geos: Vec<Geo> = Vec::new();
        let sel_idx = sel.filter(|s| s.0 == page).map(|s| s.1);
        let editing_here = sel.filter(|s| s.0 == page).is_some_and(|s| s.2);

        for idx in 0..n {
            let (p0, p1, text, fs, bold, italic, color) = {
                let Some(ann) = self.annotations.get(&page).and_then(|v| v.get(idx)) else {
                    break;
                };
                if ann.kind != annotations::AnnotationKind::TextBox {
                    continue;
                }
                let p0 = ann.points.first().copied().unwrap_or((0.1, 0.1));
                let p1 = ann
                    .points
                    .get(1)
                    .copied()
                    .unwrap_or((p0.0 + DEF_W, p0.1 + DEF_H));
                (
                    p0,
                    p1,
                    ann.text.clone(),
                    ann.font_size,
                    ann.bold,
                    ann.italic,
                    ann.color,
                )
            };

            let screen_fs = (fs * font_scale).clamp(6.0, 200.0);
            // Convert the integer padding to f32 to keep layout coordinates pixel-aligned.
            let pad_i8 = (screen_fs * 0.35).round().clamp(2.0, 64.0) as i8;
            let pad = f32::from(pad_i8);

            let tl = to_screen(p0);
            let br = to_screen(p1);
            let box_w = (br.x - tl.x).max(pw * MIN_W);
            let wrap_w = (box_w - 2.0 * pad).max(10.0);
            let font_id = egui::FontId::monospace(screen_fs);
            let col = rgb(color);

            let is_editing_this = sel_idx == Some(idx) && editing_here;
            let bg_mode = if text.starts_with('\u{200B}') {
                1
            } else if text.starts_with('\u{200C}') {
                2
            } else {
                0
            };
            let job = build_text_job(
                &text,
                font_id.clone(),
                col,
                italic,
                wrap_w,
                !is_editing_this,
            );
            let galley = ui.fonts_mut(|f| f.layout_job(job));

            let box_h = galley.size().y + 2.0 * pad;
            let rect = egui::Rect::from_min_size(tl, egui::vec2(box_w, box_h));

            let hit_w = (galley.size().x + 2.0 * pad).min(box_w).max(16.0);
            let hit_h = (galley.size().y + 2.0 * pad).min(box_h);
            let hit = egui::Rect::from_min_size(tl, egui::vec2(hit_w, hit_h)).expand(HIT_PAD);

            geos.push(Geo {
                idx,
                p0,
                p1,
                text,
                pad_i8,
                pad,
                box_w,
                font_id,
                col,
                bold,
                italic,
                fs,
                color,
                bg_mode,
                galley,
                rect,
                hit,
            });
        }

        let mut order: Vec<usize> = (0..geos.len()).collect();
        order.sort_by(|&a, &b| {
            let (ga, gb) = (&geos[a], &geos[b]);
            let (sa, sb) = (Some(ga.idx) == sel_idx, Some(gb.idx) == sel_idx);
            sa.cmp(&sb).then_with(|| {
                gb.hit
                    .area()
                    .partial_cmp(&ga.hit.area())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
        });

        let mut sel_rect: Option<egui::Rect> = None;
        let mut toolbar_rect: Option<egui::Rect> = None;

        // Scale the offset with the font size to emphasize bold text.
        let draw_bold_galley = |painter: &egui::Painter,
                                pos: egui::Pos2,
                                galley: &std::sync::Arc<egui::Galley>,
                                col: egui::Color32,
                                fs: f32| {
            let off = (fs * 0.045).clamp(0.6, 2.2);
            painter.galley(pos + egui::vec2(off * 0.5, 0.0), galley.clone(), col);
            painter.galley(pos + egui::vec2(off, 0.0), galley.clone(), col);
        };

        // 3. Drawing and interaction.
        for gi in order {
            let g = &geos[gi];
            let idx = g.idx;
            let selected = sel_idx == Some(idx);
            let editing = selected && editing_here;
            let mut rect = g.rect;

            let mut move_delta = egui::Vec2::ZERO;
            let mut resize_to: Option<egui::Pos2> = None;

            if g.bg_mode != 0 {
                let bg_fill = if g.bg_mode == 1 {
                    // First click: dark translucent gray.
                    egui::Color32::from_rgba_unmultiplied(36, 38, 46, 200)
                } else {
                    // Second click: light translucent gray.
                    egui::Color32::from_rgba_unmultiplied(245, 246, 250, 215)
                };
                let bg_rect = if selected || editing {
                    rect
                } else {
                    g.hit.shrink(HIT_PAD)
                };
                ui.painter()
                    .rect_filled(bg_rect, egui::CornerRadius::same(4), bg_fill);
            }

            // Static text when not editing.
            if !editing {
                let painter = ui.painter();
                let pos = rect.min + egui::vec2(g.pad, g.pad);
                painter.galley(pos, g.galley.clone(), g.col);
                if g.bold {
                    draw_bold_galley(painter, pos, &g.galley, g.col, g.font_id.size);
                }
            }

            if !interactive {
                continue;
            }

            // Unselected block.
            if !selected {
                let resp = ui.interact(
                    g.hit,
                    ui.make_persistent_id(("tb_hit", page, idx)),
                    egui::Sense::click_and_drag(),
                );
                let text_rect = g.hit.shrink(HIT_PAD);
                let hot = resp.hovered() || resp.dragged();
                if hot {
                    ui.painter().rect_stroke(
                        text_rect,
                        egui::CornerRadius::same(3),
                        egui::Stroke::new(1.0, accent.gamma_multiply(0.95)),
                        egui::StrokeKind::Outside,
                    );
                    ui.ctx().set_cursor_icon(if resp.dragged() {
                        egui::CursorIcon::Grabbing
                    } else {
                        egui::CursorIcon::Grab
                    });
                }

                if resp.double_clicked() {
                    sel = Some((page, idx, true));
                } else if resp.drag_started() {
                    self.push_undo_snapshot(page);
                    sel = Some((page, idx, false));
                } else if resp.clicked() {
                    sel = Some((page, idx, false));
                }
                if resp.dragged() {
                    move_delta += resp.drag_delta();
                }
            } else {
                if !editing {
                    let mv = ui.interact(
                        rect.expand(HIT_PAD),
                        ui.make_persistent_id(("tb_move", page, idx)),
                        egui::Sense::click_and_drag(),
                    );
                    if mv.hovered() || mv.dragged() {
                        ui.ctx().set_cursor_icon(if mv.dragged() {
                            egui::CursorIcon::Grabbing
                        } else {
                            egui::CursorIcon::Grab
                        });
                    }
                    if mv.drag_started() {
                        self.push_undo_snapshot(page);
                    }
                    if mv.dragged() {
                        move_delta += mv.drag_delta();
                    }
                    if mv.clicked() {
                        sel = Some((page, idx, true));
                    }
                }

                // Editing the block.
                if editing {
                    let te_id = ui.make_persistent_id(("tb_text", self.textbox_epoch, page, idx));
                    let prev_edit: Option<(u32, usize)> = ui.data(|d| d.get_temp(edit_prev_key));
                    let need_init_cursor = prev_edit != Some((page, idx));
                    if need_init_cursor {
                        ui.data_mut(|d| d.remove::<(u32, usize)>(recorded_edit_key));
                        ui.ctx().memory_mut(|m| m.request_focus(te_id));
                        let mut st = egui::text_edit::TextEditState::default();
                        st.cursor.set_char_range(Some(egui::text::CCursorRange::one(
                            egui::text::CCursor::new(g.text.chars().count()),
                        )));
                        st.store(ui.ctx(), te_id);
                    }

                    let mut text_changed = false;
                    if let Some(ann) = self.annotations.get_mut(&page).and_then(|v| v.get_mut(idx))
                    {
                        let layout_font = g.font_id.clone();
                        let (col, italic) = (g.col, g.italic);
                        let mut layouter =
                            |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
                                let job = build_text_job(
                                    buf.as_str(),
                                    layout_font.clone(),
                                    col,
                                    italic,
                                    wrap_width,
                                    false,
                                );
                                ui.fonts_mut(|f| f.layout_job(job))
                            };

                        let mut child = ui.new_child(
                            egui::UiBuilder::new()
                                .max_rect(rect)
                                .layout(egui::Layout::top_down(egui::Align::Min)),
                        );

                        {
                            let v = &mut child.style_mut().visuals.widgets;
                            v.inactive.bg_stroke.width = 0.0;
                            v.hovered.bg_stroke.width = 0.0;
                            v.active.bg_stroke.width = 0.0;
                            v.open.bg_stroke.width = 0.0;
                            v.noninteractive.bg_stroke.width = 0.0;
                        }
                        child.spacing_mut().item_spacing = egui::Vec2::ZERO;

                        let te = egui::TextEdit::multiline(&mut ann.text)
                            .id(te_id)
                            .frame(egui::Frame::NONE.inner_margin(egui::Margin::same(g.pad_i8)))
                            .desired_width(g.box_w)
                            .desired_rows(1)
                            .lock_focus(true)
                            .layouter(&mut layouter)
                            .hint_text("Type here...");

                        let out = te.show(&mut child);
                        // Use the editor's current wrapped layout.
                        let min_height = (to_screen(g.p1).y - rect.min.y).max(ph * MIN_H);
                        let text_height = out.galley.size().y + 2.0 * g.pad;
                        rect.max.y = rect.min.y + min_height.max(text_height);

                        if need_init_cursor {
                            out.response.request_focus();
                            if need_init_cursor
                                && let Some(mut st) =
                                    egui::text_edit::TextEditState::load(ui.ctx(), te_id)
                            {
                                st.cursor.set_char_range(Some(egui::text::CCursorRange::one(
                                    egui::text::CCursor::new(ann.text.chars().count()),
                                )));
                                st.store(ui.ctx(), te_id);
                            }
                            ui.ctx().request_repaint();
                        }
                        text_changed = out.response.changed();
                        dirty |= text_changed;

                        // Draw an extra pass over the TextEdit galley to render bold text.
                        if g.bold && !ann.text.is_empty() {
                            draw_bold_galley(
                                ui.painter(),
                                out.galley_pos,
                                &out.galley,
                                g.col,
                                g.font_id.size,
                            );
                        }
                    }
                    if text_changed
                        && ui.data(|d| d.get_temp(recorded_edit_key)) != Some((page, idx))
                    {
                        // The creation snapshot already covers a new, empty box.
                        if !g.text.trim().is_empty() {
                            let mut before = self.annotations[&page].clone();
                            before[idx].text.clone_from(&g.text);
                            self.record_annotation_undo(page, before);
                        }
                        ui.data_mut(|d| d.insert_temp(recorded_edit_key, (page, idx)));
                    }
                }

                sel_rect = Some(rect);

                ui.painter().rect_stroke(
                    rect,
                    egui::CornerRadius::same(3),
                    egui::Stroke::new(1.5, accent),
                    egui::StrokeKind::Outside,
                );

                // ----- Maniglia di ridimensionamento -----
                let handle_rect =
                    egui::Rect::from_center_size(rect.right_bottom(), egui::vec2(18.0, 18.0));
                let rh = ui.interact(
                    handle_rect,
                    ui.make_persistent_id(("tb_resize", page, idx)),
                    egui::Sense::drag(),
                );
                if rh.hovered() || rh.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNwSe);
                }
                if rh.drag_started() {
                    self.push_undo_snapshot(page);
                }
                if rh.dragged() {
                    resize_to = rh.interact_pointer_pos();
                }
                ui.painter().rect_filled(
                    handle_rect.shrink(3.5),
                    egui::CornerRadius::same(2),
                    accent,
                );

                // Compact glass controls for the selected textbox.
                let mut t_toggle_bg = false;
                let bg_mode = g.bg_mode;
                let mut t_size: Option<f32> = None;
                let mut t_bold: Option<bool> = None;
                let mut t_italic: Option<bool> = None;
                let mut t_wrap_token: Option<&'static str> = None;
                let mut t_toggle_list = false;
                let mut t_color: Option<[u8; 3]> = None;
                let mut t_delete = false;
                let mut grip_started = false;
                let mut grip_delta = egui::Vec2::ZERO;
                let (fs, bold, italic, color) = (g.fs, g.bold, g.italic, g.color);

                let above = rect.top() - 46.0 >= ui.clip_rect().top();
                let (bar_pos, pivot) = if above {
                    (
                        egui::pos2(rect.left(), rect.top() - 8.0),
                        egui::Align2::LEFT_BOTTOM,
                    )
                } else {
                    (
                        egui::pos2(rect.left(), rect.bottom() + 8.0),
                        egui::Align2::LEFT_TOP,
                    )
                };

                let bar = egui::Area::new(ui.make_persistent_id(("tb_toolbar", page, idx)))
                    .fixed_pos(bar_pos)
                    .pivot(pivot)
                    .order(egui::Order::Foreground)
                    .constrain(true)
                    .show(ui.ctx(), |ui| {
                        egui::Frame::window(ui.style())
                            .shadow(egui::Shadow::NONE)
                            .fill(self.modal_glass_frame(MENU_GLASS_ALPHA).fill)
                            .stroke(egui::Stroke::new(1.2, accent))
                            .corner_radius(egui::CornerRadius::same(8))
                            .inner_margin(egui::Margin::symmetric(6, 4))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    let grip = ui
                                        .add(
                                            egui::Label::new(
                                                egui::RichText::new("󰆾").size(16.0).color(fg),
                                            )
                                            .sense(egui::Sense::drag()),
                                        )
                                        .on_hover_text("Drag to move")
                                        .on_hover_cursor(egui::CursorIcon::Grab);
                                    if grip.drag_started() {
                                        grip_started = true;
                                    }
                                    if grip.dragged() {
                                        grip_delta += grip.drag_delta();
                                    }

                                    ui.separator();

                                    let mut s = fs;
                                    if ui
                                        .add(
                                            egui::DragValue::new(&mut s)
                                                .range(8.0..=72.0)
                                                .speed(0.5)
                                                .suffix(" pt"),
                                        )
                                        .changed()
                                    {
                                        t_size = Some(s);
                                    }
                                    let format_response = ui
                                        .small_button("Aa")
                                        .on_hover_text("Text formatting")
                                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                                    egui::Popup::menu(&format_response)
                                        .id(ui.make_persistent_id(("tb_format_palette", page, idx)))
                                        .frame(
                                            self.modal_glass_frame(MENU_GLASS_ALPHA)
                                                .inner_margin(egui::Margin::symmetric(6, 4)),
                                        )
                                        .close_behavior(
                                            egui::PopupCloseBehavior::CloseOnClickOutside,
                                        )
                                        .show(|ui| {
                                            ui.horizontal(|ui| {
                                                if ui
                                                    .add(crate::theme::selected_button(
                                                        egui::RichText::new("B").strong(),
                                                        bold,
                                                        ui.visuals(),
                                                    ))
                                                    .on_hover_text("Bold")
                                                    .clicked()
                                                {
                                                    t_bold = Some(!bold);
                                                }
                                                if ui
                                                    .add(crate::theme::selected_button(
                                                        egui::RichText::new("I").italics(),
                                                        italic,
                                                        ui.visuals(),
                                                    ))
                                                    .on_hover_text("Italic")
                                                    .clicked()
                                                {
                                                    t_italic = Some(!italic);
                                                }
                                                if ui
                                                    .add(egui::Button::new(
                                                        egui::RichText::new("U").underline(),
                                                    ))
                                                    .on_hover_text("Underline (__text__)")
                                                    .clicked()
                                                {
                                                    t_wrap_token = Some("__");
                                                }
                                                if ui
                                                    .add(egui::Button::new(
                                                        egui::RichText::new("S").strikethrough(),
                                                    ))
                                                    .on_hover_text("Strikethrough (~~text~~)")
                                                    .clicked()
                                                {
                                                    t_wrap_token = Some("~~");
                                                }
                                                if ui
                                                    .add(egui::Button::new("List"))
                                                    .on_hover_text("Toggle bullet list (- )")
                                                    .clicked()
                                                {
                                                    t_toggle_list = true;
                                                }
                                            });
                                        });
                                    let bg_label = match bg_mode {
                                        1 => "󰝤 Bg: Dark",
                                        2 => "󰝤 Bg: Light",
                                        _ => "󰝣 Bg",
                                    };
                                    if ui
                                        .add(crate::theme::selected_button(
                                            bg_label,
                                            bg_mode != 0,
                                            ui.visuals(),
                                        ))
                                        .on_hover_text("Background: Off → Dark → Light")
                                        .clicked()
                                    {
                                        t_toggle_bg = true;
                                    }

                                    ui.separator();

                                    let (color_rect, color_response) = ui.allocate_exact_size(
                                        egui::vec2(24.0, 22.0),
                                        egui::Sense::click(),
                                    );
                                    ui.painter().circle_filled(
                                        color_rect.center(),
                                        8.5,
                                        rgb(color),
                                    );
                                    ui.painter().circle_stroke(
                                        color_rect.center(),
                                        9.5,
                                        egui::Stroke::new(
                                            if color_response.hovered() { 2.0 } else { 1.2 },
                                            fg,
                                        ),
                                    );
                                    let color_response = color_response
                                        .on_hover_text("Text color")
                                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                                    egui::Popup::menu(&color_response)
                                        .id(ui.make_persistent_id(("tb_color_palette", page, idx)))
                                        .frame(
                                            self.modal_glass_frame(MENU_GLASS_ALPHA)
                                                .inner_margin(egui::Margin::symmetric(6, 4)),
                                        )
                                        .show(|ui| {
                                            ui.horizontal(|ui| {
                                                for (pc, name) in ANNOTATION_PALETTE {
                                                    let (sw, response) = ui.allocate_exact_size(
                                                        egui::vec2(22.0, 22.0),
                                                        egui::Sense::click(),
                                                    );
                                                    ui.painter().circle_filled(
                                                        sw.center(),
                                                        8.5,
                                                        rgb(pc),
                                                    );
                                                    if pc == color || response.hovered() {
                                                        ui.painter().circle_stroke(
                                                            sw.center(),
                                                            10.0,
                                                            egui::Stroke::new(1.8, fg),
                                                        );
                                                    }
                                                    if response
                                                        .on_hover_text(name)
                                                        .on_hover_cursor(
                                                            egui::CursorIcon::PointingHand,
                                                        )
                                                        .clicked()
                                                    {
                                                        t_color = Some(pc);
                                                        ui.close();
                                                    }
                                                }
                                            });
                                        });

                                    ui.separator();
                                    if ui
                                        .small_button("󰆴")
                                        .on_hover_text("Delete text box [Del]")
                                        .clicked()
                                    {
                                        t_delete = true;
                                    }
                                    if ui
                                        .small_button("󰆏 ")
                                        .on_hover_text("Copy whole textbox")
                                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                                        .clicked()
                                    {
                                        if let Some(ann) =
                                            self.annotations.get(&page).and_then(|v| v.get(idx))
                                        {
                                            self.clipboard.set_text(
                                                ann.text
                                                    .trim_start_matches(['\u{200B}', '\u{200C}']),
                                            );
                                        }
                                    }
                                    if ui
                                        .add_sized(
                                            [48.0, 22.0],
                                            egui::Button::new(if editing {
                                                "Done"
                                            } else {
                                                "Edit"
                                            }),
                                        )
                                        .on_hover_text("Edit text or finish editing (Esc)")
                                        .clicked()
                                    {
                                        sel = Some((page, idx, !editing));
                                    }
                                });
                            });
                    });
                toolbar_rect = Some(bar.response.rect);

                if grip_started {
                    self.push_undo_snapshot(page);
                }
                move_delta += grip_delta;

                if t_delete {
                    self.push_undo_snapshot(page);
                    if let Some(list) = self.annotations.get_mut(&page)
                        && idx < list.len()
                    {
                        list.remove(idx);
                    }
                    sel = None;
                    dirty = true;
                    break;
                }

                if t_size.is_some()
                    || t_bold.is_some()
                    || t_italic.is_some()
                    || t_color.is_some()
                    || t_wrap_token.is_some()
                    || t_toggle_list
                    || t_toggle_bg
                {
                    self.push_undo_snapshot(page);
                    if let Some(color) = t_color
                        && self
                            .annotations
                            .get(&page)
                            .and_then(|v| v.get(idx))
                            .is_some()
                    {
                        self.remember_annotation_color(annotations::AnnotationKind::TextBox, color);
                    }
                    if let Some(ann) = self.annotations.get_mut(&page).and_then(|v| v.get_mut(idx))
                    {
                        if let Some(s) = t_size {
                            ann.font_size = s;
                            self.textbox_font_size = s;
                        }
                        if let Some(b) = t_bold {
                            ann.bold = b;
                            self.textbox_bold = b;
                        }
                        if let Some(i) = t_italic {
                            ann.italic = i;
                            self.textbox_italic = i;
                        }
                        if let Some(c) = t_color {
                            ann.color = c;
                        }
                        if let Some(tok) = t_wrap_token {
                            if ann.text.starts_with(tok)
                                && ann.text.ends_with(tok)
                                && ann.text.len() >= tok.len() * 2
                            {
                                ann.text =
                                    ann.text[tok.len()..ann.text.len() - tok.len()].to_string();
                            } else {
                                ann.text = format!("{tok}{}{tok}", ann.text);
                            }
                        }
                        if t_toggle_list {
                            let all_bullets = ann
                                .text
                                .lines()
                                .filter(|l| !l.trim().is_empty())
                                .all(|l| l.starts_with("- "));
                            ann.text = ann
                                .text
                                .split('\n')
                                .map(|l| {
                                    if all_bullets {
                                        l.strip_prefix("- ").unwrap_or(l).to_string()
                                    } else if l.starts_with("- ") {
                                        l.to_string()
                                    } else {
                                        format!("- {l}")
                                    }
                                })
                                .collect::<Vec<_>>()
                                .join("\n");
                        }
                        if t_toggle_bg {
                            if ann.text.starts_with('\u{200B}') {
                                // Cycle from dark to light.
                                ann.text.remove(0);
                                ann.text.insert(0, '\u{200C}');
                            } else if ann.text.starts_with('\u{200C}') {
                                // Cycle from light to no background.
                                ann.text.remove(0);
                            } else {
                                // Cycle from no background to dark.
                                ann.text.insert(0, '\u{200B}');
                            }
                        }
                    }
                    dirty = true;
                }
            }

            // Apply movement or resizing.
            if move_delta != egui::Vec2::ZERO || resize_to.is_some() {
                let (p0, p1) = (g.p0, g.p1);
                let mut np0 = p0;
                let mut np1 = p1;

                if move_delta != egui::Vec2::ZERO {
                    let w = p1.0 - p0.0;
                    let h = p1.1 - p0.1;
                    np0.0 = (p0.0 + move_delta.x / pw).clamp(0.0, (1.0 - w).max(0.0));
                    np0.1 = (p0.1 + move_delta.y / ph).clamp(0.0, (1.0 - h).max(0.0));
                    np1 = (np0.0 + w, np0.1 + h);
                }
                if let Some(pos) = resize_to {
                    let nx = ((pos.x - page_rect.left()) / pw).clamp(0.0, 1.0);
                    let ny = ((pos.y - page_rect.top()) / ph).clamp(0.0, 1.0);
                    np1 = (nx.max(np0.0 + MIN_W), ny.max(np0.1 + MIN_H));
                }

                if let Some(ann) = self.annotations.get_mut(&page).and_then(|v| v.get_mut(idx)) {
                    ann.points = vec![np0, np1];
                }
                dirty = true;
            }
        }

        // 4. Escape, Delete, and outside clicks.
        if !egui::Popup::is_any_open(ui.ctx())
            && !just_created_this_frame
            && let Some((sp, si, ed)) = sel
            && sp == page
        {
            let (esc, del, pressed) = ui.input(|i| {
                (
                    i.key_pressed(egui::Key::Escape),
                    i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
                    if i.pointer.primary_pressed() {
                        i.pointer.interact_pos()
                    } else {
                        None
                    },
                )
            });

            if esc {
                sel = if ed { Some((sp, si, false)) } else { None };
            } else if del && !ed && !ui.ctx().egui_wants_keyboard_input() {
                self.push_undo_snapshot(page);
                if let Some(list) = self.annotations.get_mut(&page)
                    && si < list.len()
                {
                    list.remove(si);
                }
                sel = None;
                dirty = true;
            } else if let Some(p) = pressed {
                let on_sel = sel_rect.is_some_and(|r| r.expand(RING + 2.0).contains(p));
                let on_bar = toolbar_rect.is_some_and(|r| r.expand(4.0).contains(p));
                let on_any = geos.iter().any(|g| g.hit.contains(p));
                if !on_sel && !on_bar && !on_any {
                    sel = None;
                }
            }
        }

        // 5. Save state.
        ui.data_mut(|d| match sel {
            Some(s) => {
                d.insert_temp(sel_key, s);
            }
            None => {
                d.remove::<(u32, usize, bool)>(sel_key);
            }
        });

        let prev_edit: Option<(u32, usize)> = ui.data(|d| d.get_temp(edit_prev_key));
        match sel {
            Some((sp, si, true)) if sp == page => {
                ui.data_mut(|d| d.insert_temp(edit_prev_key, (sp, si)));
            }
            _ => {
                if prev_edit.map(|p| p.0) == Some(page) {
                    ui.data_mut(|d| d.remove::<(u32, usize)>(edit_prev_key));
                }
            }
        }

        if page == self.current_page {
            self.selected_textbox = sel.filter(|s| s.0 == page).map(|s| s.1);
        }

        // Check whether the pointer is over a block or its toolbar.
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let over = pointer.is_some_and(|p| {
            geos.iter().any(|g| g.hit.contains(p))
                || sel_rect.is_some_and(|r| r.expand(RING).contains(p))
                || toolbar_rect.is_some_and(|r| r.contains(p))
        });
        let over_id = egui::Id::NULL.with(("pointer_over_textbox", page));
        ui.data_mut(|d| d.insert_temp(over_id, over));

        if dirty {
            self.save_study_edit();
        }
    }

    pub(super) fn annotation_cursor_icon(&self) -> &'static str {
        match self.annotation_tool {
            AnnotationTool::Cursor => "󰇀",
            AnnotationTool::Draw => "󱦹",
            AnnotationTool::Highlight => "",
            AnnotationTool::Underline => "󰊇",
            AnnotationTool::TextBox => "󰊄",
            AnnotationTool::Eraser => "󰇾",
            AnnotationTool::Tape => "󰝤",
            AnnotationTool::Snip => "󰆞",
            AnnotationTool::Laser => "󰐾",
            AnnotationTool::Shape => match self.shape_kind {
                ShapeKind::Rect => "󰝣",
                ShapeKind::Circle => "󰝥",
                ShapeKind::Line => "󰕞",
                ShapeKind::Arrow => "󰁔",
            },
        }
    }

    pub(super) fn paint_annotation_cursor(&self, ctx: &egui::Context) {
        if self.annotation_tool == AnnotationTool::Cursor
            || self.pdf_panning
            || self.show_settings
            || self.radial_menu_pos.is_some()
            || ctx.output(|o| o.cursor_icon) != egui::CursorIcon::Crosshair
        {
            return;
        }
        let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else {
            return;
        };
        let Some((layer, clip)) = ctx.data(|d| {
            d.get_temp::<(egui::LayerId, egui::Rect)>(egui::Id::unique("annotation_cursor_target"))
        }) else {
            return;
        };
        if !clip.contains(pos) || ctx.layer_id_at(pos) != Some(layer) {
            return;
        }
        if self.annotation_tool == AnnotationTool::Eraser {
            ctx.set_cursor_icon(egui::CursorIcon::None);
            return;
        }
        ctx.set_cursor_icon(egui::CursorIcon::None);
        let painter = ctx.layer_painter(layer).with_clip_rect(ctx.content_rect());
        let color = if self.annotation_tool.color_slot().is_some() {
            rgb(self.annotation_color())
        } else {
            egui::Color32::WHITE
        };
        // Snap the painted cursor, not the annotation coordinates.
        let ppp = ctx.pixels_per_point();
        let pos = egui::pos2((pos.x * ppp).round() / ppp, (pos.y * ppp).round() / ppp);
        if self.annotation_tool == AnnotationTool::Draw {
            painter.circle_stroke(pos, 3.0, egui::Stroke::new(1.5, egui::Color32::BLACK));
            painter.circle_stroke(pos, 3.0, egui::Stroke::new(0.75, color));
            return;
        }
        let line = |a: (f32, f32), b: (f32, f32)| {
            let points = [pos + egui::vec2(a.0, a.1), pos + egui::vec2(b.0, b.1)];
            painter.line_segment(points, egui::Stroke::new(1.5, egui::Color32::BLACK));
            painter.line_segment(points, egui::Stroke::new(0.75, color));
        };
        if self.annotation_tool == AnnotationTool::Highlight {
            let corners = [(-7.0, 3.0), (3.0, -7.0), (7.0, -3.0), (-3.0, 7.0)];
            let points = corners
                .iter()
                .map(|&(x, y)| pos + egui::vec2(x, y))
                .collect();
            painter.add(egui::Shape::convex_polygon(
                points,
                color,
                egui::Stroke::new(0.9, egui::Color32::BLACK),
            ));
            return;
        }
        line((-5.0, 0.0), (5.0, 0.0));
        line((0.0, -5.0), (0.0, 5.0));
        // Keep the glyph at a fixed offset instead of flipping at screen edges.
        painter.text(
            pos + egui::vec2(12.0, 12.0),
            egui::Align2::CENTER_CENTER,
            self.annotation_cursor_icon(),
            egui::FontId::proportional(13.0),
            color,
        );
    }

    pub(super) fn handle_slide_annotations(
        &mut self,
        ui: &mut egui::Ui,
        page_num: u32,
        slide_rect: egui::Rect,
        img_resp: &egui::Response,
    ) {
        if self.pdf_panning {
            return;
        }
        if img_resp.clicked() || img_resp.drag_started() {
            self.sketch.clear_focus();
        }
        // Screenshots are painted first so that text boxes (drawn right after) always end up on top.
        self.image_annotations_ui(ui, page_num, slide_rect);
        self.render_page_textboxes(ui, page_num, slide_rect);
        self.slide_rects.push((page_num, slide_rect));

        let to_norm = |pos: egui::Pos2| -> (f32, f32) {
            (
                ((pos.x - slide_rect.left()) / slide_rect.width()).clamp(0.0, 1.0),
                ((pos.y - slide_rect.top()) / slide_rect.height()).clamp(0.0, 1.0),
            )
        };

        // A PDF text selection owns right-click; never open the radial tool menu over it.
        let selected_pdf_text = self.selection_text();
        if selected_pdf_text.is_some() {
            self.radial_menu_pos = None;
            let glass_style = self.menu_glass().modifier();
            egui::Popup::context_menu(img_resp)
                .style(glass_style)
                .show(|ui| {
                    if ui.button("󰆏  Copy").clicked() {
                        if let Some(text) = &selected_pdf_text {
                            self.clipboard.set_text(text);
                        }
                        ui.close();
                    }
                });
        } else if self.app_state.settings.radial_menu_enabled && img_resp.secondary_clicked() {
            self.radial_clipboard_image_available = screenshots::clipboard_has_image();
            self.radial_menu_pos = ui.input(|i| i.pointer.hover_pos());
        }

        let pointer_pos = ui.input(|i| i.pointer.hover_pos().or_else(|| i.pointer.interact_pos()));
        // Let the PDF set the cursor only when it is the topmost layer.
        // Overlays must keep their own cursor.

        let pointer_on_pdf_layer = pointer_pos
            .map(|pos| ui.ctx().layer_id_at(pos) == Some(img_resp.layer_id))
            .unwrap_or(false);
        if (img_resp.hovered() || img_resp.dragged()) && pointer_on_pdf_layer {
            let icon = match self.annotation_tool {
                AnnotationTool::Cursor => egui::CursorIcon::Default,
                _ => egui::CursorIcon::Crosshair,
            };
            ui.ctx().set_cursor_icon(icon);
        }

        let pointer_in_slide = pointer_pos
            .map(|p| slide_rect.contains(p) && pointer_on_pdf_layer)
            .unwrap_or(false);
        if pointer_in_slide && pointer_pos.is_some_and(|p| ui.clip_rect().contains(p)) {
            ui.ctx().data_mut(|d| {
                d.insert_temp(
                    egui::Id::unique("annotation_cursor_target"),
                    (img_resp.layer_id, slide_rect.intersect(ui.clip_rect())),
                )
            });
        }
        let any_text_focused = ui.ctx().memory(|m| m.focused().is_some());

        let hold_key_down = !any_text_focused
            && pointer_in_slide
            && ui.input(|i| self.app_state.shortcuts.trackpad_draw_hold.is_held(i));

        let hold_just_started = hold_key_down && !self.trackpad_drawing_active;
        let hold_just_released = !hold_key_down && self.trackpad_drawing_active;
        if pointer_in_slide || hold_just_released {
            self.trackpad_drawing_active = hold_key_down;
        }

        match self.annotation_tool {
            AnnotationTool::Cursor => {
                let mut hovered_link: Option<PdfLinkZone> = None;
                let mut over_text = false;

                if let Some(pos) = pointer_pos
                    && slide_rect.contains(pos)
                    && pointer_on_pdf_layer
                {
                    let (nx, ny) = to_norm(pos);

                    if let Some(links) = self.viewer.links(page_num) {
                        hovered_link = links
                            .iter()
                            .find(|l| {
                                nx >= l.rect[0]
                                    && nx <= l.rect[2]
                                    && ny >= l.rect[1]
                                    && ny <= l.rect[3]
                            })
                            .cloned();
                    }

                    if hovered_link.is_none()
                        && let Some(text) = self.viewer.text(page_num)
                    {
                        over_text = text.is_over_text(nx, ny);
                    }
                }

                if let Some(ref link) = hovered_link {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    if img_resp.clicked() {
                        if let Some(tp) = link.target_page {
                            self.go_to_page(tp, ui.ctx());
                        } else if let Some(ref url) = link.external_uri {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                        }
                        return;
                    }
                } else if pointer_on_pdf_layer && (over_text || self.selecting) {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
                }

                if img_resp.drag_started() {
                    let origin = ui
                        .input(|i| i.pointer.press_origin())
                        .or(img_resp.interact_pointer_pos());
                    if let Some(pos) = origin {
                        let (nx, ny) = to_norm(pos);
                        if let Some(text) = self.viewer.text(page_num) {
                            let c = TextCaret {
                                page: page_num,
                                idx: text.caret_at(nx, ny),
                            };
                            self.pdf_selection = Some(TextSelection {
                                anchor: c,
                                focus: c,
                            });
                            self.selecting = true;
                        }
                    }
                } else if img_resp.triple_clicked() || img_resp.double_clicked() {
                    if let Some(pos) = img_resp.interact_pointer_pos() {
                        let (nx, ny) = to_norm(pos);
                        let line = img_resp.triple_clicked();
                        let range = self.viewer.text(page_num).and_then(|t| {
                            let ci = t.char_at(nx, ny)?;
                            Some(if line {
                                t.line_range(ci)
                            } else {
                                t.word_range(ci)
                            })
                        });
                        if let Some((a, b)) = range {
                            self.pdf_selection = Some(TextSelection {
                                anchor: TextCaret {
                                    page: page_num,
                                    idx: a,
                                },
                                focus: TextCaret {
                                    page: page_num,
                                    idx: b,
                                },
                            });
                        }
                    }
                } else if img_resp.clicked() {
                    self.pdf_selection = None;
                }
            }
            AnnotationTool::Eraser => {
                if let Some(pos) = pointer_pos {
                    if pointer_in_slide && ui.clip_rect().contains(pos) {
                        // Halfway between the old fixed 7 px and the real reach, so the circle
                        // follows the page zoom and still stays small enough to aim with.
                        let reach_px =
                            ERASER_REACH * (slide_rect.width() + slide_rect.height()) * 0.5;
                        ui.painter().circle_stroke(
                            pos,
                            (7.0 + reach_px) * 0.5,
                            egui::Stroke::new(1.0, egui::Color32::from_black_alpha(140)),
                        );
                    }
                    if img_resp.clicked() || img_resp.dragged() || hold_key_down {
                        let (nx, ny) = to_norm(pos);
                        let before_state =
                            self.annotations.get(&page_num).cloned().unwrap_or_default();
                        let before_len = before_state.len();
                        let mut changed = false;

                        if let Some(list) = self.annotations.get_mut(&page_num) {
                            list.retain(|ann| match ann.kind {
                                annotations::AnnotationKind::Highlight
                                | annotations::AnnotationKind::Rect
                                | annotations::AnnotationKind::Circle
                                | annotations::AnnotationKind::Line
                                | annotations::AnnotationKind::Arrow
                                | annotations::AnnotationKind::Tape => {
                                    if ann.points.len() >= 2 {
                                        let (x1, y1) = ann.points[0];
                                        let (x2, y2) = *ann.points.last().unwrap();
                                        !(nx >= x1.min(x2) - 0.015
                                            && nx <= x1.max(x2) + 0.015
                                            && ny >= y1.min(y2) - 0.015
                                            && ny <= y1.max(y2) + 0.015)
                                    } else {
                                        true
                                    }
                                }
                                annotations::AnnotationKind::Underline => {
                                    if ann.points.len() >= 2 {
                                        let (x1, y1) = ann.points[0];
                                        let (x2, _) = *ann.points.last().unwrap();
                                        !(nx >= x1.min(x2) - 0.015
                                            && nx <= x1.max(x2) + 0.015
                                            && (ny - y1).abs() < 0.02)
                                    } else {
                                        true
                                    }
                                }
                                annotations::AnnotationKind::Freehand => !ann
                                    .points
                                    .iter()
                                    .any(|&(px, py)| (px - nx).hypot(py - ny) < ERASER_REACH),
                                annotations::AnnotationKind::TextBox => {
                                    if let Some(&(bx, by)) = ann.points.first() {
                                        let (ex, ey) = ann
                                            .points
                                            .get(1)
                                            .copied()
                                            .unwrap_or((bx + 0.16, by + 0.04));
                                        !(nx >= bx - 0.01
                                            && nx <= ex + 0.01
                                            && ny >= by - 0.01
                                            && ny <= ey + 0.01)
                                    } else {
                                        true
                                    }
                                }
                                annotations::AnnotationKind::Image => true, // The eraser skips images.
                            });
                            changed = list.len() != before_len;
                        }

                        if changed {
                            self.record_annotation_undo(page_num, before_state);
                            self.selected_textbox = None;

                            self.save_study_edit();
                        }
                    }
                }
            }
            AnnotationTool::TextBox => {}
            AnnotationTool::Laser => {
                if (img_resp.dragged() || hold_key_down)
                    && let Some(pos) = pointer_pos
                {
                    self.laser_trail
                        .push((to_norm(pos), std::time::Instant::now()));
                    ui.ctx().request_repaint();
                }
            }
            AnnotationTool::Snip => {
                let started = img_resp.drag_started() || hold_just_started;
                let active_move = img_resp.dragged() || hold_key_down;
                let stopped = img_resp.drag_stopped() || hold_just_released;

                if started {
                    let press_pos = if hold_just_started {
                        pointer_pos
                    } else {
                        ui.input(|i| i.pointer.press_origin())
                            .or(img_resp.interact_pointer_pos())
                            .or(pointer_pos)
                    };
                    if let Some(pos) = press_pos {
                        let pt = to_norm(pos);
                        self.active_snip_rect = Some((page_num, pt, pt));
                    }
                } else if active_move {
                    if let (Some(pos), Some(snip)) = (pointer_pos, self.active_snip_rect.as_mut())
                        && snip.0 == page_num
                    {
                        snip.2 = to_norm(pos);
                    }
                } else if stopped
                    && let Some((sp, p1, p2)) = self.active_snip_rect.take()
                    && sp == page_num
                {
                    match crop_slide_region_to_png(
                        &self.pdf_path,
                        page_num,
                        p1,
                        p2,
                        self.pdf_dark_mode,
                    ) {
                        Ok(png_path) => {
                            let md_link = format!(
                                "\n![Snip p.{}]({})\n",
                                page_num,
                                paths::file_uri(&png_path)
                            );
                            self.current_note.push_str(&md_link);
                            self.sync_and_save();
                            self.show_notes = true;
                            self.toast_message = Some((
                                format!("󰆞 Snip saved to page {} notes", self.note_page),
                                std::time::Instant::now(),
                            ));
                        }
                        Err(e) => {
                            self.toast_message =
                                Some((format!("󰅚 Snip failed: {}", e), std::time::Instant::now()));
                        }
                    }
                }
            }
            tool => {
                let kind = match tool {
                    AnnotationTool::Highlight => annotations::AnnotationKind::Highlight,
                    AnnotationTool::Underline => annotations::AnnotationKind::Underline,
                    AnnotationTool::Tape => annotations::AnnotationKind::Tape,
                    AnnotationTool::Shape => match self.shape_kind {
                        ShapeKind::Rect => annotations::AnnotationKind::Rect,
                        ShapeKind::Circle => annotations::AnnotationKind::Circle,
                        ShapeKind::Line => annotations::AnnotationKind::Line,
                        ShapeKind::Arrow => annotations::AnnotationKind::Arrow,
                    },
                    _ => annotations::AnnotationKind::Freehand,
                };

                let started = img_resp.drag_started() || hold_just_started;
                let active_move = img_resp.dragged() || hold_key_down;
                let stopped = img_resp.drag_stopped() || hold_just_released;

                if started {
                    let press_pos = if hold_just_started {
                        pointer_pos
                    } else {
                        ui.input(|i| i.pointer.press_origin())
                            .or(img_resp.interact_pointer_pos())
                            .or(pointer_pos)
                    };
                    if let Some(pos) = press_pos {
                        let pt = to_norm(pos);
                        self.active_stroke = Some(annotations::PageAnnotation {
                            kind,
                            color: self.annotation_color(),
                            points: vec![pt, pt],
                            text: String::new(),
                            font_size: self.textbox_font_size,
                            bold: false,
                            italic: false,
                            stroke_width: self.stroke_width,
                            native_source: None,
                        });
                    }
                } else if active_move {
                    if let (Some(pos), Some(stroke)) = (pointer_pos, &mut self.active_stroke) {
                        let pt = to_norm(pos);
                        match stroke.kind {
                            annotations::AnnotationKind::Freehand => {
                                if stroke.points.last() != Some(&pt) {
                                    stroke.points.push(pt);
                                }
                            }
                            _ => {
                                if stroke.points.len() >= 2 {
                                    *stroke.points.last_mut().unwrap() = pt;
                                }
                            }
                        }
                    }
                } else if stopped && let Some(mut stroke) = self.active_stroke.take() {
                    if stroke.kind == annotations::AnnotationKind::Freehand {
                        stroke.points = annotations::smooth_freehand_points(&stroke.points);
                    }
                    self.push_undo_snapshot(page_num);
                    self.commit_annotation_color(stroke.kind, stroke.color);
                    self.annotations.entry(page_num).or_default().push(stroke);
                    self.save_study_edit();
                }
            }
        }

        // Click on Occlusion Tapes to peek / reveal them
        if img_resp.clicked()
            && self.annotation_tool != AnnotationTool::Eraser
            && let Some(pos) = pointer_pos
        {
            let (nx, ny) = to_norm(pos);
            if let Some(list) = self.annotations.get(&page_num) {
                for (idx, ann) in list.iter().enumerate() {
                    if ann.kind == annotations::AnnotationKind::Tape && ann.points.len() >= 2 {
                        let (x1, y1) = ann.points[0];
                        let (x2, y2) = *ann.points.last().unwrap();
                        if nx >= x1.min(x2)
                            && nx <= x1.max(x2)
                            && ny >= y1.min(y2)
                            && ny <= y1.max(y2)
                        {
                            let key = (page_num, idx);
                            if !self.revealed_tapes.remove(&key) {
                                self.revealed_tapes.insert(key);
                            }
                            return;
                        }
                    }
                }
            }
        }
        // Paint saved annotations + active stroke + Snip rect + Laser trail + Text selection
        let painter = ui.painter_at(slide_rect);
        if let Some(page_anns) = self.annotations.get(&page_num) {
            for (idx, ann) in page_anns.iter().enumerate() {
                if ann.kind == annotations::AnnotationKind::Tape
                    && self.revealed_tapes.contains(&(page_num, idx))
                {
                    if ann.points.len() >= 2 {
                        let p1 = egui::pos2(
                            slide_rect.left() + ann.points[0].0 * slide_rect.width(),
                            slide_rect.top() + ann.points[0].1 * slide_rect.height(),
                        );
                        let last = *ann.points.last().unwrap();
                        let p2 = egui::pos2(
                            slide_rect.left() + last.0 * slide_rect.width(),
                            slide_rect.top() + last.1 * slide_rect.height(),
                        );
                        let r = egui::Rect::from_two_pos(p1, p2);
                        painter.rect_stroke(
                            r,
                            egui::CornerRadius::same(4),
                            egui::Stroke::new(
                                1.5,
                                egui::Color32::from_rgba_unmultiplied(
                                    ann.color[0],
                                    ann.color[1],
                                    ann.color[2],
                                    90,
                                ),
                            ),
                            egui::StrokeKind::Inside,
                        );
                    }
                } else {
                    Self::paint_annotation(&painter, slide_rect, ann);
                }
            }
        }

        if let Some(active) = &self.active_stroke
            && (img_resp.dragged() || self.trackpad_drawing_active)
        {
            Self::paint_annotation(&painter, slide_rect, active);
        }

        if let Some((snip_page, p1, p2)) = self.active_snip_rect
            && snip_page == page_num
        {
            let r = egui::Rect::from_two_pos(
                egui::pos2(
                    slide_rect.left() + p1.0 * slide_rect.width(),
                    slide_rect.top() + p1.1 * slide_rect.height(),
                ),
                egui::pos2(
                    slide_rect.left() + p2.0 * slide_rect.width(),
                    slide_rect.top() + p2.1 * slide_rect.height(),
                ),
            );
            painter.rect_filled(
                r,
                egui::CornerRadius::same(3),
                egui::Color32::from_rgba_unmultiplied(125, 156, 255, 45),
            );
            painter.rect_stroke(
                r,
                egui::CornerRadius::same(3),
                egui::Stroke::new(1.8, rgb(self.theme.accent)),
                egui::StrokeKind::Inside,
            );
        }

        let now = std::time::Instant::now();
        self.laser_trail
            .retain(|&(_, t)| now.duration_since(t).as_secs_f32() < 1.2);
        if !self.laser_trail.is_empty() {
            for win in self.laser_trail.windows(2) {
                let ((x0, y0), _) = win[0];
                let ((x1, y1), t1) = win[1];
                let age = now.duration_since(t1).as_secs_f32();
                let alpha = ((1.0 - (age / 1.2)).clamp(0.0, 1.0) * 240.0) as u8;
                let p0 = egui::pos2(
                    slide_rect.left() + x0 * slide_rect.width(),
                    slide_rect.top() + y0 * slide_rect.height(),
                );
                let p1 = egui::pos2(
                    slide_rect.left() + x1 * slide_rect.width(),
                    slide_rect.top() + y1 * slide_rect.height(),
                );
                painter.line_segment(
                    [p0, p1],
                    egui::Stroke::new(
                        5.0,
                        egui::Color32::from_rgba_unmultiplied(255, 45, 55, alpha / 2),
                    ),
                );
                painter.line_segment(
                    [p0, p1],
                    egui::Stroke::new(
                        2.2,
                        egui::Color32::from_rgba_unmultiplied(255, 220, 220, alpha),
                    ),
                );
            }
            if let Some(&((lx, ly), _)) = self.laser_trail.last() {
                let head = egui::pos2(
                    slide_rect.left() + lx * slide_rect.width(),
                    slide_rect.top() + ly * slide_rect.height(),
                );
                painter.circle_filled(head, 5.5, egui::Color32::from_rgb(255, 40, 50));
                painter.circle_filled(head, 2.5, egui::Color32::WHITE);
            }
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(16));
        }

        self.paint_text_selection(&painter, page_num, slide_rect);
        self.paint_search_highlight(&painter, page_num, slide_rect);
    }

    pub(super) fn push_undo_snapshot(&mut self, page: u32) {
        let current_state = self.annotations.get(&page).cloned().unwrap_or_default();
        self.record_annotation_undo(page, current_state);
    }

    fn record_annotation_undo(&mut self, page: u32, current_state: Vec<PageAnnotation>) {
        self.sketch.clear_focus();
        let stack = self.undo_stack.entry(page).or_default();
        // Cap history to 40 steps per page to keep RAM negligible
        if stack.len() >= 40 {
            stack.remove(0);
        }
        stack.push(current_state);
        // Any new drawing action invalidates the redo branch for that page
        self.redo_stack.remove(&page);
    }

    pub(super) fn can_undo_annotation(&self, page: u32) -> bool {
        self.undo_stack
            .get(&page)
            .is_some_and(|stack| !stack.is_empty())
    }

    pub(super) fn undo_annotation(&mut self, page: u32) {
        if let Some(stack) = self.undo_stack.get_mut(&page)
            && let Some(prev_state) = stack.pop()
        {
            let current_state = self.annotations.get(&page).cloned().unwrap_or_default();
            self.redo_stack.entry(page).or_default().push(current_state);
            if prev_state.is_empty() {
                self.annotations.remove(&page);
            } else {
                self.annotations.insert(page, prev_state);
            }
            self.selected_textbox = None;
            self.textbox_epoch = self.textbox_epoch.wrapping_add(1);
            self.sketch.clear_focus();
            self.save_study_edit();
            self.toast_message = Some(("󰕌 Undo annotation".to_string(), std::time::Instant::now()));
        }
    }

    pub(super) fn can_redo_annotation(&self, page: u32) -> bool {
        self.redo_stack
            .get(&page)
            .is_some_and(|stack| !stack.is_empty())
    }

    pub(super) fn redo_annotation(&mut self, page: u32) {
        if let Some(stack) = self.redo_stack.get_mut(&page)
            && let Some(next_state) = stack.pop()
        {
            let current_state = self.annotations.get(&page).cloned().unwrap_or_default();
            self.undo_stack.entry(page).or_default().push(current_state);
            self.annotations.insert(page, next_state);
            self.selected_textbox = None;
            self.textbox_epoch = self.textbox_epoch.wrapping_add(1);
            self.sketch.clear_focus();
            self.save_study_edit();
            self.toast_message = Some(("󰑎 Redo annotation".to_string(), std::time::Instant::now()));
        }
    }
    /// Draw page images and handle selection, movement, fixed-aspect resizing, and deletion.
    /// Interactions are enabled only with the Cursor tool so other tools
    /// can draw over the image.
    pub(super) fn image_annotations_ui(
        &mut self,
        ui: &mut egui::Ui,
        page_num: u32,
        slide_rect: egui::Rect,
    ) {
        let img_idx: Vec<usize> = match self.annotations.get(&page_num) {
            Some(list) => list
                .iter()
                .enumerate()
                .filter(|(_, a)| {
                    a.kind == annotations::AnnotationKind::Image && a.points.len() >= 2
                })
                .map(|(i, _)| i)
                .collect(),
            None => return,
        };
        if img_idx.is_empty() {
            return;
        }
        let cursor_tool = matches!(self.annotation_tool, AnnotationTool::Cursor);
        if !cursor_tool && matches!(self.selected_image, Some((p, _)) if p == page_num) {
            self.selected_image = None;
        }
        if let Some((p, i)) = self.selected_image
            && p == page_num
            && !img_idx.contains(&i)
        {
            self.selected_image = None;
        }

        let dir = audio::AudioDocIndex::media_dir_for_pdf(&self.pdf_path).join("screenshots");
        let (sw, sh) = (slide_rect.width().max(1.0), slide_rect.height().max(1.0));
        let to_screen = |(nx, ny): (f32, f32)| {
            egui::pos2(slide_rect.left() + nx * sw, slide_rect.top() + ny * sh)
        };
        let to_norm = |p: egui::Pos2| {
            (
                (p.x - slide_rect.left()) / sw,
                (p.y - slide_rect.top()) / sh,
            )
        };

        let mut hovered_any = false;
        let mut new_drag: Option<ImageDrag> = None;
        let mut cursor_icon: Option<egui::CursorIcon> = None;

        // Draw each image and its hit area.
        for &i in &img_idx {
            let (p0, p1, name) = {
                let a = &self.annotations[&page_num][i];
                (a.points[0], a.points[1], a.text.clone())
            };
            let rect = egui::Rect::from_two_pos(to_screen(p0), to_screen(p1));
            let uri = paths::file_uri(&dir.join(&name));
            egui::Image::new(uri).paint_at(ui, rect);

            if !cursor_tool {
                continue;
            }
            let resp = ui.interact(
                rect,
                ui.make_persistent_id(("img_ann", page_num, i)),
                egui::Sense::click_and_drag(),
            );
            if resp.hovered() || resp.dragged() {
                hovered_any = true;
                cursor_icon = Some(if resp.dragged() {
                    egui::CursorIcon::Grabbing
                } else {
                    egui::CursorIcon::Grab
                });
            }
            if resp.clicked() || resp.drag_started() {
                self.selected_image = Some((page_num, i));
            }
            if resp.drag_started() {
                // Start dragging from the exact press position.
                let origin = ui
                    .input(|i| i.pointer.press_origin())
                    .or(resp.interact_pointer_pos());
                if let Some(pos) = origin {
                    let (px, py) = to_norm(pos);
                    new_drag = Some(ImageDrag {
                        page: page_num,
                        idx: i,
                        mode: ImageDragMode::Move {
                            grab: (px - p0.0.min(p1.0), py - p0.1.min(p1.1)),
                        },
                    });
                }
            }
        }

        // Selected image border and corner handles.
        let mut delete_clicked = false;
        if let Some((sp, si)) = self.selected_image
            && cursor_tool
            && sp == page_num
        {
            let (p0, p1) = {
                let a = &self.annotations[&page_num][si];
                (a.points[0], a.points[1])
            };
            let (x0, x1) = (p0.0.min(p1.0), p0.0.max(p1.0));
            let (y0, y1) = (p0.1.min(p1.1), p0.1.max(p1.1));
            let rect = egui::Rect::from_two_pos(to_screen((x0, y0)), to_screen((x1, y1)));
            ui.painter().rect_stroke(
                rect,
                egui::CornerRadius::ZERO,
                egui::Stroke::new(1.5_f32, rgb(self.theme.accent)),
                egui::StrokeKind::Outside,
            );

            // Corners are TL, TR, BR, BL; the opposite corner is (k + 2) % 4.
            let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
            for (k, &c) in corners.iter().enumerate() {
                let pos = to_screen(c);
                let hrect = egui::Rect::from_center_size(pos, egui::vec2(12.0, 12.0));
                let resp = ui.interact(
                    hrect,
                    ui.make_persistent_id(("img_ann_handle", page_num, si, k)),
                    egui::Sense::drag(),
                );
                ui.painter().rect_filled(
                    hrect,
                    egui::CornerRadius::same(2),
                    rgb(self.theme.accent),
                );
                ui.painter().rect_stroke(
                    hrect,
                    egui::CornerRadius::same(2),
                    egui::Stroke::new(1.0_f32, egui::Color32::WHITE),
                    egui::StrokeKind::Inside,
                );
                if resp.hovered() || resp.dragged() {
                    hovered_any = true;
                    cursor_icon = Some(if k % 2 == 0 {
                        egui::CursorIcon::ResizeNwSe
                    } else {
                        egui::CursorIcon::ResizeNeSw
                    });
                }
                if resp.drag_started() {
                    new_drag = Some(ImageDrag {
                        page: page_num,
                        idx: si,
                        mode: ImageDragMode::Resize {
                            anchor: corners[(k + 2) % 4],
                        },
                    });
                }
            }

            // Compact floating delete button in the top-right corner.
            let pad = 6.0;
            let radius = 10.0; // 20 px diameter.
            let btn_center = egui::pos2(
                (rect.right() - pad - radius).max(rect.left() + radius),
                (rect.top() + pad + radius).min(rect.bottom() - radius),
            );
            let btn_rect =
                egui::Rect::from_center_size(btn_center, egui::vec2(radius * 2.0, radius * 2.0));
            let del_resp = ui.interact(
                btn_rect,
                ui.make_persistent_id(("img_ann_delete_btn", page_num, si)),
                egui::Sense::click(),
            );

            let (bg_color, border_color, icon_color) = if del_resp.hovered() {
                hovered_any = true;
                cursor_icon = Some(egui::CursorIcon::PointingHand);
                (
                    rgb(self.theme.bg_active),
                    rgb(self.theme.border),
                    egui::Color32::WHITE,
                )
            } else {
                (
                    rgb(self.theme.bg_hover),
                    rgb(self.theme.border),
                    rgb(self.theme.fg_muted),
                )
            };

            // Circular background.
            ui.painter().circle_filled(btn_center, radius, bg_color);
            ui.painter()
                .circle_stroke(btn_center, radius, egui::Stroke::new(1.0, border_color));

            // Compact delete icon.
            let c = btn_center;
            let s = egui::Stroke::new(1.2, icon_color);
            let s_thin = egui::Stroke::new(1.0, icon_color);

            // Horizontal lid.
            ui.painter()
                .line_segment([c + egui::vec2(-4.2, -2.6), c + egui::vec2(4.2, -2.6)], s);
            // Top handle.
            ui.painter().line_segment(
                [c + egui::vec2(-1.5, -4.0), c + egui::vec2(1.5, -4.0)],
                s_thin,
            );
            ui.painter().line_segment(
                [c + egui::vec2(-1.5, -4.0), c + egui::vec2(-1.5, -2.6)],
                s_thin,
            );
            ui.painter().line_segment(
                [c + egui::vec2(1.5, -4.0), c + egui::vec2(1.5, -2.6)],
                s_thin,
            );
            // Bin body.
            let body = vec![
                c + egui::vec2(-3.2, -2.6),
                c + egui::vec2(-2.6, 4.0),
                c + egui::vec2(2.6, 4.0),
                c + egui::vec2(3.2, -2.6),
            ];
            ui.painter().add(egui::Shape::line(body, s));
            // Due linee verticali interne
            ui.painter().line_segment(
                [c + egui::vec2(-1.0, -0.8), c + egui::vec2(-0.9, 2.4)],
                s_thin,
            );
            ui.painter().line_segment(
                [c + egui::vec2(1.0, -0.8), c + egui::vec2(0.9, 2.4)],
                s_thin,
            );

            if del_resp.clicked() {
                delete_clicked = true;
            }
        }
        if let Some(icon) = cursor_icon {
            ui.ctx().set_cursor_icon(icon);
        }

        // Drag in progress.
        if let Some(d) = new_drag {
            self.push_undo_snapshot(page_num);
            self.image_drag = Some(d);
        }
        if let Some(d) = self.image_drag
            && d.page == page_num
        {
            let (down, pointer) = ui.input(|i| {
                (
                    i.pointer.primary_down(),
                    i.pointer.interact_pos().or(i.pointer.hover_pos()),
                )
            });
            if down {
                if let (Some(pos), Some(ann)) = (
                    pointer,
                    self.annotations
                        .get_mut(&page_num)
                        .and_then(|v| v.get_mut(d.idx)),
                ) && ann.kind == annotations::AnnotationKind::Image
                    && ann.points.len() >= 2
                {
                    let (px, py) = to_norm(pos);
                    let (a0, a1) = (ann.points[0], ann.points[1]);
                    let (x0, x1) = (a0.0.min(a1.0), a0.0.max(a1.0));
                    let (y0, y1) = (a0.1.min(a1.1), a0.1.max(a1.1));
                    let (w, h) = (x1 - x0, y1 - y0);
                    let points = match d.mode {
                        ImageDragMode::Move { grab } => {
                            let nx = (px - grab.0).clamp(0.0, (1.0 - w).max(0.0));
                            let ny = (py - grab.1).clamp(0.0, (1.0 - h).max(0.0));
                            [(nx, ny), (nx + w, ny + h)]
                        }
                        ImageDragMode::Resize { anchor } => {
                            let aspect = (w * sw) / (h * sh).max(1.0); // Pixel aspect ratio.
                            let sx = if px >= anchor.0 { 1.0 } else { -1.0 };
                            let sy = if py >= anchor.1 { 1.0 } else { -1.0 };
                            let max_w_x = if sx > 0.0 { 1.0 - anchor.0 } else { anchor.0 };
                            let max_h_y = if sy > 0.0 { 1.0 - anchor.1 } else { anchor.1 };
                            // h_n = w_n * sw / aspect / sh
                            let max_w = max_w_x.min(max_h_y * aspect * sh / sw);
                            let min_w = 24.0 / sw;
                            let new_w = (px - anchor.0)
                                .abs()
                                .clamp(min_w.min(max_w), max_w.max(0.001));
                            let new_h = new_w * sw / aspect / sh;
                            let c = (anchor.0 + sx * new_w, anchor.1 + sy * new_h);
                            [
                                (anchor.0.min(c.0), anchor.1.min(c.1)),
                                (anchor.0.max(c.0), anchor.1.max(c.1)),
                            ]
                        }
                    };
                    if ann.points != points {
                        ann.points = points.to_vec();
                        self.mark_study_changed();
                    }
                }
                ui.ctx().request_repaint();
            } else {
                self.image_drag = None;
            }
        }

        // Clear selection when clicking outside all images.
        let clicked_elsewhere = ui.input(|i| {
            i.pointer.primary_clicked()
                && i.pointer
                    .interact_pos()
                    .is_some_and(|p| slide_rect.contains(p))
        });
        if cursor_tool
            && clicked_elsewhere
            && !hovered_any
            && self.image_drag.is_none()
            && matches!(self.selected_image, Some((p, _)) if p == page_num)
        {
            self.selected_image = None;
        }

        // Delete the selected image with Delete, Backspace, or the close icon.
        if let Some((sp, si)) = self.selected_image {
            let typing = ui.ctx().memory(|m| m.focused().is_some());
            let key_del = !typing
                && ui.input(|i| {
                    i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)
                });
            if cursor_tool
                && sp == page_num
                && (key_del || delete_clicked)
                && self.image_drag.is_none()
            {
                self.push_undo_snapshot(page_num);
                if let Some(list) = self.annotations.get_mut(&page_num)
                    && si < list.len()
                    && list[si].kind == annotations::AnnotationKind::Image
                {
                    list.remove(si);
                }
                self.selected_image = None;
                self.save_study_edit();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::isolated_dir;

    #[test]
    fn editing_an_existing_textbox_creates_one_pdf_undo_step() {
        let root = isolated_dir!(editing_an_existing_textbox_creates_one_pdf_undo_step);
        let pdf = root.join("textboxes.pdf");
        notes::create_blank_study_notebook(&pdf).unwrap();
        let ctx = egui::Context::default();
        let mut app = Lapis::new(
            &eframe::CreationContext::_new_kittest(ctx.clone()),
            Some(pdf),
        );
        app.annotation_tool = AnnotationTool::Cursor;
        app.annotations.insert(
            1,
            vec![notes::tests::sample_textbox(
                "Original",
                vec![(0.1, 0.1), (0.5, 0.3)],
            )],
        );
        let page_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let draw = |app: &mut Lapis, events, enter_edit| {
            let raw = egui::RawInput {
                screen_rect: Some(page_rect),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(raw, |ui| {
                if enter_edit {
                    let key = ui.make_persistent_id(("tb_sel_state", app.textbox_epoch));
                    ui.data_mut(|d| d.insert_temp(key, (1_u32, 0_usize, true)));
                }
                app.render_page_textboxes(ui, 1, page_rect);
            });
            output.textures_delta.clear();
        };

        draw(&mut app, vec![], true);
        draw(&mut app, vec![egui::Event::Text(" first".into())], false);
        draw(&mut app, vec![egui::Event::Text(" second".into())], false);
        assert_eq!(app.undo_stack[&1].len(), 1);
        let edited = app.annotations[&1][0].text.clone();
        assert_ne!(edited, "Original");

        app.undo_annotation(1);
        assert_eq!(app.annotations[&1][0].text, "Original");
        app.redo_annotation(1);
        assert_eq!(app.annotations[&1][0].text, edited);
    }
}

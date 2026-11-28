//! Selectable PDF text and its page coordinates.

use mupdf::Document;

#[derive(Clone)]
pub(crate) struct PdfCharItem {
    pub(crate) ch: char,
    /// Normalized bounding box (x0, y0, x1, y1) in 0.0..=1.0 of the page.
    pub(crate) rect: [f32; 4],
}

/// A visual line of text: chars `start..end` of `PageText::chars` (lines partition the
/// char array, so a selection is just a pair of indices).
#[derive(Clone)]
pub(crate) struct TextLine {
    pub(crate) start: usize,
    pub(crate) end: usize,
    /// Normalized bounding box of the whole line.
    pub(crate) bbox: [f32; 4],
}

#[derive(Clone, Default)]
pub(crate) struct PageText {
    pub(crate) chars: Vec<PdfCharItem>,
    pub(crate) lines: Vec<TextLine>,
}

impl PageText {
    /// Line closest to a point (normalized coords): vertical distance first, then
    /// horizontal distance (so multi-column layouts pick the right column).
    fn line_for_point(&self, nx: f32, ny: f32) -> Option<usize> {
        let mut best: Option<(usize, f32, f32)> = None;
        for (i, l) in self.lines.iter().enumerate() {
            let dy = if ny < l.bbox[1] {
                l.bbox[1] - ny
            } else if ny > l.bbox[3] {
                ny - l.bbox[3]
            } else {
                0.0
            };
            let dx = if nx < l.bbox[0] {
                l.bbox[0] - nx
            } else if nx > l.bbox[2] {
                nx - l.bbox[2]
            } else {
                0.0
            };
            let better = match best {
                None => true,
                Some((_, by, bx)) => dy < by - 1e-6 || ((dy - by).abs() <= 1e-6 && dx < bx),
            };
            if better {
                best = Some((i, dy, dx));
            }
        }
        best.map(|b| b.0)
    }

    /// Caret position (between chars) closest to a point: passing the middle of a
    /// character includes it, exactly like in a text editor.
    pub(crate) fn caret_at(&self, nx: f32, ny: f32) -> usize {
        let Some(li) = self.line_for_point(nx, ny) else {
            return 0;
        };
        let l = &self.lines[li];
        for i in l.start..l.end {
            let r = &self.chars[i].rect;
            if nx < (r[0] + r[2]) * 0.5 {
                return i;
            }
        }
        l.end
    }
    pub(crate) fn is_over_text(&self, nx: f32, ny: f32) -> bool {
        const PAD_X: f32 = 0.004;
        const PAD_Y: f32 = 0.003;

        self.chars.iter().any(|ch| {
            nx >= ch.rect[0] - PAD_X
                && nx <= ch.rect[2] + PAD_X
                && ny >= ch.rect[1] - PAD_Y
                && ny <= ch.rect[3] + PAD_Y
        })
    }
    /// Character under (or nearest to) a point, for word selection.
    pub(crate) fn char_at(&self, nx: f32, ny: f32) -> Option<usize> {
        let l = &self.lines[self.line_for_point(nx, ny)?];
        (l.start..l.end)
            .find(|&i| nx <= self.chars[i].rect[2])
            .or(Some(l.end.saturating_sub(1)))
            .filter(|&i| i < self.chars.len())
    }

    pub(crate) fn line_range(&self, i: usize) -> (usize, usize) {
        match self.lines.iter().find(|l| i >= l.start && i < l.end) {
            Some(l) => (l.start, l.end),
            None => (i, i),
        }
    }

    pub(crate) fn word_range(&self, i: usize) -> (usize, usize) {
        let Some(l) = self.lines.iter().find(|l| i >= l.start && i < l.end) else {
            return (i, i);
        };
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        if !is_word(self.chars[i].ch) {
            return (i, i + 1);
        }
        let mut a = i;
        while a > l.start && is_word(self.chars[a - 1].ch) {
            a -= 1;
        }
        let mut b = i + 1;
        while b < l.end && is_word(self.chars[b].ch) {
            b += 1;
        }
        (a, b)
    }

    /// Plain text of `lo..hi`, one '\n' per visual line.
    pub(crate) fn text_in(&self, lo: usize, hi: usize) -> String {
        let mut out = String::new();
        for l in &self.lines {
            let s = l.start.max(lo);
            let e = l.end.min(hi);
            if s >= e {
                continue;
            }
            if !out.is_empty() {
                out.push('\n');
            }
            let seg: String = self.chars[s..e].iter().map(|c| c.ch).collect();
            out.push_str(seg.trim_end());
        }
        out
    }
}

/// Extracts selectable text of one page (scale independent, so done once per page).
pub(crate) fn extract_text(doc: &Document, idx: i32) -> Option<PageText> {
    let page = doc.load_page(idx).ok()?;
    let b = page.bounds().ok()?;
    let (pw, ph) = (b.x1 - b.x0, b.y1 - b.y0);
    let mut out = PageText::default();
    if pw <= 0.0 || ph <= 0.0 {
        return Some(out);
    }
    let Ok(tp) = page.to_text_page(mupdf::TextPageFlags::empty()) else {
        return Some(out);
    };
    for block in tp.blocks() {
        for line in block.lines() {
            let start = out.chars.len();
            let mut bbox = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
            for ci in line.chars() {
                let Some(c) = ci.char() else { continue };
                if c == '\0' || c.is_control() {
                    continue;
                }
                let q = ci.quad();
                let x0 = q.ul.x.min(q.ll.x).min(q.ur.x).min(q.lr.x) / pw;
                let x1 = q.ul.x.max(q.ll.x).max(q.ur.x).max(q.lr.x) / pw;
                let y0 = q.ul.y.min(q.ur.y).min(q.ll.y).min(q.lr.y) / ph;
                let y1 = q.ul.y.max(q.ur.y).max(q.ll.y).max(q.lr.y) / ph;
                bbox[0] = bbox[0].min(x0);
                bbox[1] = bbox[1].min(y0);
                bbox[2] = bbox[2].max(x1);
                bbox[3] = bbox[3].max(y1);
                out.chars.push(PdfCharItem {
                    ch: c,
                    rect: [x0, y0, x1, y1],
                });
            }
            let end = out.chars.len();
            if end > start {
                out.lines.push(TextLine { start, end, bbox });
            }
        }
    }
    Some(out)
}

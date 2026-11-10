//! Global search: text folding, matching, snippets and the result-row widget.
//!
//! Matching is done on a *folded* copy of each text (lower-cased, runs of whitespace collapsed
//! into one space) while every folded character remembers the character of the original text it
//! came from. That keeps the reported ranges exact (they are used to highlight the hit in the
//! notes editor and on the PDF page) and lets a query match across line breaks.

use eframe::egui::{self, Color32};

/// Upper bound on the hits kept for one query (keeps the list and the memory bounded).
pub const MAX_HITS: usize = 500;

/// Height of one result row.
pub const ROW_H: f32 = 28.0;

/// Marks a character that does not exist in the original text (e.g. the space inserted
/// between two lines of a PDF page).
pub const SYNTHETIC: usize = usize::MAX;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    Note,
    Pdf,
    Audio,
    Memo,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Note => "Note",
            Source::Pdf => "Slide",
            Source::Audio => "Audio",
            Source::Memo => "Memo",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Source::Note => "\u{e68f}",
            Source::Pdf => "\u{f0219}",
            Source::Audio => "\u{f057e}",
            Source::Memo => "\u{f036c}",
        }
    }
}

/// One search result.
#[derive(Clone, Debug)]
pub struct Hit {
    pub page: u32,
    pub source: Source,
    /// Character range of the match in its source (note text / PDF page characters).
    /// Meaningless (0..0) for audio hits.
    pub start: usize,
    pub end: usize,
    /// Text around the match, shown in the list; always contains the match.
    pub snippet: String,
    /// Character range of the match inside `snippet`.
    pub snip_match: (usize, usize),
    pub ts_ms: Option<u64>,
    /// Extra label shown next to the source (timestamp, memo id, ...).
    pub tag: String,
}

/// A match inside one `SearchPage`.
pub struct PageMatch {
    pub start: usize,
    pub end: usize,
    pub snippet: String,
    pub snip_match: (usize, usize),
}

/// Searchable form of one text (a note, the text of a PDF page, a transcript segment).
#[derive(Default, Clone)]
pub struct SearchPage {
    /// Original characters (line breaks of PDFs replaced by a space).
    flat: Vec<char>,
    /// For each `flat` char: index in the original source, or `SYNTHETIC`.
    flat_src: Vec<usize>,
    /// Folded characters.
    norm: Vec<char>,
    /// For each folded char: index in `flat`.
    norm_map: Vec<usize>,
}

fn fold_char(c: char) -> char {
    if c.is_whitespace() || c.is_control() {
        ' '
    } else {
        // First char only: keeps a strict 1:1 mapping with the original text.
        c.to_lowercase().next().unwrap_or(c)
    }
}

/// Folds a query: trimmed, lower-cased, whitespace collapsed.
pub fn fold_query(q: &str) -> Vec<char> {
    let mut out: Vec<char> = Vec::new();
    for c in q.trim().chars() {
        let f = fold_char(c);
        if f == ' ' && out.last() == Some(&' ') {
            continue;
        }
        out.push(f);
    }
    out
}

impl SearchPage {
    /// Builds a page from `(char, source index)` pairs.
    pub fn new(items: impl IntoIterator<Item = (char, usize)>) -> Self {
        let mut p = SearchPage::default();
        for (c, src) in items {
            p.flat.push(c);
            p.flat_src.push(src);
        }
        for (i, &c) in p.flat.iter().enumerate() {
            let f = fold_char(c);
            if f == ' ' && p.norm.last() == Some(&' ') {
                continue;
            }
            p.norm.push(f);
            p.norm_map.push(i);
        }
        p
    }

    pub fn from_text(text: &str) -> Self {
        Self::new(text.chars().enumerate().map(|(i, c)| (c, i)))
    }

    /// All non-overlapping matches of the folded `needle`, at most `limit`.
    pub fn find(&self, needle: &[char], limit: usize) -> Vec<PageMatch> {
        let mut out = Vec::new();
        let n = needle.len();
        if n == 0 || n > self.norm.len() {
            return out;
        }
        let mut i = 0;
        while i + n <= self.norm.len() && out.len() < limit {
            if self.norm[i] == needle[0] && self.norm[i..i + n] == *needle {
                let fs = self.norm_map[i];
                let fe = self.norm_map[i + n - 1] + 1;
                let (a, b) = (self.flat_src[fs], self.flat_src[fe - 1]);
                if a != SYNTHETIC && b != SYNTHETIC {
                    let (snippet, snip_match) = make_snippet(&self.flat, fs, fe, 10, 60);
                    out.push(PageMatch {
                        start: a,
                        end: b + 1,
                        snippet,
                        snip_match,
                    });
                }
                i += n;
            } else {
                i += 1;
            }
        }
        out
    }
}

/// Text around `src[s..e]`, whitespace collapsed, with `…` where it was cut.
/// Returns the snippet and the character range of the match inside it.
pub fn make_snippet(
    src: &[char],
    s: usize,
    e: usize,
    before: usize,
    after: usize,
) -> (String, (usize, usize)) {
    let mut a = s.saturating_sub(before);
    let b = (e + after).min(src.len());
    // Start on a word boundary when that is cheap.
    if a > 0 {
        let limit = (a + 10).min(s);
        if let Some(off) = src[a..limit].iter().position(|c| c.is_whitespace()) {
            a += off + 1;
        }
    }
    let mut out = String::new();
    let mut n = 0usize;
    if a > 0 {
        out.push('…');
        n += 1;
    }
    let (mut ms, mut me) = (n, n);
    let (mut ms_set, mut me_set) = (false, false);
    let mut last_space = true;
    for (k, &c) in src.iter().enumerate().take(b).skip(a) {
        if k == s && !ms_set {
            ms = n;
            ms_set = true;
        }
        if k == e && !me_set {
            me = n;
            me_set = true;
        }
        let c = if c.is_whitespace() || c.is_control() {
            ' '
        } else {
            c
        };
        if c == ' ' {
            if last_space {
                continue;
            }
            last_space = true;
        } else {
            last_space = false;
        }
        out.push(c);
        n += 1;
    }
    if !me_set {
        me = n;
    }
    if out.ends_with(' ') {
        out.pop();
        n -= 1;
        me = me.min(n);
    }
    if b < src.len() {
        out.push('…');
    }
    (out, (ms.min(me), me))
}

/// Colors used to paint a result row.
pub struct RowColors {
    pub accent: Color32,
    pub text: Color32,
    pub weak: Color32,
    pub hover: Color32,
}

fn char_range_to_bytes(s: &str, (a, b): (usize, usize)) -> (usize, usize) {
    let mut it = s
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(s.len()));
    let start = it.nth(a).unwrap_or(s.len());
    let end = if b >= a {
        s.char_indices()
            .map(|(i, _)| i)
            .chain(std::iter::once(s.len()))
            .nth(b)
            .unwrap_or(s.len())
    } else {
        start
    };
    (start, end.max(start))
}

/// One compact result row: source icon (and timestamp), the match in its context and the page.
pub fn result_row(ui: &mut egui::Ui, hit: &Hit, selected: bool, c: &RowColors) -> egui::Response {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, ROW_H), egui::Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let painter = ui.painter();
    let r = rect.shrink2(egui::vec2(0.0, 1.0));
    if selected {
        painter.rect_filled(
            r,
            egui::CornerRadius::same(6),
            Color32::from_rgba_unmultiplied(c.accent.r(), c.accent.g(), c.accent.b(), 48),
        );
        painter.rect_stroke(
            r,
            egui::CornerRadius::same(6),
            egui::Stroke::new(1.0, c.accent),
            egui::StrokeKind::Inside,
        );
    } else if resp.hovered() {
        painter.rect_filled(r, egui::CornerRadius::same(6), c.hover);
    }

    let pad = 8.0;
    let mid = r.center().y;
    let small = egui::FontId::proportional(11.0);

    let icon = painter.text(
        egui::pos2(r.left() + pad, mid),
        egui::Align2::LEFT_CENTER,
        hit.source.icon(),
        egui::FontId::proportional(13.0),
        c.accent,
    );
    let mut x = icon.right() + 6.0;
    if !hit.tag.is_empty() {
        let tag = painter.text(
            egui::pos2(x, mid),
            egui::Align2::LEFT_CENTER,
            &hit.tag,
            small.clone(),
            c.weak,
        );
        x = tag.right() + 6.0;
    }
    let page = painter.text(
        egui::pos2(r.right() - pad, mid),
        egui::Align2::RIGHT_CENTER,
        format!("p. {}", hit.page),
        small,
        c.weak,
    );

    // The match in its context, truncated to the space left between icon and page number.
    let (b0, b1) = char_range_to_bytes(&hit.snippet, hit.snip_match);
    let font = egui::FontId::proportional(13.0);
    let plain = egui::TextFormat {
        font_id: font.clone(),
        color: c.text,
        ..Default::default()
    };
    let marked = egui::TextFormat {
        font_id: font,
        color: c.accent,
        background: Color32::from_rgba_unmultiplied(c.accent.r(), c.accent.g(), c.accent.b(), 55),
        ..Default::default()
    };
    let mut job = egui::text::LayoutJob::default();
    job.wrap = egui::text::TextWrapping::truncate_at_width((page.left() - 8.0 - x).max(20.0));
    if b0 > 0 {
        job.append(&hit.snippet[..b0], 0.0, plain.clone());
    }
    if b1 > b0 {
        job.append(&hit.snippet[b0..b1], 0.0, marked);
    }
    if b1 < hit.snippet.len() {
        job.append(&hit.snippet[b1..], 0.0, plain);
    }
    let galley = painter.layout_job(job);
    painter.galley(egui::pos2(x, mid - galley.size().y / 2.0), galley, c.text);

    // Full text on hover, since the row is truncated.
    resp.on_hover_text(format!("{} · {}", hit.source.label(), hit.snippet))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(text: &str, q: &str) -> Vec<PageMatch> {
        SearchPage::from_text(text).find(&fold_query(q), 100)
    }

    #[test]
    fn case_insensitive_and_exact_ranges() {
        let m = find("Hello World, hello again", "HELLO");
        assert_eq!(m.len(), 2);
        assert_eq!((m[0].start, m[0].end), (0, 5));
        assert_eq!((m[1].start, m[1].end), (13, 18));
    }

    #[test]
    fn snippet_always_contains_the_match() {
        let long = format!(
            "{} needle {}",
            "lorem ipsum ".repeat(20),
            "dolor sit ".repeat(20)
        );
        let m = find(&long, "needle");
        assert_eq!(m.len(), 1);
        let chars: Vec<char> = m[0].snippet.chars().collect();
        let got: String = chars[m[0].snip_match.0..m[0].snip_match.1].iter().collect();
        assert_eq!(got, "needle");
        assert!(m[0].snippet.chars().count() < 120);
    }

    #[test]
    fn matches_across_line_breaks_and_collapsed_spaces() {
        let items: Vec<(char, usize)> =
            "foo bar".chars().enumerate().map(|(i, c)| (c, i)).collect();
        let mut v = Vec::new();
        for (c, i) in &items[..3] {
            v.push((*c, *i));
        }
        v.push((' ', SYNTHETIC));
        for (c, i) in &items[4..] {
            v.push((*c, *i));
        }
        let p = SearchPage::new(v);
        let m = p.find(&fold_query("foo   bar"), 10);
        assert_eq!(m.len(), 1);
        assert_eq!((m[0].start, m[0].end), (0, 7));
    }

    #[test]
    fn unicode_and_no_match() {
        let m = find("Perché è così", "così");
        assert_eq!(m.len(), 1);
        assert_eq!((m[0].start, m[0].end), (9, 13));
        assert!(find("abc", "xyz").is_empty());
        assert!(find("abc", "   ").is_empty());
    }

    #[test]
    fn snippet_match_at_edges() {
        let m = find("needle", "needle");
        assert_eq!(m[0].snippet, "needle");
        assert_eq!(m[0].snip_match, (0, 6));
        let m = find("a needle", "needle");
        assert_eq!(m[0].snip_match, (2, 8));
    }
}

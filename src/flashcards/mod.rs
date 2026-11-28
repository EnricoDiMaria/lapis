//! Parse and export study cards written in slide notes.

use crate::markdown::{
    code_and_math_ranges, in_ranges, preprocess_markdown_math, preprocess_markdown_view,
};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct Flashcard {
    pub page: u32,
    pub front: String,
    pub back: String,
}

/// Parses all slide notes to extract flashcards using either:
/// 1. Inline syntax: `Concept :: Definition`
/// 2. Multi-line block syntax: `Q: Question...` followed by `A: Answer...`
pub fn extract_flashcards_from_notes(notes: &HashMap<u32, String>) -> Vec<Flashcard> {
    let mut cards = Vec::new();
    let mut sorted_pages: Vec<u32> = notes.keys().copied().collect();
    sorted_pages.sort_unstable();

    for page in sorted_pages {
        if let Some(content) = notes.get(&page) {
            cards.extend(extract_flashcards_from_page(page, content));
        }
    }

    cards
}

/// Removes a Markdown list marker (`- `, `* `, `+ `) at the start of a line. A `*` that is part of
/// the text (`*args`, `**Term**`, `A*`) is never touched.
fn strip_list_marker(line: &str) -> &str {
    let mut t = line.trim();
    while let Some(rest) = ["- ", "* ", "+ "].iter().find_map(|m| t.strip_prefix(m)) {
        t = rest.trim_start();
    }
    t
}

/// `**Term**` / `*Term*` -> `Term`, only when the emphasis wraps the whole title and nothing inside
/// it uses `*`; any other asterisk belongs to the title and is kept.
fn strip_title_emphasis(title: &str) -> &str {
    let t = title.trim();
    for m in ["***", "**", "*"] {
        if t.len() > 2 * m.len() && t.starts_with(m) && t.ends_with(m) {
            let inner = t[m.len()..t.len() - m.len()].trim();
            if !inner.is_empty() && !inner.contains('*') {
                return inner;
            }
        }
    }
    t
}

/// Offset in `hay` of the first `needle` outside code and math. `base` is `hay`'s offset in the note.
fn find_outside(
    hay: &str,
    base: usize,
    needle: &str,
    skip: &[std::ops::Range<usize>],
) -> Option<usize> {
    hay.match_indices(needle)
        .map(|(i, _)| i)
        .find(|&i| !in_ranges(skip, base + i))
}

/// Whether a note contains at least one flashcard (cheap pre-check; runs every frame).
pub fn page_has_flashcards(content: &str) -> bool {
    (content.contains("::") || content.contains("Q:") || content.contains("D:"))
        && !extract_flashcards_from_page(0, content).is_empty()
}

fn extract_flashcards_from_page(page: u32, content: &str) -> Vec<Flashcard> {
    flashcard_spans(page, content)
        .into_iter()
        .map(|(_, card, _)| card)
        .collect()
}

// The span covers the original source; the bool distinguishes inline and Q/A cards.
fn flashcard_spans(page: u32, content: &str) -> Vec<(std::ops::Range<usize>, Flashcard, bool)> {
    let mut cards = Vec::new();
    // Code and math never produce cards; offsets let us tell where a marker sits.
    let skip = code_and_math_ranges(content);
    let mut offset = 0;
    let mut lines = content
        .split_inclusive('\n')
        .map(|l| {
            let start = offset;
            offset += l.len();
            (start, l)
        })
        .peekable();
    while let Some((line_off, raw_line)) = lines.next() {
        let trimmed = strip_list_marker(raw_line);
        // `trimmed` is a subslice of `raw_line`.
        let base = line_off + (trimmed.as_ptr() as usize - raw_line.as_ptr() as usize);

        let question_start = if !in_ranges(&skip, base) {
            trimmed
                .strip_prefix("Q:")
                .or_else(|| trimmed.strip_prefix("**Q:**"))
                .or_else(|| trimmed.strip_prefix("D:"))
        } else {
            None
        };
        // Inline cards must not split a multiline math or code span.
        let line_end = line_off + raw_line.len();
        let first = skip.partition_point(|r| r.end <= line_off);
        let crosses_line = skip[first..]
            .iter()
            .take_while(|r| r.start < line_end)
            .any(|r| r.start < line_off || r.end > line_end);
        if !crosses_line && let Some(p) = find_outside(trimmed, base, "::", &skip) {
            let f = strip_title_emphasis(&trimmed[..p]);
            let b = trimmed[p + 2..].trim();
            if !f.is_empty() && !b.is_empty() {
                cards.push((
                    line_off..line_end,
                    Flashcard {
                        page,
                        front: f.to_string(),
                        back: b.to_string(),
                    },
                    true,
                ));
                continue;
            }
        }

        // 2. Check `Q: ...` / `A: ...` block syntax
        if let Some(q_start) = question_start {
            let mut end = line_off + raw_line.len();
            let mut question = q_start.trim().to_string();
            let mut answer = String::new();
            let mut in_answer = false;

            while let Some(&(next_off, next_raw)) = lines.peek() {
                let next_trim = next_raw.trim();
                let lead = next_raw.len() - next_raw.trim_start().len();
                // Markers inside a code block or math are content, not syntax.
                let is_syntax = !in_ranges(&skip, next_off + lead);
                if next_trim.is_empty() && in_answer && !in_ranges(&skip, next_off) {
                    break;
                }
                let a_start = if is_syntax {
                    next_trim
                        .strip_prefix("A:")
                        .or_else(|| next_trim.strip_prefix("**A:**"))
                        .or_else(|| next_trim.strip_prefix("R:"))
                } else {
                    None
                };
                if (is_syntax
                    && (next_trim.starts_with("Q:")
                        || next_trim.starts_with("**Q:**")
                        || next_trim.starts_with("D:")))
                    || (a_start.is_none()
                        && find_outside(next_raw, next_off, "::", &skip).is_some())
                {
                    break;
                }

                let consumed = lines.next().unwrap().1.trim_end_matches(['\r', '\n']);
                end = next_off + next_raw.len();
                if let Some(a_start) = a_start {
                    in_answer = true;
                    answer.push_str(a_start.trim());
                } else if in_answer {
                    if !answer.is_empty() {
                        answer.push('\n');
                    }
                    answer.push_str(consumed);
                } else if !consumed.is_empty() {
                    if !question.is_empty() {
                        question.push('\n');
                    }
                    question.push_str(consumed);
                }
            }

            if !question.is_empty() && !answer.is_empty() {
                cards.push((
                    line_off..end,
                    Flashcard {
                        page,
                        front: question,
                        back: answer,
                    },
                    false,
                ));
            }
        }
    }

    cards
}

/// Escapes the `*` of a flashcard question that Markdown would swallow because they do not form a
/// valid emphasis (`*args`, `A*`, `a * b`, `**kwargs`), so they are shown as typed. Asterisks that
/// do open and close an emphasis (`**bold**`, `*italic*`), list markers and code spans are kept.
pub fn escape_stray_asterisks(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len() + 8);
    for (i, line) in markdown.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let indent = line.len() - line.trim_start().len();
        let body = &line[indent..];
        out.push_str(&line[..indent]);
        let body = match body.strip_prefix("* ") {
            Some(rest) => {
                out.push_str("* ");
                rest
            }
            None => body,
        };
        // Even segments are text, odd ones are inside `code spans`.
        let segs: Vec<&str> = body.split('`').collect();
        let closed = segs.len() % 2 == 1;
        for (n, seg) in segs.iter().enumerate() {
            if n > 0 {
                out.push('`');
            }
            if n % 2 == 0 || !closed {
                out.push_str(&escape_stray_in_text(seg));
            } else {
                out.push_str(seg);
            }
        }
    }
    out
}

fn escape_stray_in_text(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    // Runs of `*`: (start, len, can_open, can_close)
    let mut runs: Vec<(usize, usize, bool, bool)> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '*' && (i == 0 || chars[i - 1] != '\\') {
            let start = i;
            while i < chars.len() && chars[i] == '*' {
                i += 1;
            }
            let before = start.checked_sub(1).map(|j| chars[j]);
            let after = chars.get(i).copied();
            let can_open = after.is_some_and(|c| !c.is_whitespace());
            let can_close = before.is_some_and(|c| !c.is_whitespace());
            runs.push((start, i - start, can_open, can_close));
        } else {
            i += 1;
        }
    }
    // Pair closers with the nearest opener of the same length.
    let mut matched = vec![false; runs.len()];
    let mut stack: Vec<usize> = Vec::new();
    for (ri, &(_, len, can_open, can_close)) in runs.iter().enumerate() {
        if can_close && let Some(pos) = stack.iter().rposition(|&o| runs[o].1 == len) {
            let o = stack.remove(pos);
            matched[o] = true;
            matched[ri] = true;
            continue;
        }
        if can_open {
            stack.push(ri);
        }
    }
    let mut out = String::with_capacity(text.len() + 4);
    let mut next_run = 0;
    let mut i = 0;
    while i < chars.len() {
        if next_run < runs.len() && runs[next_run].0 == i {
            let (_, len, _, _) = runs[next_run];
            for _ in 0..len {
                if !matched[next_run] {
                    out.push('\\');
                }
                out.push('*');
            }
            i += len;
            next_run += 1;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// Exports extracted flashcards into an Anki-ready `.tsv` file with automatic slide tags.
pub fn export_flashcards_anki_tsv(
    out_path: &Path,
    pdf_name: &str,
    cards: &[Flashcard],
) -> io::Result<()> {
    let mut out = String::from("#separator:tab\n#html:true\n#tags column:3\n");
    let clean_deck_tag: String = pdf_name
        .trim_end_matches(".pdf")
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();

    for c in cards {
        let front_fmt = preprocess_markdown_math(&c.front)
            .replace('\t', " ")
            .replace("  \n", "<br>")
            .replace('\n', "<br>");
        let back_fmt = preprocess_markdown_math(&c.back)
            .replace('\t', " ")
            .replace("  \n", "<br>")
            .replace('\n', "<br>");
        let tag = format!("{} slide_{}", clean_deck_tag, c.page);
        out.push_str(&format!("{}\t{}\t{}\n", front_fmt, back_fmt, tag));
    }

    fs::write(out_path, out)
}

/// Prepare notes for Preview, using the same card boundaries as the trainer.
pub fn preprocess_notes_preview(markdown: &str) -> String {
    fn quote(out: &mut String, text: &str) {
        for line in text.trim_matches('\n').lines() {
            out.push_str("> ");
            out.push_str(line);
            out.push('\n');
        }
    }

    if !markdown.contains("::") && !markdown.contains("Q:") && !markdown.contains("D:") {
        return preprocess_markdown_view(markdown);
    }
    let cards = flashcard_spans(0, markdown);
    if cards.is_empty() {
        return preprocess_markdown_view(markdown);
    }
    let mut out = String::with_capacity(markdown.len() + 128);
    let mut pos = 0;
    for (span, card, inline) in cards {
        out.push_str(&preprocess_markdown_view(&markdown[pos..span.start]));
        out.push('\n');
        // Prepare math and code before adding the blockquote prefix.
        let front = preprocess_markdown_view(&card.front);
        let back = preprocess_markdown_view(&card.back);
        if inline {
            // Avoid nesting emphasis around an already formatted title.
            let front = front.trim();
            if !front.contains(['*', '_', '\n']) {
                quote(&mut out, &format!("**{front}**  "));
            } else {
                quote(&mut out, &format!("{front}  "));
            }
        } else {
            out.push_str("> **Domanda**  \n");
            quote(&mut out, &front);
            out.push_str(">\n> **Risposta**  \n");
        }
        // Separate the answer from the title, including fenced code and display math.
        out.push_str(">\n");
        quote(&mut out, &back);
        out.push('\n');
        pos = span.end;
    }
    out.push_str(&preprocess_markdown_view(&markdown[pos..]));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card_fronts(note: &str) -> Vec<String> {
        let mut m = HashMap::new();
        m.insert(1u32, note.to_string());
        extract_flashcards_from_notes(&m)
            .into_iter()
            .map(|c| c.front)
            .collect()
    }

    #[test]
    fn asterisks_in_flashcard_titles_are_kept() {
        assert_eq!(card_fronts("*args :: positional arguments"), ["*args"]);
        assert_eq!(card_fronts("**kwargs :: keyword arguments"), ["**kwargs"]);
        assert_eq!(card_fronts("A* :: a search algorithm"), ["A*"]);
        assert_eq!(card_fronts("a * b :: product"), ["a * b"]);
        assert_eq!(card_fronts("- A* :: search"), ["A*"]);
        // A wrapping emphasis is still removed, a list marker too.
        assert_eq!(card_fronts("**Mitosis** :: division"), ["Mitosis"]);
        assert_eq!(card_fronts("* *Mitosis* :: division"), ["Mitosis"]);
        // Q/A blocks written in bold keep their text clean.
        let mut m = HashMap::new();
        m.insert(1u32, "**Q:** What is *p?\n**A:** a pointer".to_string());
        let c = extract_flashcards_from_notes(&m);
        assert_eq!(
            (c[0].front.as_str(), c[0].back.as_str()),
            ("What is *p?", "a pointer")
        );
    }

    #[test]
    fn flashcards_ignore_code_and_math() {
        assert!(card_fronts("```cpp\nstd::cout :: x\n```").is_empty());
        assert!(card_fronts("~~~\na :: b\n~~~").is_empty());
        assert!(card_fronts("Use `std::vector` for arrays").is_empty());
        assert!(card_fronts("Inline $a :: b$ math").is_empty());
        assert!(card_fronts("$$\nx :: y\n$$").is_empty());
        assert!(card_fronts("$$\nx\n$$ :: not an inline card").is_empty());
        assert!(card_fronts("```\nQ: not a card\nA: still not\n```").is_empty());
        // A real marker next to code still works.
        assert_eq!(
            card_fronts("`std::vector` :: dynamic array"),
            ["`std::vector`"]
        );
        assert_eq!(card_fronts("Vector :: uses `a::b` inside"), ["Vector"]);
    }

    #[test]
    fn preview_recognizes_inline_code_and_math_cards() {
        let note =
            "`word` :: something\n$a::b$ :: relation\n**Term** :: `x::y`\n```a ` b``` :: code";
        let cards = extract_flashcards_from_page(1, note);
        assert_eq!(cards.len(), 4);
        let preview = preprocess_notes_preview(note);
        assert!(preview.contains("> **`word`**"));
        assert!(preview.contains("> something"));
        assert!(preview.contains("> **Term**"));
        assert!(preview.contains("> `x::y`"));
        assert!(preview.contains("> **```a ` b```**"));
        assert!(!preview.contains(" :: relation"));
        assert!(preview.contains("> relation"));
    }

    #[test]
    fn preview_preserves_non_cards_and_formatted_answers() {
        let note = "`a::b`\n$$\nx :: y\n$$\n~~~rust\nQ: a :: b\n~~~\nQ: incomplete";
        assert!(extract_flashcards_from_page(1, note).is_empty());
        assert_eq!(
            preprocess_notes_preview(note),
            preprocess_markdown_view(note)
        );

        let note = "Q: What is `$@`?\nA: Example $x^2$:\n```sh\n  echo \"$@\"\n\nQ: not a card\n```\n$$\ny^2\n$$\n\nD: Next\nR: Answer";
        let cards = extract_flashcards_from_page(1, note);
        assert_eq!(cards.len(), 2);
        assert!(cards[0].back.contains("  echo \"$@\""));
        let preview = preprocess_notes_preview(note);
        assert!(preview.contains("> ```sh\n>   echo \"$@\"\n> \n> Q: not a card\n> ```"));
        assert!(preview.contains("> **`y²`**"));
        assert!(preview.contains("> Next"));
        assert!(preview.contains("> Answer"));
    }

    #[test]
    fn flashcard_answers_may_contain_code() {
        let mut m = HashMap::new();
        m.insert(
            1u32,
            "Q: Print?\nA: Use:\n```cpp\nstd::cout << 1;\n\nQ: x\n```\ndone".to_string(),
        );
        let c = extract_flashcards_from_notes(&m);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].front, "Print?");
        assert_eq!(
            c[0].back,
            "Use:\n```cpp\nstd::cout << 1;\n\nQ: x\n```\ndone"
        );
        assert!(page_has_flashcards("Q: a\nA: b"));
        assert!(!page_has_flashcards("```\na :: b\n```"));
    }

    #[test]
    fn stray_asterisks_are_escaped_for_display() {
        assert_eq!(escape_stray_asterisks("*args"), "\\*args");
        assert_eq!(escape_stray_asterisks("A*"), "A\\*");
        assert_eq!(escape_stray_asterisks("a * b"), "a \\* b");
        assert_eq!(
            escape_stray_asterisks("**kwargs and *args"),
            "\\*\\*kwargs and \\*args"
        );
        // Real emphasis, list markers and code spans are left alone.
        assert_eq!(
            escape_stray_asterisks("a **bold** and *it* word"),
            "a **bold** and *it* word"
        );
        assert_eq!(escape_stray_asterisks("* item *x"), "* item \\*x");
        assert_eq!(escape_stray_asterisks("`a*b` and c*"), "`a*b` and c\\*");
    }
}

//! Markdown and math preparation shared by the notes editor and PDF export.

mod math;
mod syntax;

use math::format_latex_expr;
pub(crate) use syntax::{code_and_math_ranges, in_ranges};

/// Fence language names unknown to the bundled syntax set, mapped to the closest syntax it has
/// (e.g. TypeScript -> JavaScript). Only used for on-screen rendering, never for exports.
fn code_fence_alias(lang: &str) -> Option<&'static str> {
    Some(match lang.to_ascii_lowercase().as_str() {
        "ts" | "typescript" | "tsx" | "jsx" | "mjs" | "cjs" | "node" | "vue" | "svelte" => "js",
        "json5" | "jsonc" => "json",
        "kt" | "kts" | "kotlin" => "java",
        "csharp" | "c#" => "cs",
        "golang" => "go",
        "python3" | "py3" => "py",
        "c++" | "cc" | "cxx" | "hpp" | "hxx" | "cuda" | "cu" => "cpp",
        "shell" | "console" | "zsh" | "fish" | "ksh" | "dockerfile" => "sh",
        "scss" | "sass" | "less" => "css",
        "svg" | "xhtml" | "xaml" => "xml",
        "postgres" | "postgresql" | "mysql" | "sqlite" | "plsql" | "tsql" => "sql",
        "octave" => "matlab",
        _ => return None,
    })
}

/// Rewrites the language of fenced code blocks through [`code_fence_alias`].
fn alias_code_fences(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut in_code_block = false;
    for line in markdown.split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']);
        let eol = &line[body.len()..];
        let rest = body.trim_start();
        if rest.starts_with("```") && (in_code_block || !rest.trim_start_matches('`').contains('`'))
        {
            in_code_block = !in_code_block;
            if in_code_block {
                let indent = &body[..body.len() - rest.len()];
                let ticks = rest.chars().take_while(|&c| c == '`').count();
                let lang = rest[ticks..]
                    .trim()
                    .split(|c: char| c.is_whitespace() || c == ',' || c == '{')
                    .next()
                    .unwrap_or("");
                if let Some(alias) = code_fence_alias(lang) {
                    out.push_str(indent);
                    out.push_str(&"`".repeat(ticks));
                    out.push_str(alias);
                    out.push_str(eol);
                    continue;
                }
            }
        }
        out.push_str(line);
    }
    out
}

/// Markdown preparation for on-screen rendering: math preprocessing plus code-fence language
/// aliases, so more programming languages get syntax highlighting.
pub fn preprocess_markdown_view(markdown: &str) -> String {
    alias_code_fences(&preprocess_markdown_math_impl(markdown, true))
}

// Convert display and inline math into styled Markdown for egui_commonmark.

pub fn preprocess_markdown_math(markdown: &str) -> String {
    preprocess_markdown_math_impl(markdown, false)
}

fn preprocess_markdown_math_impl(markdown: &str, escape_shell: bool) -> String {
    fn push_literal(out: &mut String, text: &str, escape_shell: bool) {
        if !escape_shell {
            out.push_str(text);
            return;
        }
        for (i, ch) in text.char_indices() {
            if ch == '$' && text[i..].starts_with("$@") {
                let escapes = text.as_bytes()[..i]
                    .iter()
                    .rev()
                    .take_while(|&&b| b == b'\\')
                    .count();
                if escapes % 2 == 0 {
                    out.push('\\');
                }
            }
            out.push(ch);
        }
    }

    let mut result = String::with_capacity(markdown.len() + 64);
    let mut pos = 0;
    for range in code_and_math_ranges(markdown) {
        push_literal(&mut result, &markdown[pos..range.start], escape_shell);
        let span = &markdown[range.clone()];
        if span.starts_with("$$") {
            let formatted = format_latex_expr(&span[2..span.len() - 2]);
            result.push_str(&format!("\n\n**`{}`**\n\n", formatted));
        } else if span.starts_with('$') {
            let formatted = format_latex_expr(&span[1..span.len() - 1]);
            result.push_str(&format!("`{}`", formatted));
        } else {
            result.push_str(span);
        }
        pos = range.end;
    }
    push_literal(&mut result, &markdown[pos..], escape_shell);

    let mut final_out = String::with_capacity(result.len());
    let mut fence: Option<(char, usize)> = None;
    let lines: Vec<&str> = result.lines().collect();
    for (line_idx, &line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if let Some(marker) = trimmed.chars().next().filter(|c| *c == '`' || *c == '~') {
            let run = trimmed.chars().take_while(|&c| c == marker).count();
            if let Some((open, len)) = fence {
                if marker == open && run >= len && trimmed[run..].trim().is_empty() {
                    fence = None;
                    final_out.push_str(line);
                    final_out.push('\n');
                    continue;
                }
            } else if run >= 3 && (marker != '`' || !trimmed[run..].contains('`')) {
                fence = Some((marker, run));
            }
        }
        final_out.push_str(line);
        if fence.is_some() {
            final_out.push('\n');
            continue;
        }

        let is_blockquote = trimmed.starts_with('>');
        let next_is_blockquote = lines
            .get(line_idx + 1)
            .is_some_and(|next| next.trim().starts_with('>'));

        if is_blockquote && !next_is_blockquote {
            final_out.push_str("\n\n");
        } else if !trimmed.is_empty() && !is_blockquote {
            final_out.push_str("  \n");
        } else {
            final_out.push('\n');
        }
    }

    final_out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skipped(text: &str) -> Vec<&str> {
        code_and_math_ranges(text)
            .into_iter()
            .map(|r| &text[r])
            .collect()
    }

    #[test]
    fn code_and_math_ranges_cover_code_and_latex_only() {
        assert_eq!(
            skipped("a `x y` b $m n$ c\n```\ncode\n```\nd $$\nz\n$$ e"),
            ["`x y`", "$m n$", "```\ncode\n```\n", "$$\nz\n$$"]
        );
        // Currency and unmatched markers are ordinary text.
        assert!(skipped("costs $5 and $6 total, `open and $$ open").is_empty());
        assert_eq!(skipped("``a ` b`` and `c"), ["``a ` b``"]);
        assert_eq!(skipped("```\nunclosed"), ["```\nunclosed"]);
        assert_eq!(skipped("é `ü` $α$ ok"), ["`ü`", "$α$"]);
        let r = code_and_math_ranges("a `b` c");
        assert!(in_ranges(&r, 2) && in_ranges(&r, 4) && !in_ranges(&r, 1) && !in_ranges(&r, 5));
    }

    #[test]
    fn markdown_math_preserves_shell_arguments_and_code() {
        let input = "Use `$@` and `$1` with $x$.\n```sh\necho \"$@\" $$ $1\n```\n~~~sh\necho $$ $@\n~~~\n$@ and $@; $y$";
        let rendered = preprocess_markdown_view(input);
        assert!(rendered.contains("`$@` and `$1` with `x`."));
        assert!(rendered.contains("```sh\necho \"$@\" $$ $1\n```"));
        assert!(rendered.contains("~~~sh\necho $$ $@\n~~~"));
        assert!(rendered.contains("\\$@ and \\$@; `y`"));
        assert!(preprocess_markdown_math("$$x$$").contains("**`x`**"));
        assert!(preprocess_markdown_math("$@ and $@").starts_with("$@ and $@"));
        assert_eq!(skipped("$@ and $@; $x$"), ["$x$"]);
    }
}

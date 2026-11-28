/// Byte ranges of `text` that hold code (fenced blocks, inline code) or LaTeX (`$$…$$`, `$…$`).
/// Sorted and disjoint. Inline math follows the rules of [`preprocess_markdown_math`]; an
/// unclosed fence runs to the end, as in Markdown, while unclosed math is plain text.
pub fn code_and_math_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    fn line_end(text: &str, pos: usize) -> usize {
        text[pos..].find('\n').map_or(text.len(), |p| pos + p + 1)
    }
    /// End of the fenced block opening at the line starting at `start`, if there is one.
    fn fence_end(text: &str, start: usize) -> Option<usize> {
        let first_end = line_end(text, start);
        let opening = text[start..first_end].trim_start();
        let marker = opening.chars().next().filter(|c| *c == '`' || *c == '~')?;
        let run = opening.chars().take_while(|&c| c == marker).count();
        if run < 3 || (marker == '`' && opening[run..].contains('`')) {
            return None;
        }
        let mut pos = first_end;
        while pos < text.len() {
            let end = line_end(text, pos);
            let closing = text[pos..end].trim_start();
            let len = closing.chars().take_while(|&c| c == marker).count();
            if len >= run && closing[len..].trim().is_empty() {
                return Some(end);
            }
            pos = end;
        }
        Some(text.len())
    }
    /// End of the first run of exactly `run` backticks in `from..limit`.
    fn closing_backticks(bytes: &[u8], from: usize, limit: usize, run: usize) -> Option<usize> {
        let mut j = from;
        while j < limit {
            if bytes[j] == b'`' {
                let mut k = j;
                while k < limit && bytes[k] == b'`' {
                    k += 1;
                }
                if k - j == run {
                    return Some(k);
                }
                j = k;
            } else {
                j += 1;
            }
        }
        None
    }

    let bytes = text.as_bytes();
    let mut ranges = Vec::new();
    let mut i = 0;
    let mut at_line_start = true;
    while i < bytes.len() {
        if at_line_start {
            at_line_start = false;
            if let Some(end) = fence_end(text, i) {
                ranges.push(i..end);
                i = end;
                at_line_start = true;
                continue;
            }
        }
        match bytes[i] {
            b'\n' => {
                at_line_start = true;
                i += 1;
            }
            b'\\' if matches!(bytes.get(i + 1), Some(b'$' | b'\\' | b'`')) => i += 2,
            b'`' => {
                let run = bytes[i..].iter().take_while(|&&b| b == b'`').count();
                match closing_backticks(bytes, i + run, line_end(text, i), run) {
                    Some(end) => {
                        ranges.push(i..end);
                        i = end;
                    }
                    None => i += run,
                }
            }
            b'$' if text[i..].starts_with("$@") => i += 2,
            b'$' if text[i..].starts_with("$$") => match text[i + 2..].find("$$") {
                Some(p) => {
                    let end = i + 2 + p + 2;
                    ranges.push(i..end);
                    i = end;
                }
                None => i += 2,
            },
            b'$' => {
                let after = i + 1;
                let closing = text[after..line_end(text, i)]
                    .find(['$', '`'])
                    .map(|p| after + p)
                    .filter(|&p| {
                        bytes[p] == b'$'
                            && !text[p..].starts_with("$@")
                            && bytes[..p].iter().rev().take_while(|&&b| b == b'\\').count() % 2 == 0
                    });
                match closing {
                    Some(close)
                        if close > after
                            && !text[after..close].starts_with(' ')
                            && !text[after..close].ends_with(' ') =>
                    {
                        ranges.push(i..close + 1);
                        i = close + 1;
                    }
                    _ => i += 1,
                }
            }
            _ => i += 1,
        }
    }
    ranges
}

/// Whether byte offset `byte` lies inside one of the sorted, disjoint `ranges`.
pub fn in_ranges(ranges: &[std::ops::Range<usize>], byte: usize) -> bool {
    let idx = ranges.partition_point(|r| r.end <= byte);
    ranges.get(idx).is_some_and(|r| r.start <= byte)
}

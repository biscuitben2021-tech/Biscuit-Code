//! A small, dependency-free Markdown-to-terminal renderer for the assistant's
//! answers — headings, bold/italic, inline code, fenced code blocks, bullet and
//! numbered lists, blockquotes, and horizontal rules. Styling goes through the
//! `ui` module, which automatically drops ANSI when output is not a TTY, so the
//! structure (glyphs, indentation) survives in piped output without color codes.

use crate::ui;

/// Whether to render the assistant's answer as Markdown. On by default; set
/// `BISCUITS_RENDER=raw` to stream the raw text instead.
pub fn enabled() -> bool {
    !matches!(
        std::env::var("BISCUITS_RENDER").ok().as_deref(),
        Some("raw")
    )
}

/// Render Markdown source to a styled terminal string.
pub fn render(source: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut in_code = false;
    let mut code_lang = String::new();

    for line in source.lines() {
        let trimmed = line.trim_end();

        // Fenced code blocks (``` or ~~~).
        if let Some(rest) = fence(trimmed) {
            if in_code {
                in_code = false;
                code_lang.clear();
            } else {
                in_code = true;
                code_lang = rest.trim().to_string();
            }
            continue;
        }
        if in_code {
            out.push(format!("    {}", ui::grey(line)));
            continue;
        }

        // Horizontal rule.
        if is_hr(trimmed) {
            out.push(ui::grey("────────────────────"));
            continue;
        }

        // ATX headings.
        if let Some((level, text)) = heading(trimmed) {
            let rendered = inline(text);
            let styled = match level {
                1 => ui::bold(&ui::cyan(&format!("# {rendered}"))),
                2 => ui::bold(&ui::cyan(&rendered)),
                _ => ui::bold(&rendered),
            };
            out.push(styled);
            continue;
        }

        // Blockquote.
        if let Some(rest) = trimmed
            .strip_prefix("> ")
            .or_else(|| trimmed.strip_prefix(">"))
        {
            out.push(format!("{} {}", ui::grey("│"), ui::grey(&inline(rest))));
            continue;
        }

        // Bullet list.
        if let Some((indent, rest)) = bullet(line) {
            out.push(format!("{}{} {}", indent, ui::cyan("•"), inline(rest)));
            continue;
        }

        // Numbered list.
        if let Some((indent, num, rest)) = numbered(line) {
            out.push(format!(
                "{}{} {}",
                indent,
                ui::cyan(&format!("{num}.")),
                inline(rest)
            ));
            continue;
        }

        out.push(inline(line));
    }

    out.join("\n")
}

fn fence(line: &str) -> Option<&str> {
    let t = line.trim_start();
    t.strip_prefix("```").or_else(|| t.strip_prefix("~~~"))
}

fn is_hr(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 3
        && (t.chars().all(|c| c == '-')
            || t.chars().all(|c| c == '*')
            || t.chars().all(|c| c == '_'))
}

fn heading(line: &str) -> Option<(usize, &str)> {
    if !line.starts_with('#') {
        return None;
    }
    let level = line.chars().take_while(|&c| c == '#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let rest = line[level..].strip_prefix(' ')?;
    Some((level, rest))
}

fn bullet(line: &str) -> Option<(String, &str)> {
    let indent: String = line.chars().take_while(|c| *c == ' ').collect();
    let body = &line[indent.len()..];
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = body.strip_prefix(marker) {
            return Some((indent, rest));
        }
    }
    None
}

fn numbered(line: &str) -> Option<(String, String, &str)> {
    let indent: String = line.chars().take_while(|c| *c == ' ').collect();
    let body = &line[indent.len()..];
    let digits: String = body.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let rest = &body[digits.len()..];
    let rest = rest
        .strip_prefix(". ")
        .or_else(|| rest.strip_prefix(") "))?;
    Some((indent, digits, rest))
}

/// Apply inline styling: `code`, **bold**, *italic* / _italic_.
fn inline(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '`' {
            if let Some(end) = find_close(&chars, i + 1, '`') {
                let span: String = chars[i + 1..end].iter().collect();
                out.push_str(&ui::yellow(&span));
                i = end + 1;
                continue;
            }
        } else if c == '*' && i + 1 < chars.len() && chars[i + 1] == '*' {
            if let Some(end) = find_close_seq(&chars, i + 2, "**") {
                let span: String = chars[i + 2..end].iter().collect();
                out.push_str(&ui::bold(&span));
                i = end + 2;
                continue;
            }
        } else if (c == '*' || c == '_') && i + 1 < chars.len() && chars[i + 1] != c {
            if let Some(end) = find_close(&chars, i + 1, c) {
                let span: String = chars[i + 1..end].iter().collect();
                out.push_str(&ui::dim(&span));
                i = end + 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

fn find_close(chars: &[char], from: usize, close: char) -> Option<usize> {
    (from..chars.len()).find(|&j| chars[j] == close)
}

fn find_close_seq(chars: &[char], from: usize, seq: &str) -> Option<usize> {
    let seq: Vec<char> = seq.chars().collect();
    let mut j = from;
    while j + seq.len() <= chars.len() {
        if chars[j..j + seq.len()] == seq[..] {
            return Some(j);
        }
        j += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests assert on structure (which survives without color); ANSI styling is
    // disabled here because test output is not a TTY.

    #[test]
    fn headings_render_with_hash_for_h1() {
        assert_eq!(render("# Title"), "# Title");
        assert_eq!(render("## Sub"), "Sub");
        assert_eq!(render("### Small"), "Small");
    }

    #[test]
    fn bullets_become_dots_preserving_indent() {
        assert_eq!(render("- one"), "• one");
        assert_eq!(render("  - nested"), "  • nested");
        assert_eq!(render("* star"), "• star");
    }

    #[test]
    fn numbered_lists_keep_numbers() {
        assert_eq!(render("1. first"), "1. first");
        assert_eq!(render("3) third"), "3. third");
    }

    #[test]
    fn code_fence_indents_and_hides_fences() {
        let out = render("```rust\nfn main() {}\n```");
        assert_eq!(out, "    fn main() {}");
    }

    #[test]
    fn inline_code_and_bold_strip_markers() {
        // Without color the markers are removed and the inner text kept.
        assert_eq!(render("use `cargo` now"), "use cargo now");
        assert_eq!(render("**bold** text"), "bold text");
        assert_eq!(render("_em_ here"), "em here");
    }

    #[test]
    fn horizontal_rule_renders_a_line() {
        assert_eq!(render("---"), "────────────────────");
    }

    #[test]
    fn plain_paragraph_unchanged() {
        assert_eq!(render("just words"), "just words");
    }

    #[test]
    fn unterminated_inline_marker_is_literal() {
        assert_eq!(render("a * b c"), "a * b c");
        assert_eq!(render("open `code"), "open `code");
    }
}

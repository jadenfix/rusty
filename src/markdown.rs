//! Just enough markdown for a terminal: headings, bullets, quotes, rules,
//! `code`, **bold** and fenced code blocks. Rendering is per line so it works
//! while text streams in. Pipes get the raw markdown untouched.

use crate::ui;

/// Renders one complete line. `in_code` tracks fenced blocks across lines.
pub fn line(text: &str, in_code: &mut bool) -> String {
    if !ui::tty() {
        return text.to_string();
    }
    let trimmed = text.trim_start();
    let indent = &text[..text.len() - trimmed.len()];
    if let Some(lang) = trimmed.strip_prefix("```") {
        *in_code = !*in_code;
        return if *in_code {
            ui::dim(&format!("{indent}╭─ {}", if lang.trim().is_empty() { "code" } else { lang.trim() }))
        } else {
            ui::dim(&format!("{indent}╰─"))
        };
    }
    if *in_code {
        return format!("{}{}", ui::dim("│ "), ui::info(text));
    }
    if let Some(h) = trimmed.strip_prefix("### ") {
        return ui::bold(&inline_plain(h));
    }
    if let Some(h) = trimmed.strip_prefix("## ").or_else(|| trimmed.strip_prefix("# ")) {
        return ui::accent(&inline_plain(h));
    }
    if trimmed.len() >= 3 && trimmed.chars().all(|c| c == '-' || c == '*' || c == '_') {
        return ui::dim(&"─".repeat(40));
    }
    if let Some(q) = trimmed.strip_prefix("> ") {
        return format!("{indent}{} {}", ui::dim("▏"), ui::dim(&inline_plain(q)));
    }
    if let Some(rest) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
        let rest = match rest.strip_prefix("[x] ").or_else(|| rest.strip_prefix("[X] ")) {
            Some(done) => format!("{} {}", ui::ok("■"), inline(done)),
            None => match rest.strip_prefix("[ ] ") {
                Some(open) => format!("{} {}", ui::dim("□"), inline(open)),
                None => inline(rest),
            },
        };
        return format!("{indent}{} {rest}", ui::primary("•"));
    }
    if let Some((num, rest)) = trimmed.split_once(". ") {
        if !num.is_empty() && num.len() <= 3 && num.chars().all(|c| c.is_ascii_digit()) {
            return format!("{indent}{} {}", ui::primary(&format!("{num}.")), inline(rest));
        }
    }
    format!("{indent}{}", inline(trimmed))
}

/// Inline styling: `code` and **bold**.
pub fn inline(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('`') {
            if let Some(end) = after.find('`') {
                out.push_str(&ui::info(&after[..end]));
                rest = &after[end + 1..];
                continue;
            }
        }
        if let Some(after) = rest.strip_prefix("**") {
            if let Some(end) = after.find("**") {
                out.push_str(&ui::bold(&after[..end]));
                rest = &after[end + 2..];
                continue;
            }
        }
        // Step past the whole first character: a byte offset of 1 splits `✅`.
        let first = rest.chars().next().map_or(1, char::len_utf8);
        let next = rest[first..].find(['`', '*']).map_or(rest.len(), |i| i + first);
        out.push_str(&rest[..next]);
        rest = &rest[next..];
    }
    out
}

/// Strips inline markers for text that is styled as a whole (headings, quotes).
fn inline_plain(s: &str) -> String {
    s.replace("**", "").replace('`', "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_keeps_text_when_unstyled() {
        // Tests run without a TTY, so `line` passes text through untouched;
        // `inline` must never drop characters either way.
        assert_eq!(line("- a `b` **c**", &mut false), "- a `b` **c**");
        let s = inline("x * y `z` and **w** end*");
        for part in ["x * y ", "z", " and ", "w", " end*"] {
            assert!(s.contains(part), "{part:?} missing from {s:?}");
        }
    }

    #[test]
    fn inline_handles_lines_that_start_with_wide_characters() {
        for text in ["✅ tests pass", "→ next", "文 `code` 字", "`a`é **b**", "é"] {
            let s = inline(text);
            let plain: String = s.chars().filter(|c| !c.is_control()).collect();
            for word in text.split(['`', '*', ' ']).filter(|w| !w.is_empty()) {
                assert!(plain.contains(word), "{word:?} missing from {s:?}");
            }
        }
    }
}

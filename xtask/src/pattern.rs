//! The handful of regular-expression shapes the terminal checks wait for,
//! matched the way a backtracking regex engine would (greedy, leftmost).

#[derive(Clone, Copy, Debug)]
pub enum Piece {
    /// Literal text.
    Lit(&'static str),
    /// `[^\n]*`
    Line,
    /// `\d+`
    Digits,
    /// `\s`
    Space,
    /// `(a|b|c)`
    Either(&'static [&'static str]),
}

/// The byte range of the leftmost match of `pat` in `text`.
pub fn find(pat: &[Piece], text: &str) -> Option<(usize, usize)> {
    let mut starts: Box<dyn Iterator<Item = usize>> = match pat.first() {
        Some(Piece::Lit(lit)) => Box::new(text.match_indices(lit).map(|(i, _)| i)),
        _ => Box::new(text.char_indices().map(|(i, _)| i).chain([text.len()])),
    };
    starts.find_map(|i| at(pat, text, i).map(|end| (i, end)))
}

fn at(pat: &[Piece], text: &str, pos: usize) -> Option<usize> {
    let Some((first, rest)) = pat.split_first() else { return Some(pos) };
    let s = &text[pos..];
    let then = |len: usize| at(rest, text, pos + len);
    match *first {
        Piece::Lit(lit) => s.starts_with(lit).then(|| then(lit.len())).flatten(),
        Piece::Either(alts) => alts.iter().find_map(|a| s.starts_with(a).then(|| then(a.len())).flatten()),
        Piece::Space => s.chars().next().filter(|c| c.is_whitespace()).and_then(|c| then(c.len_utf8())),
        Piece::Line => {
            let end = s.find('\n').unwrap_or(s.len());
            s[..end].char_indices().map(|(i, _)| i).chain([end]).rev().find_map(then)
        }
        Piece::Digits => {
            let n = s.bytes().take_while(u8::is_ascii_digit).count();
            (1..=n).rev().find_map(then)
        }
    }
}

/// Removes `ESC [ params letter` sequences.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("\x1b[") {
        out.push_str(&rest[..i]);
        let tail = &rest[i + 2..];
        let params = tail.bytes().take_while(|b| b.is_ascii_digit() || *b == b';' || *b == b'?').count();
        match tail.as_bytes().get(params) {
            Some(b) if b.is_ascii_alphabetic() => rest = &tail[params + 1..],
            _ => {
                out.push_str("\x1b[");
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::Piece::*;
    use super::*;

    #[test]
    fn matches_like_a_regex() {
        let text = "a ✓ 3s · 1.2k ctx · 9 ctx\n✓ 12s";
        assert_eq!(find(&[Lit("✓ "), Line, Lit("ctx")], text), Some((2, 29)));
        assert_eq!(find(&[Lit("✓ "), Digits, Lit("s")], text), Some((2, 8)));
        assert_eq!(find(&[Lit("goal "), Either(&["done", "blocked"])], "goal blocked"), Some((0, 12)));
        assert_eq!(find(&[Lit("pong"), Space], "pong\r\n"), Some((0, 5)));
        assert_eq!(find(&[Lit("pong"), Space], "pong"), None);
        assert_eq!(find(&[Lit("model "), Line, Lit("→")], "model a\n→"), None);
    }

    #[test]
    fn strips_csi() {
        assert_eq!(strip_ansi("\x1b[38;5;102m✓\x1b[0m 3s\x1b[?2004h\x1b[1!"), "✓ 3s\x1b[1!");
    }
}

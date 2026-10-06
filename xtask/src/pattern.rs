//! The handful of regular-expression shapes the terminal checks wait for,
//! matched the way a backtracking regex engine would (greedy, leftmost).

#[derive(Clone, Copy, Debug)]
pub enum Piece {
    /// Literal text.
    Lit(&'static str),
    /// `\s`
    Space,
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
        Piece::Space => s.chars().next().filter(|c| c.is_whitespace()).and_then(|c| then(c.len_utf8())),
    }
}

#[cfg(test)]
mod tests {
    use super::Piece::*;
    use super::*;

    #[test]
    fn matches_like_a_regex() {
        assert_eq!(find(&[Lit("pong"), Space], "word pong\r\n"), Some((5, 10)));
        assert_eq!(find(&[Lit("pong"), Space], "pong\x1b[?2004l"), None);
    }
}

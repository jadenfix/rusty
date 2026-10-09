//! Lexical retrieval for the memory advisor: an identifier-aware tokenizer
//! and a BM25 inverted index.
//!
//! Lessons and queries are short and code-heavy: `parseConfig`,
//! `gen_fixtures`, `src/billing/tax.rs`, an error line. Each identifier is
//! kept whole (exact-name matches) and also split into its camelCase,
//! snake_case and path parts, so "tax rate" finds `taxRate`. Emitting both,
//! rather than parts alone, is what made BM25 competitive on code retrieval
//! in arXiv 2605.18561. Plain words get a light suffix stemmer so "tests"
//! meets "test" and "rounding" meets "round". BM25 still can't bridge
//! synonyms ("tests" and `pytest`); the agent's own `recall` searches cover
//! that gap.

use std::collections::{HashMap, HashSet};

const K1: f64 = 1.2;
const B: f64 = 0.75;

const STOP: &[&str] = &[
    "the", "and", "for", "that", "this", "with", "you", "are", "was", "but", "not", "have", "has", "can", "from",
    "into", "use", "will", "should", "when", "what", "how", "why", "all", "any", "our", "its", "then", "than", "also",
    "just", "please", "make", "let", "lets", "get", "them", "they", "there", "here", "out", "per", "via", "such", "is",
    "it", "to", "of", "in", "on", "an", "be", "or", "as", "at", "by", "if", "so", "do", "we", "me", "my", "no", "up",
    "us",
];

/// Splits `HTTPServerError2` into `http`, `server`, `error2`.
fn camel_parts(word: &str) -> Vec<String> {
    let chars: Vec<char> = word.chars().collect();
    let mut parts = Vec::new();
    let mut start = 0;
    for i in 1..chars.len() {
        let (prev, cur) = (chars[i - 1], chars[i]);
        let next_lower = chars.get(i + 1).is_some_and(|c| c.is_lowercase());
        if (prev.is_lowercase() && cur.is_uppercase()) || (prev.is_uppercase() && cur.is_uppercase() && next_lower) {
            parts.push(chars[start..i].iter().collect::<String>().to_lowercase());
            start = i;
        }
    }
    parts.push(chars[start..].iter().collect::<String>().to_lowercase());
    parts
}

/// Strips common English inflections from a lowercase ASCII word. It only
/// has to be consistent, since queries and lessons go through it alike.
fn stem(word: &str) -> String {
    if !word.bytes().all(|b| b.is_ascii_lowercase()) || word.len() < 4 {
        return word.to_string();
    }
    let undouble = |s: &str| {
        let b = s.as_bytes();
        let n = b.len();
        if n > 2 && b[n - 1] == b[n - 2] && !b"aeioulsz".contains(&b[n - 1]) {
            s[..n - 1].to_string()
        } else {
            s.to_string()
        }
    };
    if let Some(s) = word.strip_suffix("ies").filter(|s| s.len() > 1) {
        return format!("{s}y");
    }
    // As in Porter's stemmer, the stem must keep a vowel: "string" stays.
    let vowel = |s: &&str| s.len() > 2 && s.bytes().any(|b| b"aeiouy".contains(&b));
    if let Some(s) = word.strip_suffix("ing").filter(vowel) {
        return undouble(s);
    }
    if let Some(s) = word.strip_suffix("ed").filter(vowel) {
        return undouble(s);
    }
    if let Some(s) = word.strip_suffix("es").filter(|s| ["s", "x", "z", "ch", "sh"].iter().any(|e| s.ends_with(e))) {
        return s.to_string();
    }
    match word.strip_suffix('s') {
        Some(s) if !s.ends_with(['s', 'u', 'i']) => s.to_string(),
        _ => word.to_string(),
    }
}

/// Index terms for `text`: each identifier lowercased whole, plus its parts
/// when it has more than one, with plain words stemmed. Stopwords and
/// one-letter tokens are dropped.
pub fn terms(text: &str) -> Vec<String> {
    let keep = |t: &str| t.chars().count() > 1 && !STOP.contains(&t);
    let mut out = Vec::new();
    for raw in text.split(|c: char| !c.is_alphanumeric() && c != '_') {
        let whole = raw.trim_matches('_').to_lowercase();
        if whole.is_empty() {
            continue;
        }
        let parts: Vec<String> = raw.split('_').filter(|p| !p.is_empty()).flat_map(camel_parts).collect();
        if keep(&whole) {
            out.push(stem(&whole));
        }
        if parts.len() > 1 {
            out.extend(parts.into_iter().filter(|p| keep(p) && *p != whole).map(|p| stem(&p)));
        }
    }
    out
}

/// The distinct terms of `text`, as a query.
pub fn query(text: &str) -> HashSet<String> {
    terms(text).into_iter().collect()
}

/// BM25 over a small set of documents, as an inverted index. Each posting
/// holds its finished term weight, so a search only touches the documents
/// that share a term with the query.
pub struct Index {
    postings: HashMap<String, Vec<(usize, f64)>>,
    len: usize,
}

impl Index {
    pub fn new(docs: &[Vec<String>]) -> Self {
        let mut tfs: HashMap<&str, Vec<(usize, f64)>> = HashMap::new();
        for (i, d) in docs.iter().enumerate() {
            let mut tf: HashMap<&str, f64> = HashMap::new();
            for t in d {
                *tf.entry(t.as_str()).or_insert(0.0) += 1.0;
            }
            for (t, n) in tf {
                tfs.entry(t).or_default().push((i, n));
            }
        }
        let lens: Vec<f64> = docs.iter().map(|d| d.len() as f64).collect();
        let avg = (lens.iter().sum::<f64>() / lens.len().max(1) as f64).max(1.0);
        let n = docs.len() as f64;
        let postings = tfs
            .into_iter()
            .map(|(t, list)| {
                // Smoothed IDF, ln((1 + N) / (1 + df)) + 1, as scikit-learn's
                // TF-IDF uses: a term every lesson shares still counts once.
                // Okapi's IDF falls to nearly zero there, so a store of
                // similar lessons could never match anything.
                let idf = ((1.0 + n) / (1.0 + list.len() as f64)).ln() + 1.0;
                let list = list
                    .into_iter()
                    .map(|(d, tf)| (d, idf * tf * (K1 + 1.0) / (tf + K1 * (1.0 - B + B * lens[d] / avg))))
                    .collect();
                (t.to_string(), list)
            })
            .collect();
        Self { postings, len: docs.len() }
    }

    /// Every document's score for `query` (zero for those sharing no term).
    pub fn scores(&self, query: &HashSet<String>) -> Vec<f64> {
        let mut out = vec![0.0; self.len];
        for term in query {
            for (doc, w) in self.postings.get(term).into_iter().flatten() {
                out[*doc] += w;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(text: &str) -> HashSet<String> {
        query(text)
    }

    #[test]
    fn identifiers_are_kept_whole_and_split() {
        assert_eq!(terms("parseConfig"), ["parseconfig", "parse", "config"]);
        assert_eq!(terms("gen_fixtures.py"), ["gen_fixtures", "gen", "fixture", "py"]);
        assert_eq!(terms("HTTPServer"), ["httpserver", "http", "server"]);
        assert_eq!(terms("src/billing/tax_rate.rs"), ["src", "bill", "tax_rate", "tax", "rate", "rs"]);
        // Stopwords and single letters go; plain words are stemmed.
        assert_eq!(terms("Run the tests with make check, a must"), ["run", "test", "check", "must"]);
        assert_eq!(terms("__init__"), ["init"]);
    }

    #[test]
    fn stemming_joins_inflections_without_mangling_code() {
        for (a, b) in [
            ("tests", "test"),
            ("rounding", "round"),
            ("rounded", "round"),
            ("running", "run"),
            ("stopped", "stop"),
            ("fixes", "fix"),
            ("matches", "match"),
            ("dependencies", "dependency"),
        ] {
            assert_eq!(stem(a), b);
        }
        for same in ["class", "status", "analysis", "pytest", "go", "tsx", "v2", "readme", "string", "thing"] {
            assert_eq!(stem(same), same);
        }
    }

    #[test]
    fn search_touches_only_matching_documents() {
        let docs = vec![terms("taxRate rounds half up in billing"), terms("the cache evicts oldest entries")];
        let index = Index::new(&docs);
        let s = index.scores(&q("fix the tax rate rounding"));
        assert!(s[0] > 0.0);
        assert_eq!(s[1], 0.0);
    }

    #[test]
    fn rare_terms_weigh_more() {
        let docs = vec![terms("tests run with make check"), terms("tests need the fixtures generated")];
        let index = Index::new(&docs);
        // "fixtures" is in one document, "tests" in both.
        assert!(index.scores(&q("fixtures"))[1] > index.scores(&q("tests"))[1]);
        // A term every document shares still counts.
        assert!(index.scores(&q("tests"))[0] > 0.5);
    }

    #[test]
    fn a_term_counts_more_in_a_larger_store() {
        let lesson = terms("regenerate fixtures before running tests");
        let one = Index::new(std::slice::from_ref(&lesson));
        let mut docs = vec![lesson];
        docs.extend((0..20).map(|i| terms(&format!("deployment note {i}"))));
        let many = Index::new(&docs);
        assert!(many.scores(&q("fixtures"))[0] > one.scores(&q("fixtures"))[0] * 2.0);
        assert_eq!(Index::new(&[]).scores(&q("anything")), Vec::<f64>::new());
    }
}

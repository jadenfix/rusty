//! Long-term memory: small, typed facts with relevance-ranked recall.
//!
//! This is not a notes file that gets pasted into every prompt. Each memory is
//! a record (kind, scope, text, usage stats) in a JSONL file. Before every
//! turn rusty ranks memories against what you asked (BM25 over the text,
//! nudged by kind, recency and how often a memory proved useful) and injects
//! only the top few within a fixed token budget. Preferences are always
//! included because they apply everywhere. The model can also search, save
//! and delete memories itself through tools.
//!
//! Project memories live under `~/.config/rusty/projects/<id>/memory.jsonl`,
//! global ones under `~/.config/rusty/memory.jsonl`.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    /// preference | fact | decision | gotcha | todo
    pub kind: String,
    pub text: String,
    #[serde(default)]
    pub global: bool,
    pub created: u64,
    #[serde(default)]
    pub last_used: u64,
    #[serde(default)]
    pub uses: u32,
}

pub const KINDS: &[&str] = &["preference", "fact", "decision", "gotcha", "todo"];

pub struct Memory {
    pub entries: Vec<Entry>,
    project_path: Option<PathBuf>,
    global_path: Option<PathBuf>,
    dirty: bool,
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// FNV-1a; stable across Rust versions, unlike `DefaultHasher`.
pub fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

const STOP: &[&str] = &[
    "the", "and", "for", "that", "this", "with", "you", "are", "was", "but", "not", "have", "has", "can", "from",
    "into", "use", "will", "should", "when", "what", "how", "why", "all", "any", "our", "its", "it's", "then", "than",
    "also", "just", "please", "make", "let", "lets", "get",
];

fn terms(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|w| w.len() > 1 && !STOP.contains(w))
        .map(String::from)
        .collect()
}

impl Memory {
    pub fn empty() -> Self {
        Self { entries: Vec::new(), project_path: None, global_path: None, dirty: false }
    }

    pub fn load(project_path: PathBuf, global_path: PathBuf) -> Self {
        let mut entries = Vec::new();
        for (path, global) in [(&global_path, true), (&project_path, false)] {
            if let Ok(text) = std::fs::read_to_string(path) {
                for line in text.lines() {
                    if let Ok(mut e) = serde_json::from_str::<Entry>(line) {
                        e.global = global;
                        entries.push(e);
                    }
                }
            }
        }
        Self { entries, project_path: Some(project_path), global_path: Some(global_path), dirty: false }
    }

    pub fn save(&mut self) {
        if !self.dirty {
            return;
        }
        for (path, global) in [(&self.global_path, true), (&self.project_path, false)] {
            let Some(path) = path else { continue };
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let body: String = self
                .entries
                .iter()
                .filter(|e| e.global == global)
                .filter_map(|e| {
                    let mut value = serde_json::to_value(e).ok()?;
                    rusty::privacy::scrub(&mut value);
                    serde_json::to_string(&value).ok()
                })
                .map(|l| l + "\n")
                .collect();
            use std::io::Write;
            if let Ok(mut f) = rusty::privacy::private_file(path, false) {
                let _ = f.write_all(body.as_bytes());
            }
        }
        self.dirty = false;
    }

    /// Saves a memory. Near-duplicates update the existing entry instead.
    pub fn add(&mut self, kind: &str, text: &str, global: bool) -> String {
        let kind = if KINDS.contains(&kind) { kind } else { "fact" };
        let cleaned = rusty::privacy::redact(text);
        let text = cleaned.0.trim();
        let new_terms: HashSet<String> = terms(text).into_iter().collect();
        for e in &mut self.entries {
            let old: HashSet<String> = terms(&e.text).into_iter().collect();
            let inter = new_terms.intersection(&old).count() as f64;
            let union = new_terms.union(&old).count().max(1) as f64;
            if inter / union > 0.75 {
                e.text = text.to_string();
                e.kind = kind.to_string();
                e.last_used = now();
                self.dirty = true;
                return format!("updated memory {}", e.id);
            }
        }
        let id = format!("{:06x}", fnv(&format!("{text}{}", now())) & 0xffffff);
        self.entries.push(Entry {
            id: id.clone(),
            kind: kind.to_string(),
            text: text.to_string(),
            global,
            created: now(),
            last_used: now(),
            uses: 0,
        });
        self.dirty = true;
        format!("saved memory {id}")
    }

    pub fn forget(&mut self, id: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.id != id);
        self.dirty |= self.entries.len() != before;
        self.entries.len() != before
    }

    pub fn clear_project(&mut self) {
        self.entries.retain(|e| e.global);
        self.dirty = true;
    }

    /// Ranks memories against `query` with BM25 plus small priors.
    pub fn search(&self, query: &str, k: usize) -> Vec<(f64, &Entry)> {
        let q = terms(query);
        if q.is_empty() || self.entries.is_empty() {
            return Vec::new();
        }
        let docs: Vec<Vec<String>> = self.entries.iter().map(|e| terms(&format!("{} {}", e.kind, e.text))).collect();
        let n = docs.len() as f64;
        let avg = docs.iter().map(|d| d.len()).sum::<usize>() as f64 / n;
        let mut df: HashMap<&str, f64> = HashMap::new();
        for d in &docs {
            for t in d.iter().collect::<HashSet<_>>() {
                *df.entry(t.as_str()).or_default() += 1.0;
            }
        }
        let now = now();
        let mut scored: Vec<(f64, &Entry)> = self
            .entries
            .iter()
            .zip(&docs)
            .filter_map(|(e, d)| {
                let len = d.len() as f64;
                let mut score = 0.0;
                for t in &q {
                    let tf = d.iter().filter(|w| *w == t).count() as f64;
                    if tf == 0.0 {
                        // Cheap prefix match so "test" finds "tests".
                        if d.iter().any(|w| {
                            w.len() > 3 && t.len() > 3 && (w.starts_with(t.as_str()) || t.starts_with(w.as_str()))
                        }) {
                            score += 0.3;
                        }
                        continue;
                    }
                    let idf = ((n - df.get(t.as_str()).copied().unwrap_or(0.0) + 0.5)
                        / (df.get(t.as_str()).copied().unwrap_or(0.0) + 0.5)
                        + 1.0)
                        .ln();
                    score += idf * tf * 2.2 / (tf + 1.2 * (0.25 + 0.75 * len / avg.max(1.0)));
                }
                if score <= 0.0 {
                    return None;
                }
                let age_days = now.saturating_sub(e.last_used.max(e.created)) as f64 / 86400.0;
                let prior = match e.kind.as_str() {
                    "gotcha" | "decision" => 1.15,
                    _ => 1.0,
                } * (1.0 + 0.05 * (e.uses.min(10) as f64))
                    * (1.0 / (1.0 + age_days / 90.0)).max(0.5);
                Some((score * prior, e))
            })
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(k);
        scored
    }

    /// Memory block for the system prompt, kept under `budget_chars`.
    /// Returns the block and the ids it used (so their usage can be bumped).
    pub fn context_block(&self, query: &str, budget_chars: usize) -> (String, Vec<String>) {
        let mut out = String::new();
        let mut ids = Vec::new();
        let mut push = |e: &Entry, out: &mut String| {
            let line = format!("- [{}:{}] {}\n", e.kind, e.id, e.text);
            if out.len() + line.len() <= budget_chars && !ids.contains(&e.id) {
                out.push_str(&line);
                ids.push(e.id.clone());
            }
        };
        for e in self.entries.iter().filter(|e| e.kind == "preference") {
            push(e, &mut out);
        }
        for (_, e) in self.search(query, 8) {
            push(e, &mut out);
        }
        (out, ids)
    }

    pub fn touch(&mut self, ids: &[String]) {
        let t = now();
        for e in self.entries.iter_mut().filter(|e| ids.contains(&e.id)) {
            e.uses = e.uses.saturating_add(1);
            e.last_used = t;
            self.dirty = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_relevant_memories_first() {
        let mut m = Memory::empty();
        m.add("fact", "integration tests need the postgres container running (make db-up)", false);
        m.add("preference", "user prefers small focused commits", true);
        m.add("gotcha", "the build script regenerates src/schema.rs; never edit it by hand", false);
        let hits = m.search("why do the integration tests fail with connection refused", 3);
        assert!(hits[0].1.text.contains("postgres"));
        let hits = m.search("edit schema.rs", 3);
        assert!(hits[0].1.text.contains("schema"));
    }

    #[test]
    fn dedupes_near_duplicates() {
        let mut m = Memory::empty();
        m.add("fact", "run tests with cargo nextest run", false);
        let r = m.add("fact", "run the tests with cargo nextest run", false);
        assert!(r.starts_with("updated"));
        assert_eq!(m.entries.len(), 1);
    }

    #[test]
    fn preferences_always_in_context() {
        let mut m = Memory::empty();
        m.add("preference", "never use emojis in commit messages", true);
        let (block, ids) = m.context_block("unrelated question about parsing", 1000);
        assert!(block.contains("emojis"));
        assert_eq!(ids.len(), 1);
    }
}

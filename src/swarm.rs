//! What a swarm's workers share while they run: a board of one-line findings.
//! Each worker sees what its peers posted since its last step, so the swarm
//! builds on its own discoveries instead of repeating them.
use std::sync::{Arc, Mutex};

/// Most tasks one swarm call runs; the rest are reported as not run.
pub const MAX_JOBS: usize = 32;
const MAX_NOTES: usize = 200;
const NOTE_CHARS: usize = 300;

#[derive(Clone, Default)]
pub struct Board(Arc<Mutex<Vec<Note>>>);

struct Note {
    author: usize,
    label: String,
    text: String,
}

impl Board {
    /// Posts one finding. Duplicates and posts past the cap are dropped.
    pub fn post(&self, author: usize, label: &str, text: &str) -> &'static str {
        let text: String = text.trim().chars().take(NOTE_CHARS).collect();
        if text.is_empty() {
            return "nothing to share";
        }
        let mut notes = self.0.lock().unwrap();
        if notes.iter().any(|n| n.text == text) {
            return "already shared by a worker";
        }
        if notes.len() >= MAX_NOTES {
            return "the board is full; put it in your report";
        }
        notes.push(Note { author, label: label.to_string(), text });
        "shared with the other workers"
    }

    /// Peers' notes posted after `seen`, which then moves to the end.
    pub fn unseen(&self, me: usize, seen: &mut usize) -> Vec<String> {
        let notes = self.0.lock().unwrap();
        let fresh = notes[*seen..].iter().filter(|n| n.author != me).map(|n| format!("[{}] {}", n.label, n.text));
        let fresh = fresh.collect();
        *seen = notes.len();
        fresh
    }

    /// Every note, for the lead.
    pub fn summary(&self) -> String {
        let notes = self.0.lock().unwrap();
        notes.iter().map(|n| format!("- [{}] {}", n.label, n.text)).collect::<Vec<_>>().join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workers_see_only_new_notes_from_peers() {
        let b = Board::default();
        let (mut seen_a, mut seen_b) = (0, 0);
        assert_eq!(b.post(0, "auth", "token check skipped at api.rs:42"), "shared with the other workers");
        assert_eq!(b.post(1, "db", "token check skipped at api.rs:42"), "already shared by a worker");
        b.post(1, "db", "migration 7 drops an index");
        assert_eq!(b.unseen(0, &mut seen_a), vec!["[db] migration 7 drops an index"]);
        assert!(b.unseen(0, &mut seen_a).is_empty());
        assert_eq!(b.unseen(1, &mut seen_b), vec!["[auth] token check skipped at api.rs:42"]);
        assert_eq!(b.summary().lines().count(), 2);
        assert_eq!(b.post(2, "x", "  "), "nothing to share");
    }

    #[test]
    fn notes_are_bounded() {
        let b = Board::default();
        for i in 0..MAX_NOTES {
            b.post(0, "w", &format!("finding {i}"));
        }
        assert_eq!(b.post(1, "w", "one more"), "the board is full; put it in your report");
        b.post(0, "w", &"x".repeat(1000));
        assert!(b.summary().lines().all(|l| l.chars().count() < NOTE_CHARS + 20));
    }
}

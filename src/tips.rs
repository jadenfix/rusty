//! Token tips: at most one short, specific suggestion after a turn, based on
//! what that turn actually spent. Each tip shows once per session.

use std::collections::HashSet;

use crate::display::TurnStats;

pub struct Facts<'a> {
    pub stats: &'a TurnStats,
    pub prompt_last: u64,
    pub window: usize,
    pub completion: u64,
    pub session_total: u64,
    pub delegation_off: bool,
    pub model: &'a str,
}

pub fn pick(f: &Facts, shown: &mut HashSet<&'static str>) -> Option<String> {
    let pct = (f.prompt_last as usize * 100) / f.window.max(1);
    let visible_tokens = (f.stats.visible_chars / 4) as u64;
    let reread = f.stats.file_reads.iter().max_by_key(|(_, n)| **n).filter(|(_, n)| **n >= 3);
    let candidates: Vec<(&'static str, Option<String>)> = vec![
        (
            "context",
            (pct >= 50).then(|| {
                format!("context is {pct}% full. /compact keeps the gist; /clear is cheaper if the next task is unrelated.")
            }),
        ),
        (
            "big-read",
            (f.stats.biggest.1 > 6_000).then(|| {
                format!(
                    "one result ({}) cost ~{}k tokens. Naming the function or file you care about lets rusty search and read a range instead.",
                    f.stats.biggest.0,
                    f.stats.biggest.1 / 1000
                )
            }),
        ),
        (
            "reread",
            reread.map(|(file, n)| {
                format!("{file} was read {n} times this turn, usually a sign the context is crowded. /compact can help.")
            }),
        ),
        (
            "explore",
            (f.delegation_off && f.stats.reads + f.stats.searches >= 15).then(|| {
                "lots of exploring this turn. /agents auto lets a subagent dig and hand back a summary instead of filling your context.".to_string()
            }),
        ),
        (
            "reasoning",
            (f.completion > 4_000 && f.completion > visible_tokens * 6 && !f.model.contains("super")).then(|| {
                "most output tokens were hidden reasoning. For routine edits a lighter model is cheaper: /model nvidia/nemotron-3-super-120b-a12b".to_string()
            }),
        ),
        (
            "session",
            (f.session_total > 1_000_000).then(|| {
                "this session has moved over 1M tokens. /clear between unrelated tasks keeps every request smaller.".to_string()
            }),
        ),
    ];
    for (key, tip) in candidates {
        if let Some(tip) = tip {
            if shown.insert(key) {
                return Some(tip);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_each_tip_once() {
        let stats = TurnStats::default();
        let f = Facts {
            stats: &stats,
            prompt_last: 90_000,
            window: 128_000,
            completion: 10,
            session_total: 0,
            delegation_off: true,
            model: "m",
        };
        let mut shown = HashSet::new();
        assert!(pick(&f, &mut shown).unwrap().contains("context is 70% full"));
        assert!(pick(&f, &mut shown).is_none());
    }
}

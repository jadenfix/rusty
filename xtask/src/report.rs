//! Summarises eval runs: one row per model (or model and memory setting, when
//! rows pin one), then one row per task across them. Reads one or more target/evals/*.jsonl files (rows from several
//! runs are pooled), prints the tables, writes them next to the first file as
//! .md, and appends them to $GITHUB_STEP_SUMMARY when set. Fails unless every
//! scored run passed.
//!
//!     cargo xtask eval-report target/evals/20261006-190000.jsonl [more.jsonl ...]

use std::io::Write;

use anyhow::{bail, Context, Result};
use serde_json::Value;

pub const USAGE: &str = "usage: cargo xtask eval-report target/evals/STAMP.jsonl [more.jsonl ...]";

/// Prints and writes the report; true when every scored run passed.
pub fn run(paths: &[String]) -> Result<bool> {
    if paths.is_empty() {
        bail!("{USAGE}");
    }
    let rows = load(paths)?;
    if rows.is_empty() {
        bail!("no eval rows");
    }
    let text = report(&rows);
    println!("\n{text}");
    let first = &paths[0];
    let md = match first.strip_suffix(".jsonl") {
        Some(stem) => format!("{stem}.md"),
        None => format!("{first}.md"),
    };
    std::fs::write(&md, format!("{text}\n")).with_context(|| format!("writing {md}"))?;
    if let Some(summary) = std::env::var_os("GITHUB_STEP_SUMMARY").filter(|s| !s.is_empty()) {
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&summary)?;
        write!(f, "## rusty evals\n\n{text}\n")?;
    }
    Ok(all_passed(&rows))
}

pub fn load(paths: &[String]) -> Result<Vec<Value>> {
    let mut rows = Vec::new();
    for p in paths {
        let text = std::fs::read_to_string(p).with_context(|| format!("reading {p}"))?;
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            rows.push(serde_json::from_str(line).with_context(|| format!("{p}: bad row {line}"))?);
        }
    }
    Ok(rows)
}

/// At least one scored run, and every scored run passed.
pub fn all_passed(rows: &[Value]) -> bool {
    let scored: Vec<_> = rows.iter().filter(|r| verdict(r) != "infra").collect();
    !scored.is_empty() && scored.iter().all(|r| verdict(r) == "pass")
}

pub fn mark(verdict: &str) -> &'static str {
    match verdict {
        "pass" => "✓",
        "fail" => "✗",
        "timeout" => "⧗",
        "infra" => "⚠",
        "budget" => "⊘",
        _ => "?",
    }
}

fn text(r: &Value, key: &str) -> String {
    match &r[key] {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        v => v.to_string(),
    }
}

/// What a row is compared by: its model, plus its memory setting if pinned.
fn key(r: &Value) -> String {
    match text(r, "memory") {
        m if m.is_empty() => text(r, "model"),
        m => format!("{} · {m}", text(r, "model")),
    }
}

fn verdict(r: &Value) -> &str {
    r["verdict"].as_str().unwrap_or("")
}

fn num(r: &Value, key: &str) -> f64 {
    r[key].as_f64().unwrap_or(0.0)
}

fn rep(r: &Value) -> i64 {
    r.get("rep").and_then(Value::as_i64).unwrap_or(1)
}

/// The `reason` lines joined by spaces, or None when there are none.
fn reason(r: &Value) -> Option<String> {
    let lines: Vec<String> =
        r["reason"].as_array()?.iter().map(|v| v.as_str().map_or(v.to_string(), str::to_string)).collect();
    (!lines.is_empty()).then(|| lines.join(" "))
}

fn prefix(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

pub fn report(rows: &[Value]) -> String {
    let mut models: Vec<String> = Vec::new();
    for r in rows {
        let m = key(r);
        if !models.contains(&m) {
            models.push(m);
        }
    }
    let mut tasks: Vec<String> = rows.iter().map(|r| text(r, "task")).collect();
    tasks.sort();
    tasks.dedup();

    let mut lines = vec![
        "| model | passed | pass rate | timeouts | infra (not scored) | tokens | agent time |".to_string(),
        "|---|---|---|---|---|---|---|".to_string(),
    ];
    for m in &models {
        let mine: Vec<&Value> = rows.iter().filter(|r| &key(r) == m).collect();
        let scored = mine.iter().filter(|r| verdict(r) != "infra").count();
        let passed = mine.iter().filter(|r| verdict(r) == "pass").count();
        let tok: f64 = mine.iter().map(|r| num(r, "prompt") + num(r, "completion")).sum();
        let rate = if scored > 0 { format!("{:.0}%", 100.0 * passed as f64 / scored as f64) } else { "–".into() };
        let timeouts = mine.iter().filter(|r| verdict(r) == "timeout").count();
        let infra = mine.iter().filter(|r| verdict(r) == "infra").count();
        let secs: f64 = mine.iter().map(|r| num(r, "secs")).sum();
        lines.push(format!(
            "| `{m}` | {passed}/{scored} | {rate} | {timeouts} | {infra} | {:.0}k | {:.1} min |",
            tok / 1000.0,
            secs / 60.0
        ));
    }

    lines.push(String::new());
    let header: Vec<String> = models.iter().map(|m| format!("`{m}`")).collect();
    lines.push(format!("| task | {} |", header.join(" | ")));
    lines.push(format!("|---|{}", "---|".repeat(models.len())));
    for t in &tasks {
        let cells: Vec<String> = models
            .iter()
            .map(|m| {
                let mut runs: Vec<&Value> = rows.iter().filter(|r| &key(r) == m && &text(r, "task") == t).collect();
                if runs.is_empty() {
                    return String::new();
                }
                runs.sort_by_key(|r| rep(r));
                let scored = runs.iter().filter(|r| verdict(r) != "infra").count();
                let passed = runs.iter().filter(|r| verdict(r) == "pass").count();
                let marks: String = runs.iter().map(|r| mark(verdict(r))).collect();
                format!("{passed}/{scored} {marks}")
            })
            .collect();
        lines.push(format!("| {t} | {} |", cells.join(" | ")));
    }

    // With repeats, a task counts for pass@k when any attempt passed and for
    // pass^k only when every attempt did: the second is what a deployed agent
    // that gets one try would see; best-of-k picked with the hidden grader is not.
    if tasks
        .iter()
        .any(|t| models.iter().any(|m| rows.iter().filter(|r| &key(r) == m && &text(r, "task") == t).count() > 1))
    {
        lines.push(String::new());
        lines.push("| model | tasks | pass@k | pass^k |".into());
        lines.push("|---|---|---|---|".into());
        for m in &models {
            let (mut n, mut any, mut all) = (0, 0, 0);
            for t in &tasks {
                let runs: Vec<&str> = rows
                    .iter()
                    .filter(|r| &key(r) == m && &text(r, "task") == t && verdict(r) != "infra")
                    .map(verdict)
                    .collect();
                if runs.is_empty() {
                    continue;
                }
                n += 1;
                any += runs.contains(&"pass") as usize;
                all += runs.iter().all(|v| *v == "pass") as usize;
            }
            lines.push(format!("| `{m}` | {n} | {any}/{n} | {all}/{n} |"));
        }
    }

    // Claims are the runtime's view, grading the independent one; they sit
    // side by side so a gate that refuses everything can't pass for honesty.
    if rows.iter().any(|r| r["claims"].as_array().is_some_and(|c| !c.is_empty())) {
        let claims = |r: &Value, f: &str| {
            r["claims"].as_array().into_iter().flatten().filter_map(|c| c[f].as_u64()).sum::<u64>()
        };
        lines.push(String::new());
        lines.push("| model | claims proposed | accepted by runtime | accepted, graded fail | graded pass |".into());
        lines.push("|---|---|---|---|---|".into());
        for m in &models {
            let mine: Vec<&Value> = rows.iter().filter(|r| &key(r) == m && verdict(r) != "infra").collect();
            let sum = |f: &str| mine.iter().map(|r| claims(r, f)).sum::<u64>();
            let count = |f: &str| mine.iter().filter(|r| r[f] == true).count();
            lines.push(format!(
                "| `{m}` | {} | {} | {} | {}/{} |",
                sum("proposed"),
                sum("accepted"),
                count("false_completion"),
                count("functional_pass"),
                mine.len()
            ));
        }
    }

    let mut causes: Vec<(String, usize)> = Vec::new();
    for r in rows.iter().filter(|r| verdict(r) == "infra") {
        let why = prefix(&reason(r).unwrap_or_else(|| "(no message)".into()), 120);
        match causes.iter_mut().find(|(w, _)| *w == why) {
            Some((_, n)) => *n += 1,
            None => causes.push((why, 1)),
        }
    }
    if !causes.is_empty() {
        // Stable, so equal counts keep the order they first appeared in.
        causes.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        lines.push(String::new());
        lines.push("Infrastructure errors (not scored):".into());
        lines.extend(causes.iter().map(|(why, n)| format!("- {n}× {why}")));
    }

    let mut misses: Vec<&Value> = rows.iter().filter(|r| matches!(verdict(r), "fail" | "timeout")).collect();
    if !misses.is_empty() {
        misses.sort_by_key(|r| (key(r), text(r, "task"), rep(r)));
        lines.push(String::new());
        lines.push("Misses:".into());
        for r in misses {
            let why = reason(r).unwrap_or_else(|| verdict(r).to_string());
            lines.push(format!("- `{}` {} #{}: {}", key(r), text(r, "task"), rep(r), prefix(&why, 160)));
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSONL: &str = r#"{"task": "b-task", "model": "m/one", "rep": 2, "verdict": "fail", "secs": 30, "sessions": 1, "requests": 3, "prompt": 1500, "completion": 500, "reason": ["assert x == 2"]}
{"task": "b-task", "model": "m/one", "rep": 1, "verdict": "pass", "secs": 90, "sessions": 1, "requests": 3, "prompt": 1000, "completion": 0, "reason": []}
{"task": "a-task", "model": "m/one", "rep": 1, "verdict": "infra", "secs": 5, "sessions": 0, "requests": 0, "prompt": 0, "completion": 0, "reason": ["request failed: connection refused"]}

{"task": "a-task", "model": "m/two", "rep": 1, "verdict": "timeout", "secs": 600, "sessions": 0, "requests": 0, "prompt": 0, "completion": 0, "reason": []}
{"task": "a-task", "model": "m/one", "rep": 2, "verdict": "infra", "secs": 5, "sessions": 0, "requests": 0, "prompt": 0, "completion": 0, "reason": []}
"#;

    fn rows(jsonl: &str) -> Vec<Value> {
        static COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("xtask-report-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("runs.jsonl");
        std::fs::write(&path, jsonl).unwrap();
        let rows = load(&[path.to_string_lossy().into_owned()]).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        rows
    }

    #[test]
    fn tables_follow_the_python_report() {
        let expected = "\
| model | passed | pass rate | timeouts | infra (not scored) | tokens | agent time |
|---|---|---|---|---|---|---|
| `m/one` | 1/2 | 50% | 0 | 2 | 3k | 2.2 min |
| `m/two` | 0/1 | 0% | 1 | 0 | 0k | 10.0 min |

| task | `m/one` | `m/two` |
|---|---|---|
| a-task | 0/0 ⚠⚠ | 0/1 ⧗ |
| b-task | 1/2 ✓✗ |  |

| model | tasks | pass@k | pass^k |
|---|---|---|---|
| `m/one` | 1 | 1/1 | 0/1 |
| `m/two` | 1 | 0/1 | 0/1 |

Infrastructure errors (not scored):
- 1× request failed: connection refused
- 1× (no message)

Misses:
- `m/one` b-task #2: assert x == 2
- `m/two` a-task #1: timeout";
        assert_eq!(report(&rows(JSONL)), expected);
    }

    #[test]
    fn only_all_scored_passes_succeed() {
        assert!(!all_passed(&rows(JSONL)));
        let pass = r#"{"task": "t", "model": "m", "verdict": "pass", "secs": 1, "prompt": 0, "completion": 0}"#;
        let infra = r#"{"task": "t", "model": "m", "verdict": "infra", "secs": 1, "prompt": 0, "completion": 0}"#;
        assert!(all_passed(&rows(&format!("{pass}\n{infra}\n"))));
        // Nothing scored is not a pass.
        assert!(!all_passed(&rows(&format!("{infra}\n"))));
        // A row without `rep` counts as the first run.
        assert!(report(&rows(pass)).contains("| t | 1/1 ✓ |"));
    }

    #[test]
    fn memory_settings_are_compared_side_by_side() {
        let on = r#"{"task": "t", "model": "m", "memory": "v2", "verdict": "pass", "secs": 1, "prompt": 0, "completion": 0}"#;
        let off = r#"{"task": "t", "model": "m", "memory": "off", "verdict": "fail", "secs": 1, "prompt": 0, "completion": 0}"#;
        let old = r#"{"task": "t", "model": "m", "verdict": "pass", "secs": 1, "prompt": 0, "completion": 0}"#;
        let text = report(&rows(&format!("{on}\n{off}\n{old}\n")));
        assert!(text.contains("| `m · v2` | 1/1 |"), "{text}");
        assert!(text.contains("| `m · off` | 0/1 |"), "{text}");
        assert!(text.contains("| `m` | 1/1 |"), "{text}");
        assert!(text.contains("| task | `m · v2` | `m · off` | `m` |"), "{text}");
        assert!(text.contains("- `m · off` t #1: fail"), "{text}");
    }

    #[test]
    fn pass_hat_k_needs_every_attempt() {
        let out = report(&rows(JSONL));
        // m/one: b-task passes once in two (pass@k yes, pass^k no); a-task is all infra.
        assert!(out.contains("| `m/one` | 1 | 1/1 | 0/1 |"), "{out}");
        let single = r#"{"task": "t", "model": "m", "rep": 1, "verdict": "pass", "secs": 1, "sessions": 1, "requests": 1, "prompt": 1, "completion": 1, "reason": []}"#;
        assert!(!report(&rows(single)).contains("pass^k"), "one attempt per task has no k");
    }

    #[test]
    fn claims_are_shown_beside_independent_grading() {
        let jsonl = r#"{"task": "t", "model": "m", "rep": 1, "verdict": "fail", "secs": 1, "sessions": 1, "requests": 1, "prompt": 1, "completion": 1, "reason": [], "false_completion": true, "functional_pass": false, "claims": [{"proposed": 3, "accepted": 1}]}
{"task": "t", "model": "m", "rep": 2, "verdict": "pass", "secs": 1, "sessions": 1, "requests": 1, "prompt": 1, "completion": 1, "reason": [], "false_completion": false, "functional_pass": true, "claims": [{"proposed": 1, "accepted": 1}]}"#;
        let out = report(&rows(jsonl));
        assert!(out.contains("| `m` | 4 | 2 | 1 | 1/2 |"), "{out}");
        assert!(!report(&rows(JSONL)).contains("claims proposed"), "older rows have no claims table");
    }

    #[test]
    fn no_scored_runs_shows_a_dash() {
        let infra = r#"{"task": "t", "model": "m", "verdict": "infra", "secs": 1, "prompt": 0, "completion": 0}"#;
        assert!(report(&rows(infra)).contains("| `m` | 0/0 | – | 0 | 1 | 0k | 0.0 min |"));
    }
}

//! Runs eval tasks headless in fresh temp copies and scores each with its own
//! check, which the agent never sees. Results go to target/evals/<stamp>.jsonl.
//!
//! Single-turn tasks (evals/<name>/prompt.txt) run as one prompt.
//! Multi-turn tasks (evals/multi/<name>/turns.txt) are replayed line by line
//! through the interactive REPL, one line per user message:
//!   ## session      start a new process (memory must carry over)
//!   ## env K=V      set an environment variable from here on
//!   ## goal         the next line runs as an autonomous --goal
//!
//!     cargo xtask eval                  # everything, EVAL_JOBS at a time (default 4)
//!     cargo xtask eval multi            # only multi-turn tasks
//!     cargo xtask eval rust-slugify     # one task (or a comma-separated list)
//!
//! EVAL_MODELS="claude-opus-5-5 gpt-5 nvidia/nemotron-3-super-120b-a12b" runs
//! every task on each model (each needs its provider's key), and
//! EVAL_REPEATS=3 runs each pair three times. EVAL_TIMEOUT (default 600) caps
//! each rusty process in seconds; EVAL_TRANSCRIPTS=0 drops the miss details
//! from the log. The summary compares models.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::report;

const DEFAULT_MODEL: &str = "nvidia/nemotron-3-super-120b-a12b";

/// The endpoint failing is an infrastructure error, not an agent failure.
/// Any line of the agent's output matching
/// `giving up after|HTTP 5[0-9][0-9]|request failed|connection to the model closed|API error: .*(overloaded|api_error|rate_limit)|is served by .* is not set`.
pub fn is_infra_line(line: &str) -> bool {
    line.contains("giving up after")
        || line.contains("request failed")
        || line.contains("connection to the model closed")
        || status_code(line, b"5")
        || line.find("API error: ").is_some_and(|i| {
            let rest = &line[i + "API error: ".len()..];
            ["overloaded", "api_error", "rate_limit"].iter().any(|w| rest.contains(w))
        })
        || line.find("is served by ").is_some_and(|i| line[i + "is served by ".len()..].contains(" is not set"))
}

/// The lines worth quoting as an infra error's reason:
/// `giving up after|HTTP [45][0-9][0-9]|request failed|connection to the model closed|API error|is not set`.
pub fn is_reason_line(line: &str) -> bool {
    line.contains("giving up after")
        || status_code(line, b"45")
        || line.contains("request failed")
        || line.contains("connection to the model closed")
        || line.contains("API error")
        || line.contains("is not set")
}

/// `HTTP ` followed by a three-digit status whose first digit is in `first`.
fn status_code(line: &str, first: &[u8]) -> bool {
    line.match_indices("HTTP ").any(|(i, _)| {
        let d = &line.as_bytes()[i + 5..];
        d.len() >= 3 && first.contains(&d[0]) && d[1].is_ascii_digit() && d[2].is_ascii_digit()
    })
}

/// Whether a task is picked by the command-line filter: empty for all,
/// `multi` for the multi-turn ones, otherwise a comma-separated list of names.
pub fn selected(filter: &str, name: &str, multi: bool) -> bool {
    match filter {
        "" => true,
        "multi" => multi,
        list => list.split(',').any(|n| n == name),
    }
}

#[derive(Clone, Debug)]
pub struct Task {
    pub name: String,
    pub dir: PathBuf,
}

/// Every runnable task under evals/ then evals/multi/, in name order.
pub fn tasks(evals: &Path, filter: &str) -> Result<Vec<Task>> {
    let mut out = Vec::new();
    for (base, multi) in [(evals.to_path_buf(), false), (evals.join("multi"), true)] {
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(&base)
            .with_context(|| format!("reading {}", base.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_dir() && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
            .collect();
        dirs.sort();
        for dir in dirs {
            if !dir.join("prompt.txt").is_file() && !dir.join("turns.txt").is_file() {
                continue;
            }
            let name = dir.file_name().unwrap().to_string_lossy().into_owned();
            if selected(filter, &name, multi) {
                out.push(Task { name, dir });
            }
        }
    }
    Ok(out)
}

/// One rusty process of a multi-turn task.
#[derive(Debug, PartialEq)]
pub struct Session {
    /// `K=V` pairs from the `## env` lines read so far.
    pub env: Vec<String>,
    /// Run the first line as `--goal` instead of piping every line in.
    pub goal: bool,
    pub lines: Vec<String>,
}

/// Splits turns.txt into the processes to run.
pub fn sessions(turns: &str) -> Vec<Session> {
    let mut out = Vec::new();
    let (mut env, mut buf, mut goal) = (Vec::new(), Vec::new(), false);
    let mut flush = |env: &Vec<String>, buf: &mut Vec<String>, goal: &mut bool| {
        if !buf.is_empty() {
            out.push(Session { env: env.clone(), goal: *goal, lines: std::mem::take(buf) });
            *goal = false;
        }
    };
    for line in turns.split('\n') {
        match line {
            "## session" => flush(&env, &mut buf, &mut goal),
            "## goal" => {
                flush(&env, &mut buf, &mut goal);
                goal = true;
            }
            "" => {}
            l => match l.strip_prefix("## env ") {
                Some(kv) => env.push(kv.to_string()),
                None => buf.push(l.to_string()),
            },
        }
    }
    flush(&env, &mut buf, &mut goal);
    out
}

struct Config {
    root: PathBuf,
    bin: PathBuf,
    limit: Duration,
    out: PathBuf,
    logs: PathBuf,
    transcripts: bool,
}

/// An environment variable, with empty counting as unset.
fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn number(name: &str, default: u64) -> Result<u64> {
    match var(name) {
        Some(v) => v.trim().parse().with_context(|| format!("{name}={v} is not a whole number")),
        None => Ok(default),
    }
}

/// Runs the selected tasks on every model and prints the report. True when
/// every scored run passed.
pub fn run(filter: &str) -> Result<bool> {
    let root = crate::root();
    let status = Command::new("cargo").args(["build", "--release", "-q"]).current_dir(&root).status()?;
    if !status.success() {
        bail!("cargo build --release failed ({status})");
    }
    let limit = number("EVAL_TIMEOUT", 600)?;
    let models = var("EVAL_MODELS").or_else(|| var("RUSTY_MODEL")).unwrap_or_else(|| DEFAULT_MODEL.into());
    let repeats = number("EVAL_REPEATS", 1)?;
    let jobs = number("EVAL_JOBS", 4)?.max(1) as usize;
    let stamp = stamp(SystemTime::now());
    let cfg = Config {
        bin: root.join("target/release/rusty"),
        limit: Duration::from_secs(limit),
        out: root.join(format!("target/evals/{stamp}.jsonl")),
        logs: root.join(format!("target/evals/{stamp}")),
        transcripts: var("EVAL_TRANSCRIPTS").is_none_or(|v| v == "1"),
        root,
    };
    std::fs::create_dir_all(&cfg.logs)?;
    File::create(&cfg.out)?;

    let tasks = tasks(&cfg.root.join("evals"), filter)?;
    let mut queue = Vec::new();
    for model in models.split_whitespace() {
        for rep in 1..=repeats {
            queue.extend(tasks.iter().map(|t| (t, model, rep)));
        }
    }
    println!("models {models} · {repeats}x · timeout {limit}s per session");
    let next = AtomicUsize::new(0);
    let jsonl = Mutex::new(());
    std::thread::scope(|s| {
        for _ in 0..jobs.min(queue.len()) {
            s.spawn(|| {
                while let Some((task, model, rep)) = queue.get(next.fetch_add(1, Ordering::SeqCst)) {
                    if let Err(e) = run_task(&cfg, task, model, *rep, &jsonl) {
                        eprintln!("{} {model} #{rep}: {e:#}", task.name);
                    }
                }
            });
        }
    });

    report::run(&[cfg.out.to_string_lossy().into_owned()])
}

enum Exit {
    Done,
    TimedOut,
}

/// Runs one rusty process with a hard time limit, appending to the home's
/// stdout and stderr. Extra args pass through.
fn agent(
    cfg: &Config,
    work: &Path,
    home: &Path,
    model: &str,
    env: &[String],
    args: &[&str],
    input: Option<String>,
) -> Result<Exit> {
    let append = |name: &str| OpenOptions::new().create(true).append(true).open(home.join(name));
    let (stdout, mut stderr) = (append("stdout")?, append("stderr")?);
    let mut cmd = Command::new(&cfg.bin);
    cmd.args(["--yolo", "--stats"])
        .args(args)
        .current_dir(work)
        .env("RUSTY_HOME", home)
        .env("RUSTY_MODEL", model)
        .env("NO_COLOR", "1")
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(stdout)
        .stderr(stderr.try_clone()?);
    for kv in env {
        if let Some((k, v)) = kv.split_once('=') {
            cmd.env(k, v);
        }
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            writeln!(stderr, "starting {}: {e}", cfg.bin.display())?;
            return Ok(Exit::Done);
        }
    };
    if let (Some(text), Some(mut pipe)) = (input, child.stdin.take()) {
        // A thread, so a child that stops reading can't stall the time limit.
        std::thread::spawn(move || pipe.write_all(text.as_bytes()));
    }
    let start = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            return Ok(Exit::Done);
        }
        if start.elapsed() >= cfg.limit {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(Exit::TimedOut);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Runs every session; timed out if any one of them was.
fn run_multi(cfg: &Config, task: &Task, work: &Path, home: &Path, model: &str) -> Result<Exit> {
    let turns = std::fs::read_to_string(task.dir.join("turns.txt"))?;
    let mut timed_out = false;
    for s in sessions(&turns) {
        let exit = if s.goal {
            agent(cfg, work, home, model, &s.env, &["--goal", &s.lines[0]], None)?
        } else {
            let input: String = s.lines.iter().map(|l| format!("{l}\n")).collect();
            agent(cfg, work, home, model, &s.env, &[], Some(input))?
        };
        timed_out |= matches!(exit, Exit::TimedOut);
    }
    Ok(if timed_out { Exit::TimedOut } else { Exit::Done })
}

fn run_task(cfg: &Config, task: &Task, model: &str, rep: u64, jsonl: &Mutex<()>) -> Result<()> {
    let (work, home) = (temp_dir()?, temp_dir()?);
    let result = score(cfg, task, model, rep, &work, &home, jsonl);
    let _ = std::fs::remove_dir_all(&work);
    let _ = std::fs::remove_dir_all(&home);
    result
}

fn score(cfg: &Config, task: &Task, model: &str, rep: u64, work: &Path, home: &Path, jsonl: &Mutex<()>) -> Result<()> {
    let _ = copy_dir(&task.dir.join("files"), work);
    let start = Instant::now();
    let exit = if task.dir.join("turns.txt").is_file() {
        run_multi(cfg, task, work, home, model)?
    } else {
        let prompt = std::fs::read_to_string(task.dir.join("prompt.txt"))?;
        agent(cfg, work, home, model, &[], &[prompt.trim_end_matches('\n')], None)?
    };
    let secs = start.elapsed().as_secs();

    let stdout = std::fs::read(home.join("stdout")).unwrap_or_default();
    let stderr = std::fs::read(home.join("stderr")).unwrap_or_default();
    let verdict = if matches!(exit, Exit::TimedOut) {
        "timeout"
    } else if check(task, work, home)? {
        "pass"
    } else if [&stdout, &stderr].iter().any(|b| String::from_utf8_lossy(b).split('\n').any(is_infra_line)) {
        "infra"
    } else {
        "fail"
    };

    let dest = cfg.logs.join(model.replace('/', "_")).join(format!("{}.{rep}", task.name));
    std::fs::create_dir_all(&dest)?;
    for f in ["stdout", "stderr", "check"] {
        let _ = std::fs::copy(home.join(f), dest.join(f));
    }

    let stats: Vec<Value> = py_lines(&String::from_utf8_lossy(&stderr))
        .into_iter()
        .filter(|l| l.starts_with('{'))
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let total = |k: &str| stats.iter().map(|r| r[k].as_i64().unwrap_or(0)).sum::<i64>();
    let (requests, prompt, completion) = (total("requests"), total("prompt"), total("completion"));
    let reason: Vec<String> = match verdict {
        "fail" => {
            let check = String::from_utf8_lossy(&std::fs::read(home.join("check")).unwrap_or_default()).into_owned();
            last_line(&check).into_iter().collect()
        }
        // The endpoint's own words, so a run of these can be diagnosed from the jsonl.
        "infra" => {
            let lines: Vec<String> = [&stdout, &stderr]
                .iter()
                .flat_map(|b| {
                    py_lines(&String::from_utf8_lossy(b)).iter().map(|l| l.trim().to_string()).collect::<Vec<_>>()
                })
                .collect();
            lines.into_iter().rfind(|l| is_reason_line(l)).into_iter().collect()
        }
        _ => Vec::new(),
    };
    let row = row(&task.name, model, rep, verdict, secs, stats.len(), requests, prompt, completion, &reason);
    {
        let _lock = jsonl.lock().unwrap();
        OpenOptions::new().append(true).open(&cfg.out)?.write_all(format!("{row}\n").as_bytes())?;
    }
    let short: String = model.chars().rev().take(24).collect::<Vec<_>>().into_iter().rev().collect();
    let why: String = reason.join(" ").chars().take(80).collect();
    println!(
        "{} {:<18} {short:<24} {verdict:<8} {secs:>4}s  {} session(s)  {:>5.0}k tok  {why}",
        report::mark(verdict),
        task.name,
        stats.len(),
        (prompt + completion) as f64 / 1000.0
    );

    // A miss explains itself in the log: the end of the agent's transcript, what
    // it changed, and the check's output (artifacts aren't always easy to fetch).
    if cfg.transcripts && matches!(verdict, "fail" | "timeout") {
        let mut miss =
            format!("::group::{} {model} #{rep}: {verdict}\n--- agent (last 60 lines)\n", task.name).into_bytes();
        miss.extend_from_slice(tail(&stdout, 60));
        miss.extend_from_slice(b"--- changes\n");
        let diff = Command::new("diff")
            .args(["-ruN", "--exclude=__pycache__", "--exclude=target", "--exclude=*.json"])
            .arg(task.dir.join("files"))
            .arg(work)
            .stderr(Stdio::null())
            .output();
        if let Ok(d) = diff {
            miss.extend_from_slice(head(&d.stdout, 120));
        }
        miss.extend_from_slice(b"--- check\n");
        miss.extend_from_slice(tail(&std::fs::read(home.join("check")).unwrap_or_default(), 20));
        miss.extend_from_slice(b"::endgroup::\n");
        std::io::stdout().lock().write_all(&miss)?;
    }
    Ok(())
}

/// Runs the task's hidden check in the agent's working copy.
fn check(task: &Task, work: &Path, home: &Path) -> Result<bool> {
    let log = File::create(home.join("check"))?;
    let status = Command::new("bash")
        .arg(task.dir.join("check.sh"))
        .current_dir(work)
        .env("EVAL_DIR", &task.dir)
        .env("RUSTY_HOME", home)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .status()?;
    Ok(status.success())
}

/// One jsonl row, laid out exactly as Python's `json.dumps` writes it.
#[allow(clippy::too_many_arguments)]
pub fn row(
    task: &str,
    model: &str,
    rep: u64,
    verdict: &str,
    secs: u64,
    sessions: usize,
    requests: i64,
    prompt: i64,
    completion: i64,
    reason: &[String],
) -> String {
    let reason: Vec<String> = reason.iter().map(|r| py_json(r)).collect();
    format!(
        "{{\"task\": {}, \"model\": {}, \"rep\": {rep}, \"verdict\": {}, \"secs\": {secs}, \"sessions\": {sessions}, \
         \"requests\": {requests}, \"prompt\": {prompt}, \"completion\": {completion}, \"reason\": [{}]}}",
        py_json(task),
        py_json(model),
        py_json(verdict),
        reason.join(", ")
    )
}

/// A JSON string with non-ASCII escaped, as Python's `json.dumps` does.
pub fn py_json(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7f => {
                let mut units = [0u16; 2];
                for u in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{u:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Lines as Python reads a text file: `\n`, `\r\n` and `\r` all end one.
fn py_lines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(['\r', '\n']) {
        out.push(&rest[..i]);
        let skip = if rest[i..].starts_with("\r\n") { 2 } else { 1 };
        rest = &rest[i + skip..];
    }
    if !rest.is_empty() {
        out.push(rest);
    }
    out
}

/// The last line of the stripped text (Python's `.strip().splitlines()[-1:]`).
fn last_line(text: &str) -> Option<String> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let text = text.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c));
    if text.is_empty() {
        return None;
    }
    let breaks = ['\n', '\u{b}', '\u{c}', '\u{1c}', '\u{1d}', '\u{1e}', '\u{85}', '\u{2028}', '\u{2029}'];
    text.rsplit(breaks).next().map(str::to_string)
}

/// The last `n` lines, as `tail -n` prints them.
pub fn tail(b: &[u8], n: usize) -> &[u8] {
    if n == 0 {
        return &[];
    }
    let end = if b.ends_with(b"\n") { b.len() - 1 } else { b.len() };
    let mut seen = 0;
    for i in (0..end).rev() {
        if b[i] == b'\n' {
            seen += 1;
            if seen == n {
                return &b[i + 1..];
            }
        }
    }
    b
}

/// The first `n` lines, as `head -n` prints them.
pub fn head(b: &[u8], n: usize) -> &[u8] {
    let mut seen = 0;
    for (i, &c) in b.iter().enumerate() {
        if c == b'\n' {
            seen += 1;
            if seen == n {
                return &b[..=i];
            }
        }
    }
    b
}

/// Copies a directory tree like `cp -R src/. dst/`: modes kept, symlinks as links.
fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let (from, to) = (entry.path(), dst.join(entry.file_name()));
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_dir(&from, &to)?;
        } else if kind.is_symlink() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(std::fs::read_link(&from)?, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// A fresh, empty directory under the system temp dir (like `mktemp -d`).
fn temp_dir() -> Result<PathBuf> {
    static COUNT: AtomicUsize = AtomicUsize::new(0);
    let base = std::env::temp_dir();
    loop {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().subsec_nanos();
        let n = COUNT.fetch_add(1, Ordering::SeqCst);
        let dir = base.join(format!("rusty-eval-{}-{n}-{nanos:x}", std::process::id()));
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e).with_context(|| format!("creating {}", dir.display())),
        }
    }
}

/// `YYYYmmdd-HHMMSS` in UTC.
pub fn stamp(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}{month:02}{day:02}-{:02}{:02}{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_selection() {
        assert!(selected("", "rust-slugify", false));
        assert!(selected("", "undo-one-step", true));
        assert!(selected("multi", "undo-one-step", true));
        assert!(!selected("multi", "rust-slugify", false));
        assert!(selected("rust-slugify", "rust-slugify", false));
        assert!(!selected("rust-slug", "rust-slugify", false));
        assert!(selected("cli-flag,undo-one-step", "undo-one-step", true));
        assert!(selected("cli-flag,undo-one-step", "cli-flag", false));
        assert!(!selected("cli-flag,undo-one-step", "js-dedupe", false));
        assert!(!selected("nope", "cli-flag", false));
    }

    #[test]
    fn tasks_come_from_the_repository() {
        let evals = crate::root().join("evals");
        let all = tasks(&evals, "").unwrap();
        let multi = |t: &Task| t.dir.parent() == Some(evals.join("multi").as_path());
        assert!(all.iter().any(|t| t.name == "rust-slugify" && !multi(t)));
        assert!(all.iter().any(|t| t.name == "goal-todo-cli" && multi(t)));
        assert!(!all.iter().any(|t| t.name == "multi"));
        let single: Vec<_> = all.iter().take_while(|t| !multi(t)).map(|t| &t.name).collect();
        let mut sorted = single.clone();
        sorted.sort();
        assert_eq!(single, sorted);
        assert!(tasks(&evals, "multi").unwrap().iter().all(multi));
        let two = tasks(&evals, "rust-slugify,undo-one-step").unwrap();
        assert_eq!(two.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["rust-slugify", "undo-one-step"]);
        assert!(tasks(&evals, "no-such-task").unwrap().is_empty());
    }

    #[test]
    fn endpoint_failures_are_infra() {
        for line in [
            "error: giving up after 5 attempts in 120s: HTTP 429 Too Many Requests",
            "HTTP 503 Service Unavailable",
            "request failed: error sending request for url (http://127.0.0.1:9/v1/chat/completions)",
            "error: the connection to the model closed unexpectedly",
            r#"API error: {"type":"overloaded_error","message":"Overloaded"}"#,
            r#"error: API error: {"type":"rate_limit_error"}"#,
            "`gpt-5` is served by OpenAI, but OPENAI_API_KEY is not set. Add it to your shell",
        ] {
            assert!(is_infra_line(line), "{line}");
            assert!(is_reason_line(line), "{line}");
        }
        for line in [
            "HTTP 404 Not Found",
            "HTTP 5xx",
            r#"API error: {"code":400,"message":"bad tool schema","type":"invalid_request_error"}"#,
            "assertion failed: the value is not set",
            "test result: FAILED. 3 passed; 1 failed",
        ] {
            assert!(!is_infra_line(line), "{line}");
        }
        // Quoted as the reason, though not infra on their own.
        assert!(is_reason_line("HTTP 401 Unauthorized"));
        assert!(is_reason_line(r#"API error: {"code":400}"#));
        assert!(!is_reason_line("HTTP 302 Found"));
    }

    #[test]
    fn turns_split_into_sessions() {
        let turns = "## env RUSTY_CONTEXT_TOKENS=26000\nfirst\nsecond\n\n## session\nthird\n## goal\nbuild it\n## env A=1\n## session\nlast";
        assert_eq!(
            sessions(turns),
            [
                Session {
                    env: vec!["RUSTY_CONTEXT_TOKENS=26000".into()],
                    goal: false,
                    lines: vec!["first".into(), "second".into()]
                },
                Session { env: vec!["RUSTY_CONTEXT_TOKENS=26000".into()], goal: false, lines: vec!["third".into()] },
                Session {
                    env: vec!["RUSTY_CONTEXT_TOKENS=26000".into(), "A=1".into()],
                    goal: true,
                    lines: vec!["build it".into()]
                },
                Session {
                    env: vec!["RUSTY_CONTEXT_TOKENS=26000".into(), "A=1".into()],
                    goal: false,
                    lines: vec!["last".into()]
                },
            ]
        );
        assert!(sessions("").is_empty());
        assert!(sessions("## session\n\n## session\n").is_empty());
    }

    #[test]
    fn rows_match_python_json() {
        let r = row("t", "m/x", 2, "infra", 7, 1, 3, 100, 20, &["café \"x\"\t\u{1f600}".into()]);
        assert_eq!(
            r,
            r#"{"task": "t", "model": "m/x", "rep": 2, "verdict": "infra", "secs": 7, "sessions": 1, "requests": 3, "prompt": 100, "completion": 20, "reason": ["caf\u00e9 \"x\"\t\ud83d\ude00"]}"#
        );
        assert!(row("t", "m", 1, "pass", 0, 0, 0, 0, 0, &[]).ends_with(r#""reason": []}"#));
    }

    #[test]
    fn tail_and_head_match_coreutils() {
        assert_eq!(tail(b"a\nb\nc\n", 2), b"b\nc\n");
        assert_eq!(tail(b"a\nb\nc", 2), b"b\nc");
        assert_eq!(tail(b"a\nb\n", 5), b"a\nb\n");
        assert_eq!(tail(b"", 5), b"");
        assert_eq!(head(b"a\nb\nc\n", 2), b"a\nb\n");
        assert_eq!(head(b"a\nb", 5), b"a\nb");
    }

    #[test]
    fn check_reason_is_its_last_line() {
        assert_eq!(last_line("ok\n  FAILED: x\n\n").as_deref(), Some("  FAILED: x"));
        assert_eq!(last_line("one\r\ntwo\r\n").as_deref(), Some("two"));
        assert_eq!(last_line(" \n "), None);
        assert_eq!(py_lines("a\r\nb\rc\nd"), ["a", "b", "c", "d"]);
    }

    #[test]
    fn stamps_are_utc() {
        assert_eq!(stamp(UNIX_EPOCH), "19700101-000000");
        assert_eq!(stamp(UNIX_EPOCH + Duration::from_secs(1_791_313_200)), "20261006-190000");
        assert_eq!(stamp(UNIX_EPOCH + Duration::from_secs(951_782_400)), "20000229-000000");
    }
}

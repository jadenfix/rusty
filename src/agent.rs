//! The agent loop: ask the model, run the tools it calls, repeat until it
//! answers. Goal mode, loop mode, subagents and swarms are built on one turn.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config::{AgentsConfig, AgentsMode};
use crate::context;
use crate::display::{self, Display, View};
use crate::llm::{Client, Event, Reply, ToolCall};
use crate::memory::{Memory, KINDS};
use crate::permissions::{Mode, Policy, Verdict};
use crate::signal;
use crate::tips;
use crate::tools;
use crate::ui;

const MAX_STEPS: usize = 80;
const WORKER_STEPS: usize = 30;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanItem {
    pub text: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum GoalStatus {
    Active,
    Done(String),
    Blocked(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goal {
    pub objective: String,
    pub status: GoalStatus,
    pub turns: u32,
}

#[derive(Default)]
struct LoopCtl {
    self_paced: bool,
    next_delay: Option<u64>,
    stop: bool,
}

/// Token accounting for the session, including delegated work.
#[derive(Default, Clone)]
pub struct Totals {
    pub prompt: u64,
    pub completion: u64,
    pub requests: u64,
    /// model -> (requests, prompt, completion)
    pub by_model: BTreeMap<String, (u64, u64, u64)>,
    /// tool -> approx tokens its results added to context
    pub by_tool: BTreeMap<String, u64>,
}

impl Totals {
    fn add_request(&mut self, model: &str, prompt: u64, completion: u64) {
        self.prompt += prompt;
        self.completion += completion;
        self.requests += 1;
        let e = self.by_model.entry(model.to_string()).or_default();
        e.0 += 1;
        e.1 += prompt;
        e.2 += completion;
    }

    fn merge(&mut self, other: &Totals) {
        for (m, (r, p, c)) in &other.by_model {
            let e = self.by_model.entry(m.clone()).or_default();
            e.0 += r;
            e.1 += p;
            e.2 += c;
        }
        for (t, n) in &other.by_tool {
            *self.by_tool.entry(t.clone()).or_default() += n;
        }
        self.prompt += other.prompt;
        self.completion += other.completion;
        self.requests += other.requests;
    }
}

pub struct Agent {
    client: Arc<Client>,
    pub model: String,
    pub temperature: f32,
    pub cwd: PathBuf,
    pub history: Vec<Value>,
    pub policy: Policy,
    pub memory: Memory,
    pub plan: Vec<PlanItem>,
    pub goal: Option<Goal>,
    loop_ctl: Option<LoopCtl>,
    pub agents: AgentsConfig,
    pub tips: bool,
    tips_shown: HashSet<&'static str>,
    pub totals: Totals,
    pub last_prompt_tokens: u64,
    project_notes: String,
    pub session_path: Option<PathBuf>,
    is_worker: bool,
    progress: Option<Arc<AtomicUsize>>,
}

impl Agent {
    pub fn new(client: Arc<Client>, model: String, cwd: PathBuf, policy: Policy, memory: Memory) -> Self {
        let project_notes = load_project_notes(&cwd);
        Self {
            client,
            model,
            temperature: 0.2,
            cwd,
            history: Vec::new(),
            policy,
            memory,
            plan: Vec::new(),
            goal: None,
            loop_ctl: None,
            agents: AgentsConfig::default(),
            tips: true,
            tips_shown: HashSet::new(),
            totals: Totals::default(),
            last_prompt_tokens: 0,
            project_notes,
            session_path: None,
            is_worker: false,
            progress: None,
        }
    }

    pub fn clear(&mut self) {
        self.history.clear();
        self.plan.clear();
        self.goal = None;
        self.last_prompt_tokens = 0;
    }

    // ---------------------------------------------------------------- prompt

    fn system_prompt(&self, query: &str) -> (String, Vec<String>) {
        if self.is_worker {
            let s = format!(
                "You are a research worker for a coding agent, working in {}. Investigate the task you are given \
                 with read-only tools and report back: findings first, with path:line references, then anything \
                 uncertain. You cannot edit files. Prefer search, glob and outline over reading whole files. Stop \
                 as soon as you can answer; your report should be under 400 words.\n",
                self.cwd.display()
            );
            return (s, Vec::new());
        }
        let mut s = format!(
            "You are Rusty, a coding agent working in the user's terminal on the project at {}. You're a calm, \
             sharp engineer with a dry sense of humour who has lived in terminals since the dial-up days. Warm \
             but brief. You never gush or pad, and you'd rather show a passing test than talk about one.\n\n",
            self.cwd.display()
        );
        s.push_str(
            "How to work:\n\
             - Look before you change. Find code with search, glob and outline; read only the ranges you need. \
             Read a file before editing it.\n\
             - Make the smallest change that fully solves the task, in the project's existing style.\n\
             - Verify with a build, the tests, or by running the code. Never claim something works without evidence.\n\
             - For multi-step work keep a short plan with the plan tool and update it as you go.\n\
             - Save durable lessons with remember: build and test commands, conventions, traps, user preferences.\n\
             - If a tool call is denied, don't retry it. Find another way or ask.\n\
             - No flattery, no filler, no apologies. When you finish, say what changed and how you checked it.\n\
             \n\
             Token discipline: every token you pull into context is paid for on every later step. Search before \
             reading, read ranges of big files, don't re-read what you just read, and keep command output short \
             (tail, head, -q flags).\n",
        );
        match self.agents.mode {
            AgentsMode::Off => {}
            AgentsMode::Sub => s.push_str(
                "\nDelegation: the task tool runs a read-only subagent with its own context and returns its \
                 report. Use it for a focused investigation that would take many reads; do small lookups yourself.\n",
            ),
            AgentsMode::Swarm => s.push_str(&format!(
                "\nDelegation: the swarm tool runs up to {} read-only workers in parallel, each with its own \
                 context, and returns their reports. Use it for broad work that splits cleanly, such as surveying \
                 several modules or checking many files for the same problem. Give each worker a self-contained \
                 prompt and its own slice.\n",
                self.agents.swarm_max
            )),
            AgentsMode::Auto => s.push_str(&format!(
                "\nDelegation: task runs one read-only subagent; swarm runs up to {} in parallel. Pick the lightest \
                 option that works: do it yourself if it takes fewer than about five reads, use task for one deep \
                 investigation, and use swarm only when the work splits into three or more independent parts.\n",
                self.agents.swarm_max
            )),
        }
        s.push_str(&format!(
            "\nEnvironment: {} {}, commands run with bash in the project directory. Permission mode: {}.\n",
            std::env::consts::OS,
            std::env::consts::ARCH,
            self.policy.mode.name()
        ));
        if !self.project_notes.is_empty() {
            s.push_str("\nProject instructions:\n");
            s.push_str(&self.project_notes);
            s.push('\n');
        }
        let (mem, ids) = self.memory.context_block(query, context::MEMORY_CHARS);
        if !mem.is_empty() {
            s.push_str("\nMemories relevant to this request (from earlier sessions; verify before relying on them):\n");
            s.push_str(&mem);
        }
        if let Some(g) = &self.goal {
            s.push_str(&format!("\nCurrent goal (keep working until it is done and verified): {}\n", g.objective));
        }
        if !self.plan.is_empty() {
            s.push_str("\nCurrent plan:\n");
            for p in &self.plan {
                s.push_str(&format!("- [{}] {}\n", p.status, p.text));
            }
        }
        if display::view() == View::Adhd {
            // Last in the prompt, where it carries the most weight.
            s.push_str(
                "\nFOCUS MODE. Your final reply must be at most 40 words: one bolded bottom-line sentence, then \
                 at most three bullets of a few words each. No preamble, no step-by-step recap, no caveats \
                 unless something failed.\n",
            );
        }
        (s, ids)
    }

    fn tool_defs(&self) -> Value {
        let mut defs: Vec<Value> = tools::definitions()
            .as_array()
            .unwrap()
            .iter()
            .filter(|d| !self.is_worker || !matches!(d["function"]["name"].as_str(), Some("write_file" | "edit_file")))
            .cloned()
            .collect();
        if self.is_worker {
            return Value::Array(defs);
        }
        let t = tools::tool;
        defs.push(t(
            "remember",
            "Save a durable memory for future sessions.",
            json!({
                "kind": {"type": "string", "enum": KINDS},
                "text": {"type": "string", "description": "One self-contained sentence"},
                "global": {"type": "boolean", "description": "True for user preferences that apply to every project"}
            }),
            &["kind", "text"],
        ));
        defs.push(t("recall", "Search long-term memory.", json!({"query": {"type": "string"}}), &["query"]));
        defs.push(t("forget", "Delete a memory that is wrong or stale.", json!({"id": {"type": "string"}}), &["id"]));
        defs.push(t(
            "plan",
            "Replace the task plan. Use it for any work with more than two steps.",
            json!({
                "items": {"type": "array", "items": {"type": "object", "properties": {
                    "text": {"type": "string"},
                    "status": {"type": "string", "enum": ["pending", "in_progress", "done"]}
                }, "required": ["text", "status"]}}
            }),
            &["items"],
        ));
        let job = json!({"type": "object", "properties": {
            "description": {"type": "string", "description": "3-6 word label"},
            "prompt": {"type": "string", "description": "Self-contained instructions; the worker can't see this conversation"}
        }, "required": ["description", "prompt"]});
        if matches!(self.agents.mode, AgentsMode::Sub | AgentsMode::Auto) {
            defs.push(t(
                "task",
                "Run one read-only subagent with its own context to investigate something and report back.",
                job["properties"].clone(),
                &["description", "prompt"],
            ));
        }
        if matches!(self.agents.mode, AgentsMode::Swarm | AgentsMode::Auto) {
            defs.push(t(
                "swarm",
                "Run several read-only workers in parallel, one per task, and get all their reports.",
                json!({"tasks": {"type": "array", "items": job, "maxItems": self.agents.swarm_max}}),
                &["tasks"],
            ));
        }
        if self.goal.as_ref().is_some_and(|g| g.status == GoalStatus::Active) {
            defs.push(t(
                "goal_done",
                "Close the current goal. Call only when it is achieved and verified, or when you are blocked on the user.",
                json!({
                    "evidence": {"type": "string", "description": "How you verified it, or what you need from the user"},
                    "blocked": {"type": "boolean"}
                }),
                &["evidence"],
            ));
        }
        if self.loop_ctl.as_ref().is_some_and(|l| l.self_paced) {
            defs.push(t(
                "loop_next",
                "Schedule the next loop run, or stop the loop.",
                json!({
                    "delay_secs": {"type": "integer", "description": "Seconds until the next run (60-3600)"},
                    "stop": {"type": "boolean"}
                }),
                &[],
            ));
        }
        Value::Array(defs)
    }

    // ------------------------------------------------------------------ turn

    /// Runs one user turn to completion. Returns false if interrupted.
    pub fn run_turn(&mut self, user: &str) -> Result<bool> {
        self.history.push(json!({"role": "user", "content": user}));
        let query = match &self.goal {
            Some(g) => format!("{user} {}", g.objective),
            None => user.to_string(),
        };
        let max_steps = if self.is_worker { WORKER_STEPS } else { MAX_STEPS };
        let mut d = Display::new(self.is_worker);
        let before = self.totals.clone();
        // How often each exact call has been made this turn, to catch doom loops.
        let mut seen: HashMap<String, u32> = HashMap::new();
        let mut outcome: Result<bool> = Ok(true);

        for step in 0..max_steps {
            if signal::interrupted() {
                outcome = Ok(false);
                break;
            }
            self.manage_context(&mut d);
            let (system, mem_ids) = self.system_prompt(&query);
            if step == 0 {
                self.memory.touch(&mem_ids);
            }
            let mut messages = Vec::with_capacity(self.history.len() + 1);
            messages.push(json!({"role": "system", "content": system}));
            messages.extend(self.history.iter().cloned());

            let started = Instant::now();
            let reply = match self.ask(&mut d, messages, self.tool_defs()) {
                Ok(r) => r,
                Err(e) => {
                    // Drop a dangling user message so a retry starts clean.
                    if step == 0 {
                        self.history.pop();
                    }
                    outcome = Err(e);
                    break;
                }
            };
            if let Some(u) = reply.usage {
                self.totals.add_request(&self.model, u.prompt, u.completion);
                self.last_prompt_tokens = u.prompt;
            }
            self.trace(&reply, started.elapsed());

            let mut msg = json!({"role": "assistant", "content": reply.content});
            if !reply.tool_calls.is_empty() {
                msg["tool_calls"] = Value::Array(
                    reply
                        .tool_calls
                        .iter()
                        .map(|c| json!({"id": c.id, "type": "function", "function": {"name": c.name, "arguments": c.arguments}}))
                        .collect(),
                );
            }
            self.history.push(msg);

            if reply.finish_reason.as_deref() == Some("interrupted") {
                outcome = Ok(false);
                break;
            }
            if reply.tool_calls.is_empty() {
                if reply.finish_reason.as_deref() == Some("length") {
                    d.line(&ui::warn("  (the reply hit the token limit; say \"continue\" to resume)"));
                }
                break;
            }
            for call in &reply.tool_calls {
                let output = if signal::interrupted() {
                    "skipped: the user interrupted".to_string()
                } else {
                    let repeats = seen.entry(format!("{}{}", call.name, call.arguments)).or_insert(0);
                    *repeats += 1;
                    let n = *repeats;
                    let mut out = self.dispatch(call, &mut d);
                    if n >= 3 && !matches!(call.name.as_str(), "plan" | "recall") {
                        out.push_str(&format!(
                            "\n\nnote: this exact call has now run {n} times this turn. Repeating it will not change \
                             the result. Step back, re-read the error, and try a different approach."
                        ));
                    }
                    out
                };
                self.history.push(json!({"role": "tool", "tool_call_id": call.id, "content": output}));
            }
            if self.goal.as_ref().is_some_and(|g| g.status != GoalStatus::Active) {
                break;
            }
            if step + 1 == max_steps && !self.is_worker {
                d.line(&ui::warn(&format!("  (stopped after {max_steps} steps; say \"continue\" to keep going)")));
            }
        }

        for (tool, n) in &d.stats.tool_tokens {
            *self.totals.by_tool.entry(tool.clone()).or_default() += n;
        }
        if self.is_worker {
            return outcome;
        }
        let interrupted = matches!(outcome, Ok(false)) || signal::interrupted();
        d.recap(
            self.totals.prompt - before.prompt,
            self.totals.completion - before.completion,
            self.context_pct(),
            interrupted,
        );
        if self.tips && outcome.is_ok() {
            let facts = tips::Facts {
                stats: &d.stats,
                prompt_last: self.last_prompt_tokens,
                window: context::window(),
                completion: self.totals.completion - before.completion,
                session_total: self.totals.prompt + self.totals.completion,
                delegation_off: self.agents.mode == AgentsMode::Off,
                model: &self.model,
            };
            if let Some(tip) = tips::pick(&facts, &mut self.tips_shown) {
                println!("{} {}\n", ui::warn("tip"), ui::dim(&tip));
            }
        }
        outcome
    }

    /// Sends one request on a background thread and renders it as it streams.
    /// Ctrl-C returns at once with whatever text arrived.
    fn ask(&mut self, d: &mut Display, messages: Vec<Value>, tools: Value) -> Result<Reply> {
        let rx = self.client.start_chat(self.model.clone(), messages, tools, self.temperature);
        loop {
            // Checked on every event: a model that streams steadily never goes quiet.
            if signal::interrupted() {
                let content = std::mem::take(&mut d.content);
                d.end_stream();
                return Ok(Reply { content, finish_reason: Some("interrupted".into()), ..Default::default() });
            }
            match rx.recv_timeout(Duration::from_millis(60)) {
                Ok(Event::Reasoning(s)) => d.reasoning(&s),
                Ok(Event::Content(s)) => d.content(&s),
                Ok(Event::Done(r)) => {
                    d.end_stream();
                    return r;
                }
                Err(RecvTimeoutError::Timeout) => d.tick(),
                Err(RecvTimeoutError::Disconnected) => {
                    d.end_stream();
                    bail!("the connection to the model closed unexpectedly");
                }
            }
        }
    }

    fn dispatch(&mut self, call: &ToolCall, d: &mut Display) -> String {
        let raw = if call.arguments.trim().is_empty() { "{}" } else { &call.arguments };
        let args: Value = match serde_json::from_str(raw) {
            Ok(v) => v,
            Err(e) => {
                return format!("error: arguments were not valid JSON ({e}). Call the tool again with valid JSON.")
            }
        };
        if let Some(p) = &self.progress {
            p.fetch_add(1, Ordering::Relaxed);
        }
        d.tool(&call.name, &args);
        let result = match call.name.as_str() {
            "remember" => Ok(self.memory.add(
                args["kind"].as_str().unwrap_or("fact"),
                args["text"].as_str().unwrap_or(""),
                args["global"].as_bool().unwrap_or(false),
            )),
            "recall" => Ok(self.recall(args["query"].as_str().unwrap_or(""))),
            "forget" => {
                let id = args["id"].as_str().unwrap_or("");
                Ok(if self.memory.forget(id) { format!("forgot {id}") } else { format!("no memory with id {id}") })
            }
            "plan" => {
                self.plan = serde_json::from_value(args["items"].clone()).unwrap_or_default();
                Ok("plan updated".into())
            }
            "task" => Ok(self.run_workers(vec![job(&args)], d)),
            "swarm" => {
                let jobs: Vec<(String, String)> = args["tasks"].as_array().into_iter().flatten().map(job).collect();
                if jobs.is_empty() {
                    Err(anyhow::anyhow!("swarm needs at least one task"))
                } else {
                    Ok(self.run_workers(jobs, d))
                }
            }
            "goal_done" => Ok(self.finish_goal(&args)),
            "loop_next" => {
                if let Some(l) = &mut self.loop_ctl {
                    l.stop = args["stop"].as_bool().unwrap_or(false);
                    l.next_delay = args["delay_secs"].as_u64().map(|s| s.clamp(60, 3600));
                }
                Ok("noted".into())
            }
            name => self.run_guarded(name, &args, d),
        };
        let (text, ok) = match result {
            Ok(t) => (t, true),
            Err(e) => (format!("error: {e:#}"), false),
        };
        d.tool_result(&call.name, &args, &text, ok);
        text
    }

    /// Runs a filesystem/shell tool through the permission policy.
    fn run_guarded(&mut self, name: &str, args: &Value, d: &mut Display) -> Result<String> {
        match self.policy.decide(name, args, &self.cwd) {
            Verdict::Allow => {}
            Verdict::Deny(why) => {
                return Ok(format!(
                    "denied by permissions ({why}). Do not retry this call; choose another approach or ask the user."
                ));
            }
            Verdict::Ask(why) => {
                d.pause();
                if display::view() == View::Adhd {
                    println!("{} {}", ui::info("▸"), tools::summary(name, args));
                }
                print!("{}", tools::preview(name, args));
                match ask_user(&why) {
                    Answer::Yes => {}
                    Answer::Always => {
                        let rule = Policy::suggest_rule(name, args);
                        println!("  {}", ui::dim(&format!("allowing `{rule}` from now on (/permissions to undo)")));
                        self.policy.allow.push(rule);
                        self.policy.save();
                    }
                    Answer::No(None) => {
                        return Ok("the user declined this call. Do not retry it; ask what they want instead.".into())
                    }
                    Answer::No(Some(fb)) => return Ok(format!("the user declined this call and said: {fb}")),
                }
            }
        }
        tools::execute(name, args)
    }

    fn recall(&mut self, query: &str) -> String {
        let hits: Vec<(String, String)> = self
            .memory
            .search(query, 10)
            .into_iter()
            .map(|(_, e)| (e.id.clone(), format!("[{}:{}] {}", e.kind, e.id, e.text)))
            .collect();
        if hits.is_empty() {
            return "no matching memories".into();
        }
        let ids: Vec<String> = hits.iter().map(|h| h.0.clone()).collect();
        self.memory.touch(&ids);
        hits.into_iter().map(|h| h.1).collect::<Vec<_>>().join("\n")
    }

    // ------------------------------------------------- subagents and swarms

    /// Runs read-only workers in parallel threads and collects their reports.
    /// One job is a subagent; several are a swarm.
    fn run_workers(&mut self, jobs: Vec<(String, String)>, d: &mut Display) -> String {
        let cfg = self.agents.clone();
        let n = jobs.len().min(cfg.swarm_max.max(1));
        let label = if n == 1 { "subagent" } else { "swarm" };
        let (tx, rx) = std::sync::mpsc::channel();
        let mut counters = Vec::new();
        for (i, (desc, prompt)) in jobs.into_iter().take(n).enumerate() {
            let model = if !cfg.swarm_models.is_empty() && n > 1 {
                cfg.swarm_models[i % cfg.swarm_models.len()].clone()
            } else {
                cfg.sub_model.clone().unwrap_or_else(|| self.model.clone())
            };
            let temperature = if n > 1 { 0.2 + cfg.spread * i as f32 / (n - 1) as f32 } else { 0.2 };
            let counter = Arc::new(AtomicUsize::new(0));
            counters.push(counter.clone());
            let (client, cwd, deny, tx) = (self.client.clone(), self.cwd.clone(), self.policy.deny.clone(), tx.clone());
            std::thread::spawn(move || {
                let mut w = Agent::new(client, model.clone(), cwd, Policy::new(Mode::ReadOnly), Memory::empty());
                w.is_worker = true;
                w.temperature = temperature.min(1.2);
                w.progress = Some(counter);
                w.policy.deny = deny;
                let t0 = Instant::now();
                let result = w.run_turn(&prompt);
                let answer = w
                    .history
                    .iter()
                    .rev()
                    .find(|m| m["role"] == "assistant" && m["content"].as_str().is_some_and(|s| !s.trim().is_empty()))
                    .and_then(|m| m["content"].as_str())
                    .unwrap_or("")
                    .to_string();
                let report = match result {
                    Err(e) => format!("failed: {e:#}"),
                    Ok(_) if answer.is_empty() => "returned nothing".to_string(),
                    Ok(_) => answer,
                };
                let _ = tx.send((i, desc, model, report, w.totals, t0.elapsed()));
            });
        }
        drop(tx);

        let mut reports = vec![String::new(); n];
        let mut done = 0;
        while done < n {
            match rx.recv_timeout(Duration::from_millis(60)) {
                Ok((i, desc, model, report, totals, took)) => {
                    done += 1;
                    self.totals.merge(&totals);
                    let calls = counters[i].load(Ordering::Relaxed);
                    let failed = report.starts_with("failed:");
                    if display::view() != View::Adhd {
                        let mark = if failed { ui::err("✗") } else { ui::ok("✓") };
                        let short_model = model.rsplit('/').next().unwrap_or(&model).to_string();
                        d.line(&format!(
                            "  {mark} {} {}",
                            desc,
                            ui::dim(&format!("· {calls} calls · {}s · {short_model}", took.as_secs()))
                        ));
                    }
                    reports[i] = format!("## {} ({})\n{}", desc, if failed { "failed" } else { "done" }, report);
                }
                Err(RecvTimeoutError::Timeout) => {
                    if signal::interrupted() {
                        break;
                    }
                    let calls: usize = counters.iter().map(|c| c.load(Ordering::Relaxed)).sum();
                    d.set_status(format!("{label} {done}/{n} · {calls} tool calls"));
                    d.tick();
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        d.set_status(String::new());
        if done < n {
            return format!("{label} interrupted after {done} of {n} workers finished.\n\n{}", reports.join("\n\n"));
        }
        // Split a fixed budget across workers so a big swarm can't flood the context.
        let per = (24_000 / n).max(2_000);
        reports.iter().map(|r| ui::truncate(r, per).to_string()).collect::<Vec<_>>().join("\n\n")
    }

    // ------------------------------------------------------------- goal mode

    fn finish_goal(&mut self, args: &Value) -> String {
        let evidence = args["evidence"].as_str().unwrap_or("").to_string();
        let Some(g) = &mut self.goal else { return "no active goal".into() };
        g.status = if args["blocked"].as_bool().unwrap_or(false) {
            GoalStatus::Blocked(evidence)
        } else {
            GoalStatus::Done(evidence)
        };
        "goal closed".into()
    }

    /// Works on a goal across as many turns as it takes, up to a limit.
    pub fn run_goal(&mut self, objective: Option<&str>) -> Result<()> {
        let mut prompt = match objective {
            Some(obj) => {
                self.goal = Some(Goal { objective: obj.to_string(), status: GoalStatus::Active, turns: 0 });
                format!(
                    "New goal: {obj}\n\nWork on this autonomously until it is done. Start with a plan (plan tool). \
                     Verify as you go with builds, tests or by running the code. When the goal is achieved and \
                     verified, call goal_done with the evidence. If you are blocked on something only the user can \
                     provide, call goal_done with blocked=true and say what you need."
                )
            }
            None => match &mut self.goal {
                Some(g) => {
                    g.status = GoalStatus::Active;
                    "Resume work on the goal from where you left off.".to_string()
                }
                None => {
                    println!("{}", ui::dim("  no goal set. /goal <objective>"));
                    return Ok(());
                }
            },
        };
        println!("{} {}", ui::accent("◎ goal"), ui::bold(&self.goal.as_ref().unwrap().objective));
        let max_turns: u32 = std::env::var("RUSTY_GOAL_MAX_TURNS").ok().and_then(|v| v.parse().ok()).unwrap_or(25);
        loop {
            let finished = self.run_turn(&prompt)?;
            self.save_session();
            let Some(g) = &mut self.goal else { return Ok(()) };
            g.turns += 1;
            if !finished || signal::interrupted() {
                println!("{}", ui::warn("  goal paused. /goal resume to continue"));
                return Ok(());
            }
            match &g.status {
                GoalStatus::Done(ev) => {
                    println!("{} {}\n", ui::ok("◉ goal done"), ev);
                    return Ok(());
                }
                GoalStatus::Blocked(why) => {
                    println!("{} {}\n", ui::warn("◌ goal blocked"), why);
                    return Ok(());
                }
                GoalStatus::Active => {}
            }
            if g.turns >= max_turns {
                println!("{}", ui::warn(&format!("  goal paused after {max_turns} turns. /goal resume to continue")));
                return Ok(());
            }
            let open = self.plan.iter().filter(|p| p.status != "done").count();
            println!("{}", ui::dim(&format!("  ↻ goal turn {}: not closed yet, continuing", g.turns + 1)));
            prompt = format!(
                "The goal is not closed yet ({open} plan items open). Keep going: pick the next step, do it, verify \
                 it. If everything is done and verified, call goal_done with evidence. If you are blocked on the \
                 user, call goal_done with blocked=true."
            );
        }
    }

    // ------------------------------------------------------------- loop mode

    /// Repeats `prompt` on a fixed interval, or self-paced when `interval` is None.
    pub fn run_loop(&mut self, prompt: &str, interval: Option<u64>, max_runs: Option<u32>) -> Result<()> {
        self.loop_ctl = Some(LoopCtl { self_paced: interval.is_none(), ..Default::default() });
        let mut run = 0;
        let out = loop {
            run += 1;
            println!("{}", ui::info(&format!("⟳ loop run {run}")));
            if let Some(l) = &mut self.loop_ctl {
                l.next_delay = None;
            }
            let text = match interval {
                Some(s) => format!("{prompt}\n\n(Loop run {run}; this prompt repeats every {s}s.)"),
                None => format!(
                    "{prompt}\n\n(Loop run {run}. This prompt repeats. When this run is done, call loop_next with \
                     how many seconds to wait before the next run, or stop=true if nothing is left to do.)"
                ),
            };
            if let Err(e) = self.run_turn(&text) {
                break Err(e);
            }
            self.save_session();
            let ctl = self.loop_ctl.as_ref().unwrap();
            if signal::interrupted() || ctl.stop || max_runs.is_some_and(|m| run >= m) {
                break Ok(());
            }
            let delay = interval.or(ctl.next_delay).unwrap_or(300);
            println!("{}", ui::dim(&format!("  next run in {delay}s · ctrl-c to stop")));
            signal::sleep(delay);
            if signal::interrupted() {
                break Ok(());
            }
        };
        self.loop_ctl = None;
        println!("{}", ui::dim("  loop stopped"));
        out
    }

    // --------------------------------------------------------------- context

    fn system_tokens(&self) -> usize {
        1_800 + self.project_notes.len() / 4 + context::MEMORY_CHARS / 4
    }

    pub fn context_pct(&self) -> usize {
        (self.last_prompt_tokens as usize * 100) / context::window().max(1)
    }

    fn manage_context(&mut self, d: &mut Display) {
        let budget = context::history_budget(self.system_tokens());
        if context::estimate(&self.history) > budget * 6 / 10 {
            let saved = context::elide_old_tool_outputs(&mut self.history);
            if saved > 0 {
                d.line(&ui::dim(&format!("  ✂ trimmed old tool output, ~{saved} tokens freed")));
            }
        }
        if context::estimate(&self.history) > budget * 8 / 10 {
            if let Err(e) = self.compact_with(d, None, false) {
                d.line(&ui::err(&format!("  compaction failed: {e:#}")));
            }
        }
    }

    /// `/compact [focus]`: summarise now, steered by what the user wants kept.
    pub fn compact(&mut self, focus: Option<&str>) -> Result<()> {
        let mut d = Display::new(false);
        self.compact_with(&mut d, focus, true)
    }

    /// Summarises older turns into one message. Every user message is kept
    /// word for word, a working set (files changed and read, recent commands,
    /// the last error) is rebuilt from the tool log, and lasting facts the
    /// summary surfaces are saved to memory.
    fn compact_with(&mut self, d: &mut Display, focus: Option<&str>, manual: bool) -> Result<()> {
        let split =
            if manual { context::manual_split(&self.history) } else { context::compaction_split(&self.history) };
        let Some(split) = split else {
            d.line(&ui::dim("  nothing to compact yet"));
            return Ok(());
        };
        let before = context::estimate(&self.history);
        if !manual && context::estimate(&self.history[..split]) * 4 < before {
            // The bulk is in recent messages, so summarising older ones won't help.
            let saved = context::elide_tool_outputs(&mut self.history, 1);
            if saved > 0 {
                d.line(&ui::dim(&format!("  ✂ context is mostly recent tool output; trimmed ~{saved} tokens")));
            }
            return Ok(());
        }
        let carried = context::user_messages(&self.history[..split], 8_000);
        let working_set = context::working_set(&self.history[..split]);
        let mut messages =
            vec![json!({"role": "system", "content": "You write precise hand-off summaries of coding sessions."})];
        messages.extend(self.history[..split].iter().cloned());
        messages.push(json!({"role": "user", "content": context::compact_prompt(focus)}));
        d.set_status(match focus {
            Some(f) if !f.trim().is_empty() => format!("compacting · focus: {}", ui::truncate(f.trim(), 40)),
            _ => "compacting".into(),
        });
        let reply = self.ask(&mut Display::new(true), messages, json!([]));
        d.set_status(String::new());
        let reply = reply?;
        if let Some(u) = reply.usage {
            self.totals.add_request(&self.model, u.prompt, u.completion);
        }
        let (summary, memories) = context::parse_compaction(&reply.content);
        if summary.is_empty() {
            bail!("the model returned an empty summary");
        }
        for (kind, text) in &memories {
            self.memory.add(kind, text, false);
        }
        self.memory.save();
        let tail = self.history.split_off(split);
        let mut block = format!("[Summary of the conversation so far]\n{summary}\n\n");
        if !working_set.is_empty() {
            block.push_str(&format!("{}\n{working_set}\n", context::WORKING_SET_HEADER));
        }
        // The verbatim user messages go last; later compactions read them back from here.
        block.push_str(&format!("{}\n{carried}", context::USER_MESSAGES_HEADER));
        self.history = vec![json!({"role": "user", "content": block})];
        // Keep user/assistant alternation when the kept tail starts with a user turn.
        if tail.first().is_some_and(|m| m["role"] == "user") {
            self.history.push(json!({"role": "assistant", "content": "Got it. I'll continue from this summary."}));
        }
        self.history.extend(tail);
        d.line(&ui::dim(&format!(
            "  ⇣ compacted ~{} → ~{} tokens{}",
            ui::human(before as u64),
            ui::human(context::estimate(&self.history) as u64),
            if memories.is_empty() { String::new() } else { format!(", {} memories saved", memories.len()) }
        )));
        Ok(())
    }

    pub fn context_report(&self) -> String {
        let (system, _) = self.system_prompt("");
        let sys = system.len() / 4;
        let hist = context::estimate(&self.history);
        let win = context::window();
        let tools_n = self.history.iter().filter(|m| m["role"] == "tool").count();
        let used = (sys + hist) * 100 / win.max(1);
        let bar_w = 30;
        let filled = (used * bar_w / 100).min(bar_w);
        let bar = format!("{}{}", ui::primary(&"█".repeat(filled)), ui::dim(&"░".repeat(bar_w - filled)));
        format!(
            "  {bar} ~{used}%\n  window        {win}\n  system+memory ~{sys}\n  history       ~{hist} ({} messages, {tools_n} tool results)\n  reply reserve {}\n  last request  {} prompt tokens (reported by the API)",
            self.history.len(),
            context::REPLY_RESERVE,
            self.last_prompt_tokens,
        )
    }

    // --------------------------------------------------------------- session

    pub fn save_session(&mut self) {
        self.memory.save();
        let Some(path) = &self.session_path else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let body = json!({"model": self.model, "history": self.history, "plan": self.plan, "goal": self.goal});
        let _ = std::fs::write(path, body.to_string());
    }

    pub fn load_session(&mut self, path: &std::path::Path) -> Result<()> {
        let v: Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
        self.history = v["history"].as_array().cloned().unwrap_or_default();
        self.plan = serde_json::from_value(v["plan"].clone()).unwrap_or_default();
        self.goal = serde_json::from_value(v["goal"].clone()).unwrap_or(None);
        self.session_path = Some(path.to_path_buf());
        Ok(())
    }

    /// Appends one line per request to `<session>.trace.jsonl` for later analysis.
    fn trace(&self, reply: &Reply, took: Duration) {
        if self.is_worker {
            return;
        }
        let Some(path) = &self.session_path else { return };
        let line = json!({
            "t": crate::memory::now(),
            "model": self.model,
            "ms": took.as_millis() as u64,
            "prompt": reply.usage.map(|u| u.prompt),
            "completion": reply.usage.map(|u| u.completion),
            "tools": reply.tool_calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            "finish": reply.finish_reason,
        });
        use std::io::Write;
        if let Ok(mut f) =
            std::fs::OpenOptions::new().create(true).append(true).open(path.with_extension("trace.jsonl"))
        {
            let _ = writeln!(f, "{line}");
        }
    }
}

fn job(v: &Value) -> (String, String) {
    (v["description"].as_str().unwrap_or("worker").to_string(), v["prompt"].as_str().unwrap_or("").to_string())
}

fn load_project_notes(cwd: &std::path::Path) -> String {
    let mut out = String::new();
    for name in ["AGENTS.md", "RUSTY.md", "CLAUDE.md"] {
        if let Ok(text) = std::fs::read_to_string(cwd.join(name)) {
            out.push_str(&format!("--- {name} ---\n{}\n", ui::truncate(text.trim(), 6_000)));
        }
    }
    ui::truncate(&out, 10_000).to_string()
}

enum Answer {
    Yes,
    Always,
    No(Option<String>),
}

/// Asks for approval. Uses line editing so Ctrl-C cleanly means "no, stop".
fn ask_user(why: &str) -> Answer {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        return Answer::No(Some("non-interactive session; the call needs approval".into()));
    }
    println!("  {} {}", ui::warn("?"), why);
    let prompt = format!("  {} ", ui::dim("[y]es  [a]lways  [n]o  or say what to do instead ›"));
    let line = match rustyline::DefaultEditor::new().and_then(|mut rl| rl.readline(&prompt)) {
        Ok(l) => l,
        Err(rustyline::error::ReadlineError::Interrupted) => {
            signal::trip();
            return Answer::No(Some("the user pressed ctrl-c to stop".into()));
        }
        Err(_) => return Answer::No(None),
    };
    match line.trim() {
        "" | "y" | "Y" | "yes" => Answer::Yes,
        "a" | "A" | "always" => Answer::Always,
        "n" | "N" | "no" => Answer::No(None),
        other => Answer::No(Some(other.to_string())),
    }
}

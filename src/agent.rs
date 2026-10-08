//! The agent loop: ask the model, run the tools it calls, repeat until it
//! answers. Goal mode, loop mode, subagents and swarms are built on one turn.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config::{AgentsConfig, AgentsMode};
use crate::context;
use crate::display::{self, Display, View};
use crate::execution::ExecutionMode;
use crate::infra;
use crate::llm::{Client, Event, Reply, ToolCall};
use crate::memory::{Memory, KINDS};
use crate::permissions::{Mode, Policy, Verdict};
use crate::signal;
use crate::tips;
use crate::tools;
use crate::ui;
use crate::verification::{self, CheckRecord, CheckSpec};
use rusty::advisor::{Hooks, Mode as MemoryMode};

const MAX_STEPS: usize = 80;
const WORKER_STEPS: usize = 30;
const TRAJECTORY_EVERY: Duration = Duration::from_secs(30);

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
    /// One checker attempt per goal, including after saving and resuming.
    #[serde(default)]
    pub checker_used: bool,
    /// A fixed criterion chosen by the user, not supplied by goal_done.
    #[serde(default)]
    pub check: Option<CheckSpec>,
}

#[derive(Default)]
struct LoopCtl {
    self_paced: bool,
    next_delay: Option<u64>,
    stop: bool,
}

enum Authorization {
    Allowed(Verdict),
    Refused(String),
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
    pub backend: crate::backend::Backend,
    client: Arc<Client>,
    pub model: String,
    pub temperature: f32,
    pub execution_mode: ExecutionMode,
    /// The user chose the mode; otherwise rusty picks one per request.
    pub mode_fixed: bool,
    /// Why the current mode was picked, when rusty picked it.
    pub mode_reason: String,
    pub model_override: Option<String>,
    pub delegation_override: bool,
    pending_goal: Option<Value>,
    /// Careful mode still owes one checker review for this turn or goal.
    checker_owed: bool,
    pub cwd: PathBuf,
    pub history: Vec<Value>,
    /// Messages that compaction or clearing removed from `history`, oldest
    /// first. Only the exported trajectory reads them.
    pub archived: Vec<Value>,
    /// Where `--trajectory` goes; long runs refresh it as they go so a run
    /// killed from outside still leaves a record.
    pub trajectory_path: Option<PathBuf>,
    trajectory_saved: Option<Instant>,
    pub policy: Policy,
    pub memory: Memory,
    pub memory_mode: MemoryMode,
    pub advisor: Option<Hooks>,
    pub plan: Vec<PlanItem>,
    pub goal: Option<Goal>,
    pub goal_check: Option<CheckSpec>,
    /// Observations are exported, but never reloaded as acceptance authority.
    pub verification_records: Vec<CheckRecord>,
    loop_ctl: Option<LoopCtl>,
    pub agents: AgentsConfig,
    pub tips: bool,
    tips_shown: HashSet<&'static str>,
    pub totals: Totals,
    pub last_prompt_tokens: u64,
    project_notes: String,
    pub session_path: Option<PathBuf>,
    /// Target, audit log and change record; inert for workers.
    pub infra: infra::Harness,
    is_worker: bool,
    progress: Option<Arc<AtomicUsize>>,
}

impl Agent {
    pub fn remember(&mut self, kind: &str, text: &str, global: bool) -> String {
        if self.memory_mode == MemoryMode::Off {
            return "memory is off".into();
        }
        if let Some(h) = &mut self.advisor {
            return h.control("remember", json!({"kind":kind,"text":infra::redact(text).0,"global":false})).text;
        }
        self.memory.add(kind, text, global)
    }

    pub fn forget_memory(&mut self, id: &str) -> String {
        if self.memory_mode == MemoryMode::Off {
            return "memory is off".into();
        }
        if let Some(h) = &mut self.advisor {
            return h.control("forget", json!({"id":id})).text;
        }
        if self.memory.forget(id) {
            format!("forgot {id}")
        } else {
            format!("no memory with id {id}")
        }
    }

    pub fn new(client: Arc<Client>, model: String, cwd: PathBuf, policy: Policy, memory: Memory) -> Self {
        let project_notes = load_project_notes(&cwd);
        Self {
            backend: crate::backend::Backend::Local,
            client,
            model,
            temperature: 0.2,
            execution_mode: ExecutionMode::Standard,
            mode_fixed: false,
            mode_reason: String::new(),
            model_override: None,
            delegation_override: false,
            pending_goal: None,
            checker_owed: false,
            cwd,
            history: Vec::new(),
            archived: Vec::new(),
            trajectory_path: None,
            trajectory_saved: None,
            policy,
            memory,
            memory_mode: MemoryMode::Legacy,
            advisor: None,
            plan: Vec::new(),
            goal: None,
            goal_check: None,
            verification_records: Vec::new(),
            loop_ctl: None,
            agents: AgentsConfig::default(),
            tips: true,
            tips_shown: HashSet::new(),
            totals: Totals::default(),
            last_prompt_tokens: 0,
            project_notes,
            session_path: None,
            infra: infra::Harness::default(),
            is_worker: false,
            progress: None,
        }
    }

    pub fn set_execution_mode(&mut self, mode: ExecutionMode) {
        self.execution_mode = mode;
        self.model = self.model_override.clone().or_else(|| mode.model()).unwrap_or_else(crate::config::default_model);
        if !self.delegation_override {
            self.agents.mode = if mode == ExecutionMode::Vibe { AgentsMode::Auto } else { AgentsMode::Off };
        }
    }

    pub fn set_backend(&mut self, backend: crate::backend::Backend) -> Result<()> {
        if backend.name() == "daytona" {
            let description = backend.initial_description();
            self.cwd = backend.cwd(&self.cwd);
            self.project_notes = description["notes"].as_str().unwrap_or("").to_string();
            self.infra.target = serde_json::from_value(description["target"].clone())?;
        }
        self.backend = backend;
        Ok(())
    }

    /// Auto's pick for a request; a production target is always careful.
    fn pick_mode(&self, request: &str) -> (ExecutionMode, String) {
        if self.infra.target.production {
            return (ExecutionMode::Careful, "the target looks like production".into());
        }
        crate::execution::pick(request)
    }

    fn show_pick(&self, d: &mut Display) {
        let name = self.execution_mode.name();
        d.line(&format!("  {} {}", ui::accent(&format!("◇ {name}")), ui::dim(&format!("auto · {}", self.mode_reason))));
    }

    /// Switches profile without touching the model or permissions; used when
    /// rusty picks the mode itself.
    fn apply_profile(&mut self, mode: ExecutionMode, reason: String) {
        self.execution_mode = mode;
        self.mode_reason = reason;
        if !self.delegation_override {
            self.agents.mode = if mode == ExecutionMode::Vibe { AgentsMode::Auto } else { AgentsMode::Off };
        }
    }

    /// Moves an auto-picked turn up to careful, once, and says why.
    fn escalate(&mut self, why: &str, d: &mut Display) {
        if self.mode_fixed || self.is_worker || self.execution_mode == ExecutionMode::Careful {
            return;
        }
        self.apply_profile(ExecutionMode::Careful, why.to_string());
        self.checker_owed = !self.goal.as_ref().is_some_and(|g| g.status == GoalStatus::Active && g.checker_used);
        d.line(&format!("  {} {}", ui::accent("◇ careful"), ui::dim(&format!("auto · switched up: {why}"))));
    }

    /// The most workers one swarm may start. Vibe keeps it small unless the
    /// user chose delegation settings themselves.
    pub fn swarm_cap(&self) -> usize {
        let cap = if self.execution_mode == ExecutionMode::Vibe && !self.delegation_override {
            self.agents.swarm_max.min(ExecutionMode::VIBE_WORKERS)
        } else {
            self.agents.swarm_max
        };
        if self.backend.name() == "daytona" {
            cap.min(8)
        } else {
            cap
        }
    }

    /// Careful mode's one review: a read-only worker checks the proposed
    /// completion against the real files and the user's constraints. The main
    /// agent owns the fixes; there is no second review.
    fn review_completion(&mut self, query: &str, d: &mut Display) {
        self.checker_owed = false;
        if let Some(g) = &mut self.goal {
            g.checker_used = true;
        }
        d.line(&format!("  {} {}", ui::accent("◇ careful"), ui::dim("a read-only checker is reviewing the result")));
        let constraints = context::user_messages(&self.history, 8_000);
        let working = context::working_set(&self.history);
        let recent: Vec<Value> = self.history.iter().rev().take(12).rev().cloned().collect();
        let recent = serde_json::to_string(&recent).unwrap_or_default();
        let prompt = format!(
            "Check this proposed completion once, as a read-only reviewer. Look at the actual files or state it \
             touched, compare the work with everything the user asked for, and judge the test evidence. Look for \
             concrete bugs, regressions, and security or data-integrity problems where they matter. You may read \
             and run read-only commands, never change anything. Report actionable findings with file and line \
             references, or say you found none and name what you could not verify. Keep it short.\n\n\
             Task: {query}\n\nWhat the user asked for:\n{constraints}\n\nWorking set:\n{working}\n\n\
             Recent steps and the proposed completion:\n{}",
            ui::truncate(&recent, 24_000)
        );
        let report = self.run_workers(vec![("careful check".into(), prompt)], d);
        self.history.push(json!({"role": "user", "content": format!(
            "{}[checker's review]\n{report}\n\nFix anything actionable it found and verify the fix the usual way. \
             There is no second review. If the checker failed or couldn't verify something, say so plainly. Then \
             give your final answer, or call goal_done with evidence.",
            context::NOTE
        )}));
    }

    pub fn clear(&mut self) {
        self.archived.append(&mut self.history);
        self.plan.clear();
        self.goal = None;
        self.verification_records.clear();
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
        s.push_str(self.execution_mode.instructions());
        s.push('\n');
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
                self.swarm_cap()
            )),
            AgentsMode::Auto => s.push_str(&format!(
                "\nDelegation: task runs one read-only subagent; swarm runs up to {} in parallel. Pick the lightest \
                 option that works: do it yourself if it takes fewer than about five reads, use task for one deep \
                 investigation, and use swarm only when the work splits into three or more independent parts.\n",
                self.swarm_cap()
            )),
        }
        s.push_str(&format!(
            "\nEnvironment: {} {}. Commands already run with bash in the project directory, so use relative \
             paths and never cd into it. Permission mode: {}.\n",
            std::env::consts::OS,
            std::env::consts::ARCH,
            self.policy.mode.name()
        ));
        s.push_str(&self.infra.target.prompt_block(self.execution_mode == ExecutionMode::Careful));
        if !self.project_notes.is_empty() {
            s.push_str("\nProject instructions:\n");
            s.push_str(&self.project_notes);
            s.push('\n');
        }
        let (mem, ids) = if self.memory_mode == MemoryMode::Legacy {
            self.memory.context_block(query, context::MEMORY_CHARS)
        } else {
            (String::new(), Vec::new())
        };
        if !mem.is_empty() {
            s.push_str("\nMemories relevant to this request (from earlier sessions; verify before relying on them):\n");
            s.push_str(&mem);
        }
        if let Some(g) = &self.goal {
            s.push_str(&format!("\nCurrent goal (keep working until it is done and verified): {}\n", g.objective));
            if let Some(check) = &g.check {
                s.push_str(&format!("\nFixed acceptance command: {}\nThe runtime executes it when you propose goal_done. You cannot replace this criterion with evidence text. Keep verification inputs unchanged while it runs.\n", check.command()));
            }
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
                json!({"tasks": {"type": "array", "items": job, "maxItems": self.swarm_cap()}}),
                &["tasks"],
            ));
        }
        if self.goal.as_ref().is_some_and(|g| g.status == GoalStatus::Active) {
            defs.push(t(
                "goal_done",
                "Propose completion after all calls in this reply. A fixed user acceptance command, if set, must pass. Use blocked=true only when you need the user.",
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
        if let Some(h) = &mut self.advisor {
            h.begin(&infra::redact(user).0);
        }
        let query = match &self.goal {
            Some(g) => format!("{user} {}", g.objective),
            None => user.to_string(),
        };
        let max_steps = if self.is_worker { WORKER_STEPS } else { MAX_STEPS };
        let mut d = Display::new(self.is_worker);
        let before = self.totals.clone();
        // Only consecutive unchanged results count: an edit, a different call,
        // or new output is progress. Retain one call rather than a turn-sized map.
        let mut previous_call: Option<(String, String)> = None;
        let mut repeats = 0;
        let mut outcome: Result<bool> = Ok(true);
        self.pending_goal = None;
        // Goals keep one review budget across all their turns (set in run_goal);
        // every other turn gets its own.
        let in_goal = self.goal.as_ref().is_some_and(|g| g.status == GoalStatus::Active);
        if !self.mode_fixed && !self.is_worker && !in_goal {
            let (mode, why) = self.pick_mode(user);
            self.apply_profile(mode, why);
            self.show_pick(&mut d);
        }
        if !in_goal {
            self.checker_owed = !self.is_worker && self.execution_mode == ExecutionMode::Careful;
        }
        // The checker only reviews turns that changed something.
        let mut changed = false;
        let mut failures_in_a_row = 0;
        // Commands that only print text: announcements no one reads.
        let mut talk_only = 0;
        let mut empty_replies = 0;

        for step in 0..max_steps {
            if signal::interrupted() {
                outcome = Ok(false);
                break;
            }
            self.manage_context(&mut d);
            let (mut system, mem_ids) = self.system_prompt(&query);
            if let Some(h) = &mut self.advisor {
                system.push_str(&h.advice());
            }
            if step == 0 {
                self.memory.touch(&mem_ids);
            }
            let mut messages = Vec::with_capacity(self.history.len() + 1);
            messages.push(json!({"role": "system", "content": system}));
            messages.extend(self.history.iter().cloned());

            let started = Instant::now();
            let mut reply = match self.ask(&mut d, messages, self.tool_defs()) {
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

            // Fresh background analysis can arrive while the main model thinks.
            // Reconsider an unexecuted tool proposal at most within the advisor budget.
            if !reply.tool_calls.is_empty() {
                if let Some(h) = &mut self.advisor {
                    let advice = h.late_advice();
                    if !advice.is_empty() {
                        self.history.push(json!({"role":"user","content":advice}));
                        continue;
                    }
                }
            }
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
            if let Some(raw) = reply.raw.take() {
                msg[crate::anthropic::BLOCKS_KEY] = raw;
            }
            self.history.push(msg);

            if reply.finish_reason.as_deref() == Some("interrupted") {
                outcome = Ok(false);
                break;
            }
            if reply.tool_calls.is_empty() {
                if reply.content.trim().is_empty() && reply.finish_reason.as_deref() != Some("length") {
                    empty_replies += 1;
                    if empty_replies > 1 {
                        outcome =
                            Err(anyhow::anyhow!("model returned two empty replies; task completion is unconfirmed"));
                        break;
                    }
                    self.history.push(json!({"role":"user","content":"The previous reply was empty. Continue the existing task using the available tools, or give a substantive final answer. Do not repeat completed actions."}));
                    continue;
                }
                if reply.finish_reason.as_deref() == Some("length") {
                    d.line(&ui::warn("  (the reply hit the token limit; say \"continue\" to resume)"));
                }
                if reply.finish_reason.as_deref() != Some("length") && changed && !in_goal && self.checker_owed {
                    self.review_completion(&query, &mut d);
                    if signal::interrupted() {
                        outcome = Ok(false);
                        break;
                    }
                    continue;
                }
                break;
            }
            for call in &reply.tool_calls {
                let output = if signal::interrupted() {
                    "skipped: the user interrupted".to_string()
                } else if outcome.is_err() {
                    "skipped: the turn stalled; task completion is unconfirmed".to_string()
                } else {
                    let args: Value = serde_json::from_str(&call.arguments).unwrap_or(Value::Null);
                    // JSON whitespace/key order must not bypass repeat detection.
                    let signature = format!("{}:{args}", call.name);
                    let class = crate::permissions::classify(&call.name, &args, &self.cwd);
                    if let crate::permissions::Class::Risky(why) | crate::permissions::Class::Destructive(why) = &class
                    {
                        self.escalate(&format!("consequential step ({why})"), &mut d);
                    }
                    let mut out = self.dispatch(call, &mut d);
                    // Only a call that actually ran changes anything.
                    let refused =
                        ["denied", "the user declined", "blocked by careful mode"].iter().any(|p| out.starts_with(p));
                    if class != crate::permissions::Class::ReadOnly && !refused {
                        changed = true;
                    }
                    if call.name == "bash" {
                        let failed = out.starts_with("exit code: ") && !out.starts_with("exit code: 0\n");
                        failures_in_a_row = if failed { failures_in_a_row + 1 } else { 0 };
                        if failures_in_a_row == 3 {
                            self.escalate("three commands failed in a row", &mut d);
                        }
                        if crate::tools::only_prints(args["command"].as_str().unwrap_or("")) {
                            talk_only += 1;
                            out.push_str(
                                "\n\nnote: this command only printed text, and no one reads tool output but you. \
                                 When the work is done, give your answer as a plain reply with no tool call; \
                                 that ends the turn.",
                            );
                            if talk_only == 5 {
                                outcome = Err(anyhow::anyhow!(
                                    "stopped after five commands that only printed text; the work may be done, \
                                     but its summary never came as a reply"
                                ));
                            }
                        }
                    }
                    if matches!(call.name.as_str(), "plan" | "recall" | "goal_done" | "loop_next") {
                        previous_call = None;
                        repeats = 0;
                    } else {
                        repeats = if previous_call.as_ref().is_some_and(|(s, o)| s == &signature && o == &out) {
                            repeats + 1
                        } else {
                            1
                        };
                        previous_call = Some((signature, out.clone()));
                    }
                    if repeats == 3 {
                        self.escalate("the same call keeps repeating", &mut d);
                    }
                    if repeats == 4 {
                        outcome = Err(anyhow::anyhow!(
                            "stopped after four consecutive identical tool calls with unchanged results; \
                             task completion is unconfirmed. Inspect the result and try a different approach."
                        ));
                    }
                    if repeats >= 3 {
                        out.push_str(&format!(
                            "\n\nnote: this call has returned the same result {repeats} times consecutively. \
                             Step back, re-read the result, and try a different approach."
                        ));
                    }
                    out
                };
                self.history.push(json!({"role": "tool", "tool_call_id": call.id, "content": output}));
            }
            self.checkpoint_trajectory();
            if outcome.is_err() {
                self.pending_goal = None;
                break;
            }
            if signal::interrupted() {
                self.pending_goal = None;
                outcome = Ok(false);
                break;
            }
            if reply.finish_reason.as_deref() == Some("length") {
                self.pending_goal = None;
                d.line(&ui::warn("  (the reply hit the token limit; say \"continue\" to resume)"));
                break;
            }
            if let Some(args) = self.pending_goal.take() {
                if self.checker_owed && !args["blocked"].as_bool().unwrap_or(false) {
                    self.review_completion(&query, &mut d);
                    if signal::interrupted() {
                        outcome = Ok(false);
                        break;
                    }
                } else {
                    let result = self.close_goal(&args, &mut d);
                    if self.goal.as_ref().is_some_and(|g| g.status == GoalStatus::Active) {
                        self.history.push(json!({"role":"user", "content":format!("{}Completion rejected: {result}\nFix the cause before proposing completion again, or explain what blocks the goal.", context::NOTE)}));
                    }
                }
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
        if let Some(h) = &mut self.advisor {
            h.finish(outcome.is_ok() && !interrupted);
        }
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
        let rx = self.client.start_chat(self.model.clone(), messages, tools, self.temperature, self.execution_mode);
        loop {
            // Checked on every event: a model that streams steadily never goes quiet.
            if signal::interrupted() {
                let content = std::mem::take(&mut d.content);
                d.end_stream();
                return Ok(Reply { content, finish_reason: Some("interrupted".into()), ..Default::default() });
            }
            signal::poll_keys();
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
        if self.is_worker
            && !matches!(call.name.as_str(), "read_file" | "list_files" | "search" | "glob" | "outline" | "bash")
        {
            return "denied: a read-only worker can only inspect files and run read-only commands".into();
        }
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
            "remember" => Ok(self.remember(
                args["kind"].as_str().unwrap_or("fact"),
                args["text"].as_str().unwrap_or(""),
                args["global"].as_bool().unwrap_or(false),
            )),
            "recall" => Ok(self.recall(args["query"].as_str().unwrap_or(""))),
            "forget" => {
                let id = args["id"].as_str().unwrap_or("");
                Ok(self.forget_memory(id))
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
        if ok && matches!(call.name.as_str(), "write_file" | "edit_file") {
            self.infra.invalidate(args["path"].as_str().unwrap_or(""));
        }
        // Secrets never reach the model, the screen or the session file.
        let (mut text, redacted) = infra::redact(&text);
        if let Some(h) = &mut self.advisor {
            h.event(&call.name, &infra::redact(&call.arguments).0, &text);
        }
        if redacted > 0 {
            text.push_str(&infra::redaction_note(redacted));
        }
        d.tool_result(&call.name, &args, &text, ok);
        if redacted > 0 {
            d.line(&format!(
                "    {}",
                ui::dim(&format!("⊘ {redacted} secret{} redacted", if redacted == 1 { "" } else { "s" }))
            ));
        }
        text
    }

    /// Runs a filesystem/shell tool through the permission policy.
    fn run_guarded(&mut self, name: &str, args: &Value, d: &mut Display) -> Result<String> {
        let approval = match self.authorize(name, args, d)? {
            Authorization::Allowed(approval) => approval,
            Authorization::Refused(why) => return Ok(why),
        };
        if name == "bash" {
            return self.run_bash(args, &approval, d);
        }
        self.backend.execute(name, args, &self.policy, &approval)
    }

    fn authorize(&mut self, name: &str, args: &Value, d: &mut Display) -> Result<Authorization> {
        let mut approval = self.backend.decide(name, args, &self.policy, &self.cwd)?;
        match approval.clone() {
            Verdict::Allow => {}
            Verdict::Deny(why) => {
                self.refused(name, args, "denied", &why);
                return Ok(Authorization::Refused(format!(
                    "denied by permissions ({why}). Do not retry this call; choose another approach or ask the user."
                )));
            }
            Verdict::Confirm(why) if unattended_destructive(self.policy.mode) => {
                d.line(&ui::warn(&format!("  ‼ {why}: allowed unattended by RUSTY_ALLOW_DESTRUCTIVE")));
            }
            Verdict::Confirm(why) => {
                d.pause();
                if display::view() == View::Adhd {
                    println!("{} {}", ui::info("▸"), tools::summary(name, args));
                }
                print!("{}", tools::preview(name, args));
                match ask_user(&why, true) {
                    Answer::Yes | Answer::Always => {}
                    Answer::No(None) => {
                        self.refused(name, args, "declined", &why);
                        return Ok(Authorization::Refused(
                            "the user declined this destructive call. Do not retry it or look for another \
                                   route to the same result; ask what they want instead."
                                .into(),
                        ));
                    }
                    Answer::No(Some(fb)) => {
                        self.refused(name, args, "declined", &fb);
                        return Ok(Authorization::Refused(format!("the user declined this call and said: {fb}")));
                    }
                }
            }
            Verdict::Ask(why) => {
                d.pause();
                if display::view() == View::Adhd {
                    println!("{} {}", ui::info("▸"), tools::summary(name, args));
                }
                print!("{}", tools::preview(name, args));
                match ask_user(&why, false) {
                    Answer::Yes => {}
                    Answer::Always => {
                        let rule = Policy::suggest_rule(name, args);
                        println!("  {}", ui::dim(&format!("allowing `{rule}` from now on (/permissions to undo)")));
                        self.policy.allow.push(rule);
                        self.policy.save();
                        approval = Verdict::Allow;
                    }
                    Answer::No(None) => {
                        self.refused(name, args, "declined", "");
                        return Ok(Authorization::Refused(
                            "the user declined this call. Do not retry it; ask what they want instead.".into(),
                        ));
                    }
                    Answer::No(Some(fb)) => {
                        self.refused(name, args, "declined", &fb);
                        return Ok(Authorization::Refused(format!("the user declined this call and said: {fb}")));
                    }
                }
            }
        }
        Ok(Authorization::Allowed(approval))
    }

    fn check_goal(&mut self, check: &CheckSpec, d: &mut Display) -> Result<CheckRecord> {
        check.validate()?;
        if self.backend.name() != "local" || std::env::current_dir()?.canonicalize()? != self.cwd.canonicalize()? {
            bail!("fixed verification needs the current local workspace; no fallback or replay");
        }
        // Infrastructure acceptance already has a separate gate and receipts.
        // Do not accidentally bypass its dry-run/snapshot/health-check path.
        if infra::inspect(check.command()).is_some_and(|a| a.mutating) {
            bail!("fixed acceptance must be a check, not an infrastructure change");
        }
        let args = json!({"command":check.command(), "timeout_secs":check.timeout().as_secs()});
        if let Authorization::Refused(why) = self.authorize("bash", &args, d)? {
            bail!("{why}");
        }
        let before = verification::fingerprint(&self.cwd)?;
        d.tool("bash", &args);
        let started = Instant::now();
        let (status, stdout, stderr) = self.backend.shell(check.command(), check.timeout())?;
        let output = infra::redact(&format!("stdout:\n{stdout}\nstderr:\n{stderr}")).0;
        let after = verification::fingerprint(&self.cwd)?;
        let record = CheckRecord::observed(
            check,
            before,
            after,
            status,
            signal::interrupted(),
            started.elapsed().as_secs_f64(),
            output,
        );
        let summary = record.summary();
        d.tool_result("bash", &args, &summary, record.passed());
        self.history.push(
            json!({"role":"user", "content":format!("{}Runtime acceptance observation:\n{summary}", context::NOTE)}),
        );
        Ok(record)
    }

    /// A shell command that never ran still gets its audit line.
    fn refused(&mut self, name: &str, args: &Value, outcome: &str, note: &str) {
        if name == "bash" {
            let mode = self.execution_mode.name();
            self.infra.record_refusal(mode, args["command"].as_str().unwrap_or(""), outcome, note);
        }
    }

    /// Runs a shell command through the infra harness: a change gets a
    /// snapshot of what it is about to touch first, and everything that
    /// touches infrastructure is logged.
    fn run_bash(&mut self, args: &Value, approval: &Verdict, d: &mut Display) -> Result<String> {
        let cmd = args["command"].as_str().unwrap_or("").to_string();
        let Some(action) = infra::inspect(&cmd) else {
            return self.backend.execute("bash", args, &self.policy, approval);
        };
        if self.execution_mode == ExecutionMode::Careful {
            if let Some(refusal) = self.infra.gate(&action) {
                self.infra.record_refusal("careful", &cmd, "blocked", "needs a dry run first");
                return Ok(refusal);
            }
        }
        let mut snapshot = None;
        let mut note = String::new();
        if action.mutating && !action.snapshots.is_empty() {
            d.set_status("snapshotting before the change".into());
            d.tick();
            match self.backend.snapshot(&self.infra, &action, &cmd) {
                Ok(path) => {
                    d.line(&format!("  {} {}", ui::accent("⎘ snapshot"), ui::dim(&path.display().to_string())));
                    snapshot = Some(path.display().to_string());
                }
                Err(why) => {
                    d.line(&format!("  {} {}", ui::dim("⎘"), ui::dim(&why)));
                    note = why;
                }
            }
            d.set_status(String::new());
        }
        let started = Instant::now();
        let mut out = self.backend.execute("bash", args, &self.policy, approval)?;
        let (outcome, exit) = infra::outcome(&out);
        self.infra.saw_dry_run(&action, exit);
        // In careful mode a change is not done until it is seen to be healthy.
        let mut verified = None;
        if self.execution_mode == ExecutionMode::Careful
            && action.mutating
            && exit == Some(0)
            && action.verify != infra::Verify::None
        {
            d.set_status("verifying the change".into());
            d.tick();
            let (ok, what) = if self.backend.name() == "local" {
                self.infra.verify(&action)
            } else {
                self.infra.verify_with(&action, |command, timeout| self.backend.shell(command, timeout))
            };
            d.set_status(String::new());
            verified = Some(ok);
            if ok {
                d.line(&format!("  {} {}", ui::ok("✓ verified"), ui::dim(&what)));
                out.push_str(&format!("\n[verified: {what}]"));
            } else {
                d.line(&format!("  {} {}", ui::err("✗ not verified"), ui::dim(&what)));
                out.push_str(&format!(
                    "\n[verification FAILED: {what}. The change is NOT healthy; do not report it as done. Look at \
                     describe, events and logs before deciding whether to fix forward or roll back.]"
                ));
            }
        }
        if let Some(path) = &snapshot {
            out.push_str(&format!(
                "\n[pre-change snapshot of the previous state: {path}. Rollback: {}]",
                action.rollback.replace("SNAPSHOT", path)
            ));
        }
        let rec = infra::Record {
            command: cmd,
            action,
            outcome,
            exit,
            secs: started.elapsed().as_secs_f32(),
            snapshot,
            verified,
            note,
            ..infra::empty_record()
        };
        self.infra.record(self.execution_mode.name(), rec);
        Ok(out)
    }

    fn recall(&mut self, query: &str) -> String {
        if self.memory_mode == MemoryMode::Off {
            return "memory is off".into();
        }
        if let Some(h) = &mut self.advisor {
            return h.control("recall", json!({"query": infra::redact(query).0})).text;
        }
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
        let n = jobs.len().min(self.swarm_cap().max(1));
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
            let execution_mode = self.execution_mode;
            let backend = self.backend.clone();
            let (client, cwd, deny, tx) = (self.client.clone(), self.cwd.clone(), self.policy.deny.clone(), tx.clone());
            std::thread::spawn(move || {
                let mut w = Agent::new(client, model.clone(), cwd, Policy::new(Mode::ReadOnly), Memory::empty());
                w.is_worker = true;
                w.backend = backend;
                w.execution_mode = execution_mode;
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
                    signal::poll_keys();
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
        self.pending_goal = Some(args.clone());
        "completion proposed; acceptance happens after this reply's remaining calls and any required review".into()
    }

    fn close_goal(&mut self, args: &Value, d: &mut Display) -> String {
        let evidence = args["evidence"].as_str().unwrap_or("").to_string();
        if !args["blocked"].as_bool().unwrap_or(false) {
            let check = self.goal.as_ref().and_then(|g| g.check.clone());
            if let Some(check) = check {
                match self.check_goal(&check, d) {
                    Ok(record) => {
                        let passed = record.passed();
                        let reason = record.summary();
                        self.verification_records.push(record);
                        if !passed {
                            return reason;
                        }
                    }
                    Err(e) => return format!("fixed verification could not run: {e:#}"),
                }
            }
        }
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
                self.verification_records.clear();
                self.goal = Some(Goal {
                    objective: obj.to_string(),
                    status: GoalStatus::Active,
                    turns: 0,
                    checker_used: false,
                    check: self.goal_check.clone(),
                });
                format!(
                    "{}New goal: {obj}\n\nWork on this autonomously until it is done. Start with a plan (plan tool). \
                     Verify as you go with builds, tests or by running the code. When the goal is achieved and \
                     verified, call goal_done with the evidence. If you are blocked on something only the user can \
                     provide, call goal_done with blocked=true and say what you need.",
                    context::NOTE
                )
            }
            None => match &mut self.goal {
                Some(g) => {
                    g.status = GoalStatus::Active;
                    format!("{}Resume work on the goal from where you left off.", context::NOTE)
                }
                None => {
                    println!("{}", ui::dim("  no goal set. /goal <objective>"));
                    return Ok(());
                }
            },
        };
        println!("{} {}", ui::accent("◎ goal"), ui::bold(&self.goal.as_ref().unwrap().objective));
        if !self.mode_fixed {
            let (mode, why) = self.pick_mode(&self.goal.as_ref().unwrap().objective.clone());
            self.apply_profile(mode, why);
            self.show_pick(&mut Display::new(false));
        }
        self.checker_owed =
            self.execution_mode == ExecutionMode::Careful && !self.goal.as_ref().is_some_and(|g| g.checker_used);
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
                "{}The goal is not closed yet ({open} plan items open). Keep going: pick the next step, do it, verify \
                 it. If everything is done and verified, call goal_done with evidence. If you are blocked on the \
                 user, call goal_done with blocked=true.",
                context::NOTE
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
        1_800
            + self.project_notes.len() / 4
            + context::MEMORY_CHARS / 4
            + (self.execution_mode.max_tokens() as usize).saturating_sub(context::REPLY_RESERVE)
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
        if manual && before < 2_000 {
            d.line(&ui::dim(&format!("  nothing worth compacting yet (~{} tokens)", ui::human(before as u64))));
            return Ok(());
        }
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
            self.remember(kind, text, false);
        }
        self.memory.save();
        let tail = self.history.split_off(split);
        let mut block = format!("[Summary of the conversation so far]\n{summary}\n\n");
        if !working_set.is_empty() {
            block.push_str(&format!("{}\n{working_set}\n", context::WORKING_SET_HEADER));
        }
        // The verbatim user messages go last; later compactions read them back from here.
        block.push_str(&format!("{}\n{carried}", context::USER_MESSAGES_HEADER));
        self.archived.append(&mut self.history);
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

    /// The whole run for benchmark harnesses and reviewers: every message in
    /// order, including the ones compaction summarised away.
    pub fn trajectory(&self) -> Value {
        let t = &self.totals;
        let mut messages = self.archived.clone();
        messages.extend(self.history.iter().cloned());
        json!({
            "agent": "rusty",
            "version": env!("CARGO_PKG_VERSION"),
            "model": self.model,
            "memory_mode": self.memory_mode,
            "memory": self.advisor.as_ref().map(|h| &h.metrics),
            "execution_mode": self.execution_mode.name(),
            "tools_location": self.backend.summary(),
            "messages": messages,
            "archived_messages": self.archived.len(),
            "goal": self.goal,
            "verification": self.verification_records,
            "totals": {"requests": t.requests, "prompt_tokens": t.prompt, "completion_tokens": t.completion},
        })
    }

    pub fn write_trajectory(&self) -> Result<()> {
        let Some(path) = &self.trajectory_path else { return Ok(()) };
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        // Write beside it and rename, so a run killed mid-write keeps the last good copy.
        let tmp = path.with_extension("partial");
        rusty::privacy::write_json(&tmp, self.trajectory())?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Rewrites the trajectory at most every `TRAJECTORY_EVERY` during a run.
    /// Best effort: the final write on exit still reports errors.
    fn checkpoint_trajectory(&mut self) {
        if self.trajectory_path.is_none() || self.trajectory_saved.is_some_and(|t| t.elapsed() < TRAJECTORY_EVERY) {
            return;
        }
        let _ = self.write_trajectory();
        self.trajectory_saved = Some(Instant::now());
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
        let body = json!({"model": self.model, "history": self.history, "plan": self.plan, "goal": self.goal, "verification":self.verification_records});
        let _ = rusty::privacy::write_json(path, body);
        let changes = self.infra.change_record();
        if !changes.is_empty() {
            use std::io::Write;
            if let Ok(mut f) = rusty::privacy::private_file(&path.with_extension("changes.txt"), false) {
                let _ = f.write_all(infra::redact(&changes).0.as_bytes());
            }
        }
    }

    pub fn load_session(&mut self, path: &std::path::Path) -> Result<()> {
        let v: Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
        self.history = v["history"].as_array().cloned().unwrap_or_default();
        self.plan = serde_json::from_value(v["plan"].clone()).unwrap_or_default();
        self.goal = serde_json::from_value(v["goal"].clone()).unwrap_or(None);
        self.verification_records.clear();
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
            "execution_mode": self.execution_mode.name(),
            "mode_reason": if self.mode_fixed { "chosen by the user" } else { self.mode_reason.as_str() },
            "ms": took.as_millis() as u64,
            "prompt": reply.usage.map(|u| u.prompt),
            "completion": reply.usage.map(|u| u.completion),
            "tools": reply.tool_calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            "finish": reply.finish_reason,
        });
        use std::io::Write;
        if let Ok(mut f) = rusty::privacy::private_file(&path.with_extension("trace.jsonl"), true) {
            let _ = writeln!(f, "{line}");
        }
    }
}

fn job(v: &Value) -> (String, String) {
    (v["description"].as_str().unwrap_or("worker").to_string(), v["prompt"].as_str().unwrap_or("").to_string())
}

pub(crate) fn load_project_notes(cwd: &std::path::Path) -> String {
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

/// Destructive calls normally need a person every time. A disposable sandbox
/// run by a harness can opt out with `RUSTY_ALLOW_DESTRUCTIVE=1`, which only
/// counts in yolo mode with nobody at the terminal, so it can't weaken an
/// interactive session or the other permission modes.
fn unattended_destructive(mode: crate::permissions::Mode) -> bool {
    use std::io::IsTerminal;
    mode == crate::permissions::Mode::Yolo
        && std::env::var("RUSTY_ALLOW_DESTRUCTIVE").is_ok_and(|v| v.trim() == "1")
        && !std::io::stdin().is_terminal()
}

/// Asks for approval. Uses line editing so Ctrl-C cleanly means "no, stop".
/// Asks the person at the keyboard. A destructive call can only be approved
/// once, by typing `yes` in full, and never in a session nobody is watching.
fn ask_user(why: &str, destructive: bool) -> Answer {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        return Answer::No(Some(if destructive {
            format!(
                "refused: {why}. This can't be undone, so it needs a person to approve it and nobody is watching \
                 this session. Don't retry it or work around it; tell the user the exact command so they can run \
                 it themselves."
            )
        } else {
            "non-interactive session; the call needs approval".into()
        }));
    }
    if destructive {
        println!("  {} {}", ui::err("‼ can't be undone:"), why);
        let prompt = format!("  {} ", ui::dim("type yes to run it once · anything else is sent back as feedback ›"));
        let mut prompt = prompt;
        loop {
            let line = match rustyline::DefaultEditor::new().and_then(|mut rl| rl.readline(&prompt)) {
                Ok(l) => l,
                Err(rustyline::error::ReadlineError::Interrupted) => {
                    signal::trip();
                    return Answer::No(Some("the user pressed ctrl-c to stop".into()));
                }
                Err(_) => return Answer::No(None),
            };
            return match line.trim() {
                "yes" | "YES" => Answer::Yes,
                // A reflexive `y` is exactly what this prompt guards against.
                "y" | "Y" => {
                    prompt = format!("  {} ", ui::dim("type the whole word yes to run it, or n ›"));
                    continue;
                }
                "" | "n" | "N" | "no" => Answer::No(None),
                other => Answer::No(Some(other.to_string())),
            };
        }
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

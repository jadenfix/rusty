mod agent;
mod config;
mod context;
mod display;
mod execution;
mod llm;
mod markdown;
mod memory;
mod permissions;
mod signal;
mod skills;
mod tips;
mod tools;
mod ui;

use anyhow::{anyhow, Result};
use clap::Parser;
use rustyline::error::ReadlineError;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use agent::{Agent, GoalStatus};
use config::{AgentsMode, Settings};
use execution::ExecutionMode;
use llm::Client;
use memory::Memory;
use permissions::{Mode, Policy, Verdict};

/// A coding agent for your terminal.
#[derive(Parser)]
#[command(name = "rusty", version)]
struct Cli {
    /// Run this prompt once and exit. Omit for interactive mode.
    prompt: Vec<String>,

    /// Any model id your endpoint serves (see --list-models)
    #[arg(short, long, env = "RUSTY_MODEL")]
    model: Option<String>,

    /// Execution mode: careful, standard or vibe (independent of permissions)
    #[arg(long, env = "RUSTY_MODE")]
    mode: Option<String>,

    /// Permission mode: read-only, ask, auto or yolo
    #[arg(short, long, env = "RUSTY_PERMISSIONS", default_value = "auto")]
    permissions: String,

    /// Shortcut for --permissions yolo
    #[arg(long)]
    yolo: bool,

    /// Delegation: off, sub, swarm or auto (overrides saved settings)
    #[arg(short, long, env = "RUSTY_AGENTS")]
    agents: Option<String>,

    /// View: default, verbose or adhd (overrides saved settings)
    #[arg(long)]
    view: Option<String>,

    /// Resume the most recent session in this directory
    #[arg(short = 'c', long = "continue")]
    resume: bool,

    /// Work on this goal until it's done, then exit
    #[arg(long)]
    goal: Option<String>,

    /// Print a JSON line with token and timing stats to stderr on exit
    #[arg(long)]
    stats: bool,

    /// Write the conversation (OpenAI message format) and totals to this JSON file on exit
    #[arg(long, value_name = "PATH")]
    trajectory: Option<PathBuf>,

    /// List the models your endpoint serves and exit
    #[arg(long)]
    list_models: bool,

    /// Project directory (defaults to the current directory)
    #[arg(short = 'C', long)]
    dir: Option<PathBuf>,
}

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("{} {e:#}", ui::err("error:"));
            std::process::exit(1);
        }
    }
}

fn run() -> Result<i32> {
    config::load_env();
    let cli = Cli::parse();
    if let Some(d) = &cli.dir {
        std::env::set_current_dir(d).map_err(|e| anyhow!("cannot enter {}: {e}", d.display()))?;
    }
    let cwd = std::env::current_dir()?.canonicalize()?;

    let mut settings = Settings::load();
    ui::set_theme(&settings.theme);
    ui::set_font(&settings.font);
    display::set_view(&settings.view);
    if let Some(v) = &cli.view {
        if !display::set_view(v) {
            return Err(anyhow!("unknown view `{v}` (default, verbose, adhd)"));
        }
    }
    if let Some(a) = &cli.agents {
        settings.agents.mode = AgentsMode::parse(a).ok_or_else(|| anyhow!("unknown agents mode `{a}`"))?;
    }

    let execution_mode = match &cli.mode {
        Some(mode) => ExecutionMode::parse(mode)
            .ok_or_else(|| anyhow!("unknown execution mode `{mode}` (careful, standard, vibe)"))?,
        None => settings.execution_mode,
    };
    let model =
        cli.model.clone().or_else(|| execution_mode.model()).unwrap_or_else(|| config::DEFAULT_MODEL.to_string());
    let client = Arc::new(Client::new(config::base_url(), config::api_keys())?);
    if cli.list_models {
        for m in client.list_models()? {
            println!("{m}");
        }
        return Ok(0);
    }

    signal::install();
    signal::save_terminal();
    let mode = if cli.yolo {
        Mode::Yolo
    } else {
        Mode::parse(&cli.permissions).ok_or_else(|| anyhow!("unknown permission mode `{}`", cli.permissions))?
    };
    let pdir = project_dir(&cwd);
    let global = config::config_dir().unwrap_or_default();
    let policy = Policy::load(pdir.join("permissions.json"), mode);
    let memory = Memory::load(pdir.join("memory.jsonl"), global.join("memory.jsonl"));

    let mut agent = Agent::new(client.clone(), model, cwd.clone(), policy, memory);
    agent.agents = settings.agents.clone();
    agent.tips = settings.tips;
    agent.model_override = cli.model.clone();
    agent.delegation_override =
        cli.agents.is_some() || settings.delegation_override || settings.agents.mode != AgentsMode::Off;
    agent.set_execution_mode(execution_mode);

    let sessions = pdir.join("sessions");
    if cli.resume {
        match latest_session(&sessions) {
            Some(p) => {
                agent.load_session(&p)?;
                println!("{}", ui::dim(&format!("resumed {} messages", agent.history.len())));
            }
            None => println!("{}", ui::dim("no previous session here; starting fresh")),
        }
    }
    if agent.session_path.is_none() {
        agent.session_path = Some(new_session_path(&sessions));
    }

    // Headless modes.
    let started = Instant::now();
    let headless = cli.goal.is_some() || !cli.prompt.is_empty();
    if headless {
        let result = match &cli.goal {
            Some(goal) => agent.run_goal(Some(goal)),
            None => {
                let input = cli.prompt.join(" ");
                let input = expand_skill(&cwd, &input).unwrap_or(input);
                agent.run_turn(&input).map(|_| ())
            }
        };
        agent.save_session();
        if cli.stats {
            print_stats(&agent, started);
        }
        if let Some(path) = &cli.trajectory {
            write_trajectory(&agent, path)?;
        }
        result?;
        return Ok(if signal::interrupted() { 130 } else { 0 });
    }

    repl(&mut agent, &client, &cwd, &mut settings)?;
    if cli.stats {
        print_stats(&agent, started);
    }
    if let Some(path) = &cli.trajectory {
        write_trajectory(&agent, path)?;
    }
    Ok(0)
}

/// The whole conversation for benchmark harnesses and reviewers.
fn write_trajectory(agent: &Agent, path: &Path) -> Result<()> {
    let t = &agent.totals;
    let body = serde_json::json!({
        "agent": "rusty",
        "version": env!("CARGO_PKG_VERSION"),
        "model": agent.model,
        "execution_mode": agent.execution_mode.name(),
        "messages": agent.history,
        "totals": {"requests": t.requests, "prompt_tokens": t.prompt, "completion_tokens": t.completion},
    });
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(&body)?)?;
    Ok(())
}

/// One JSON line on stderr for scripts and evals.
fn print_stats(agent: &Agent, started: Instant) {
    let t = &agent.totals;
    let goal = agent.goal.as_ref().map(|g| match &g.status {
        GoalStatus::Done(_) => "done",
        GoalStatus::Blocked(_) => "blocked",
        GoalStatus::Active => "open",
    });
    eprintln!(
        "{}",
        serde_json::json!({
            "execution_mode": agent.execution_mode.name(),
            "requests": t.requests, "prompt": t.prompt, "completion": t.completion,
            "secs": started.elapsed().as_secs_f32(), "interrupted": signal::interrupted(), "goal": goal,
        })
    );
}

/// `/name args` → the skill's prompt, if `name` is a skill.
fn expand_skill(cwd: &Path, input: &str) -> Option<String> {
    let rest = input.strip_prefix('/')?;
    let (name, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    skills::find(cwd, name).map(|s| skills::render(&s, args))
}

fn repl(agent: &mut Agent, client: &Arc<Client>, cwd: &Path, settings: &mut Settings) -> Result<()> {
    ui::banner(&ui::BannerInfo {
        model: &agent.model,
        cwd: &tilde(cwd),
        exec: agent.execution_mode.name(),
        perms: agent.policy.mode.name(),
        agents: agent.agents.mode.name(),
        view: display::view_name(),
        keys: client.key_count(),
    });
    let mut rl = rustyline::DefaultEditor::new()?;
    let history_file = config::config_dir().map(|d| d.join("history"));
    if let Some(h) = &history_file {
        let _ = rl.load_history(h);
    }
    let prompt = format!("{} ", ui::primary("›"));
    let mut armed_quit = false;

    loop {
        // Something typed while the last turn ran goes next, as if just entered.
        if let Some(queued) = signal::take_queued() {
            println!("{} {}", ui::primary("›"), queued);
            println!("{}", ui::dim("  (typed while rusty was working)"));
            signal::restore_terminal();
            signal::reset();
            let _ = rl.add_history_entry(&queued);
            if let Err(e) = agent.run_turn(&queued) {
                println!("{} {e:#}\n", ui::err("error:"));
            }
            agent.save_session();
            continue;
        }
        let mut input = match rl.readline(&prompt) {
            Ok(l) => {
                armed_quit = false;
                l
            }
            Err(ReadlineError::Interrupted) => {
                if armed_quit {
                    break;
                }
                armed_quit = true;
                println!("{}", ui::dim("  (ctrl-c again to quit)"));
                continue;
            }
            Err(ReadlineError::Eof) => break,
            Err(e) => return Err(e.into()),
        };
        // A trailing backslash continues the message on the next line.
        while input.ends_with('\\') {
            input.pop();
            input.push('\n');
            match rl.readline("  ") {
                Ok(l) => input.push_str(&l),
                Err(_) => break,
            }
        }
        let input = input.trim().to_string();
        if input.is_empty() {
            continue;
        }
        let _ = rl.add_history_entry(&input);
        signal::restore_terminal();
        signal::reset();

        let result = if input.starts_with('/') {
            match command(agent, client, cwd, settings, &input) {
                Ok(true) => break,
                Ok(false) => Ok(()),
                Err(e) => Err(e),
            }
        } else {
            agent.run_turn(&input).map(|_| ())
        };
        if let Err(e) = result {
            println!("{} {e:#}\n", ui::err("error:"));
        }
        agent.save_session();
    }
    if let Some(h) = &history_file {
        if let Some(dir) = h.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = rl.save_history(h);
    }
    agent.save_session();
    println!("{}", ui::dim("  ◌ link closed. go ship something."));
    Ok(())
}

/// Handles a slash command. Returns true to quit.
fn command(agent: &mut Agent, client: &Arc<Client>, cwd: &Path, settings: &mut Settings, input: &str) -> Result<bool> {
    let (cmd, rest) = match input.split_once(char::is_whitespace) {
        Some((c, r)) => (c, r.trim()),
        None => (input, ""),
    };
    match cmd {
        "/exit" | "/quit" | "/q" => return Ok(true),
        "/help" | "/?" => print_help(cwd),
        "/clear" | "/new" => {
            agent.save_session();
            agent.clear();
            if let Some(p) = &agent.session_path {
                agent.session_path = p.parent().map(new_session_path);
            }
            println!("{}", ui::dim("  fresh context. memories kept."));
        }
        "/model" => {
            if !rest.is_empty() {
                agent.model = rest.to_string();
                agent.model_override = Some(rest.to_string());
            }
            println!("  model {}", ui::info(&agent.model));
        }
        "/mode" => {
            if !rest.is_empty() {
                let mode = ExecutionMode::parse(rest)
                    .ok_or_else(|| anyhow!("unknown execution mode `{rest}` (careful, standard, vibe)"))?;
                agent.set_execution_mode(mode);
                settings.execution_mode = mode;
                settings.save();
            }
            let m = agent.execution_mode;
            let what = match m {
                ExecutionMode::Careful => "thinks longer, takes two extra looks before calling work done",
                ExecutionMode::Standard => "normal effort, checks what it changed",
                ExecutionMode::Vibe => "quick passes, small checks, may send out a few read-only workers",
            };
            println!("  execution {} {}", ui::bold(m.name()), ui::dim(&format!("· {what}")));
            println!(
                "  {}",
                ui::dim(&format!(
                    "model {} · rechecks {} · workers {} (≤{})",
                    agent.model,
                    m.review_passes(),
                    agent.agents.mode.name(),
                    agent.swarm_cap()
                ))
            );
        }
        "/models" => {
            for m in client.list_models()?.into_iter().filter(|m| m.contains(rest)) {
                println!("  {m}");
            }
        }
        "/compact" => agent.compact(Some(rest).filter(|r| !r.is_empty()))?,
        "/context" => println!("{}", agent.context_report()),
        "/tokens" | "/usage" | "/cost" => print_tokens(agent),
        "/view" | "/adhd" | "/verbose" => {
            let target = match cmd {
                "/adhd" => "adhd",
                "/verbose" => "verbose",
                _ => rest,
            };
            if target.is_empty() {
                println!("  view {} {}", ui::bold(display::view_name()), ui::dim("(default · verbose · adhd)"));
            } else if display::set_view(target) {
                settings.view = display::view_name().to_string();
                settings.save();
                println!("  view {}", ui::bold(display::view_name()));
            } else {
                println!("  unknown view. default · verbose · adhd");
            }
        }
        "/theme" => {
            if rest.is_empty() || !ui::set_theme(rest) {
                let names: Vec<String> = ui::THEMES
                    .iter()
                    .map(|t| if t.name == ui::theme().name { ui::bold(t.name) } else { ui::dim(t.name) })
                    .collect();
                println!("  theme {}", names.join(" · "));
            } else {
                settings.theme = rest.to_string();
                settings.save();
                println!("  {} {}", ui::gradient("▓▒░ theme"), ui::bold(rest));
            }
        }
        "/font" => {
            if rest.is_empty() || !ui::set_font(rest) {
                println!("  font {} {}", ui::bold(ui::font()), ui::dim(&format!("({})", ui::FONTS.join(" · "))));
            } else {
                settings.font = rest.to_string();
                settings.save();
                ui::banner(&ui::BannerInfo {
                    model: &agent.model,
                    cwd: &tilde(cwd),
                    exec: agent.execution_mode.name(),
                    perms: agent.policy.mode.name(),
                    agents: agent.agents.mode.name(),
                    view: display::view_name(),
                    keys: client.key_count(),
                });
            }
        }
        "/agents" | "/subagents" if rest == "default" => {
            // Hand delegation back to the execution mode.
            agent.delegation_override = false;
            settings.delegation_override = false;
            agent.set_execution_mode(agent.execution_mode);
            settings.agents.mode = AgentsMode::Off;
            settings.save();
            agents_cmd(agent, "");
        }
        "/agents" | "/subagents" => {
            agents_cmd(agent, rest);
            if !rest.is_empty() {
                agent.delegation_override = true;
                settings.delegation_override = true;
                settings.agents = agent.agents.clone();
                settings.save();
            }
        }
        "/swarm" => {
            swarm_cmd(agent, rest);
            if !rest.is_empty() {
                agent.delegation_override = true;
                settings.delegation_override = true;
                settings.agents = agent.agents.clone();
                settings.save();
            }
        }
        "/tips" => {
            agent.tips = match rest {
                "off" => false,
                "on" => true,
                _ => !agent.tips,
            };
            settings.tips = agent.tips;
            settings.save();
            println!("  tips {}", on_off(agent.tips));
        }
        "/settings" => print_settings(agent),
        "/permissions" | "/perms" => permissions_cmd(agent, rest),
        "/yolo" => {
            agent.policy.mode = Mode::Yolo;
            println!("  permissions {}", ui::warn("yolo · everything runs except deny rules and destructive commands"));
        }
        "/plan" => {
            if agent.plan.is_empty() {
                println!("{}", ui::dim("  no plan yet"));
            } else {
                let items = serde_json::to_value(&agent.plan).unwrap_or_default();
                println!("{}", display::render_plan(&items));
            }
        }
        "/goal" => match rest {
            "" | "status" => match &agent.goal {
                Some(g) => {
                    let status = match &g.status {
                        GoalStatus::Active => "active".to_string(),
                        GoalStatus::Done(e) => format!("done: {e}"),
                        GoalStatus::Blocked(e) => format!("blocked: {e}"),
                    };
                    println!(
                        "  goal {}\n  {}",
                        ui::bold(&g.objective),
                        ui::dim(&format!("{status} · {} turns", g.turns))
                    );
                }
                None => println!("{}", ui::dim("  no goal. /goal <objective> to start one")),
            },
            "clear" | "off" => {
                agent.goal = None;
                println!("{}", ui::dim("  goal cleared"));
            }
            "resume" | "continue" => agent.run_goal(None)?,
            objective => agent.run_goal(Some(objective))?,
        },
        "/loop" => match parse_loop(rest) {
            Some((interval, max, prompt)) => agent.run_loop(&prompt, interval, max)?,
            None => println!("  usage: /loop [30s|5m|1h] [x10] <prompt>   (no interval: rusty picks the pace)"),
        },
        "/memory" | "/mem" => memory_cmd(agent, rest),
        "/remember" => {
            if rest.is_empty() {
                println!("  usage: /remember [preference|fact|decision|gotcha:] <text>");
            } else {
                let (kind, text) = match rest.split_once(':') {
                    Some((k, t)) if memory::KINDS.contains(&k.trim()) => (k.trim(), t.trim()),
                    _ => ("fact", rest),
                };
                println!("  {}", agent.memory.add(kind, text, kind == "preference"));
                agent.memory.save();
            }
        }
        "/forget" => {
            let ok = agent.memory.forget(rest);
            agent.memory.save();
            println!("  {}", if ok { format!("forgot {rest}") } else { format!("no memory {rest}") });
        }
        "/skills" => {
            for s in skills::all(cwd) {
                println!(
                    "  {} {} {}",
                    ui::info(&format!("{:<14}", format!("/{}", s.name))),
                    s.description,
                    ui::dim(&format!("· {}", s.source))
                );
            }
            println!("{}", ui::dim("  add your own as .md files in ~/.config/rusty/skills or .rusty/skills"));
        }
        other => match expand_skill(cwd, input) {
            Some(prompt) => {
                agent.run_turn(&prompt)?;
            }
            None => println!("  unknown command {other}. /help lists them"),
        },
    }
    Ok(false)
}

fn on_off(b: bool) -> String {
    if b {
        ui::ok("on")
    } else {
        ui::dim("off")
    }
}

fn agents_cmd(agent: &mut Agent, rest: &str) {
    let (sub, arg) = rest.split_once(char::is_whitespace).map(|(s, a)| (s, a.trim())).unwrap_or((rest, ""));
    match sub {
        "" => {}
        "model" => {
            agent.agents.sub_model = if arg.is_empty() || arg == "default" { None } else { Some(arg.to_string()) };
        }
        m => match AgentsMode::parse(m) {
            Some(mode) => agent.agents.mode = mode,
            None => {
                println!("  /agents off · sub · swarm · auto · default · model <id>");
                return;
            }
        },
    }
    let a = &agent.agents;
    let desc = match a.mode {
        AgentsMode::Off => "no delegation",
        AgentsMode::Sub => "one read-only subagent at a time",
        AgentsMode::Swarm => "parallel read-only workers",
        AgentsMode::Auto => "rusty picks: itself, a subagent, or a swarm",
    };
    println!("  agents {} {}", ui::bold(a.mode.name()), ui::dim(&format!("· {desc}")));
    println!("  {}", ui::dim(&format!("subagent model {}", a.sub_model.as_deref().unwrap_or("(same as main)"))));
}

fn swarm_cmd(agent: &mut Agent, rest: &str) {
    let (sub, arg) = rest.split_once(char::is_whitespace).map(|(s, a)| (s, a.trim())).unwrap_or((rest, ""));
    let a = &mut agent.agents;
    match sub {
        "" => {}
        "on" => a.mode = AgentsMode::Swarm,
        "off" => a.mode = AgentsMode::Off,
        "size" | "max" => match arg.parse::<usize>() {
            Ok(n) if (1..=64).contains(&n) => a.swarm_max = n,
            _ => println!("  size must be 1-64"),
        },
        "models" => {
            a.swarm_models = arg
                .split([',', ' '])
                .map(str::trim)
                .filter(|s| !s.is_empty() && *s != "default")
                .map(String::from)
                .collect()
        }
        "spread" => match arg.parse::<f32>() {
            Ok(x) if (0.0..=1.0).contains(&x) => a.spread = x,
            _ => println!("  spread must be between 0 and 1"),
        },
        _ => {
            println!("  /swarm on · off · size <n> · models <a,b,…> · spread <0-1>");
            return;
        }
    }
    let models = if a.swarm_models.is_empty() { "(subagent model)".to_string() } else { a.swarm_models.join(", ") };
    println!(
        "  swarm {} {}",
        if a.mode == AgentsMode::Swarm || a.mode == AgentsMode::Auto { ui::ok("armed") } else { ui::dim("off") },
        ui::dim(&format!("· mode {}", a.mode.name()))
    );
    println!(
        "  {}",
        ui::dim(&format!("size ≤{} · models {models} · temperature 0.2→{:.1}", a.swarm_max, 0.2 + a.spread))
    );
}

fn print_settings(agent: &Agent) {
    let a = &agent.agents;
    let rows = [
        ("model", agent.model.clone()),
        ("execution", agent.execution_mode.name().to_string()),
        ("rechecks", agent.execution_mode.review_passes().to_string()),
        ("permissions", agent.policy.mode.name().to_string()),
        ("view", display::view_name().to_string()),
        ("theme", ui::theme().name.to_string()),
        ("font", ui::font().to_string()),
        ("agents", a.mode.name().to_string()),
        ("subagent model", a.sub_model.clone().unwrap_or_else(|| "(same as main)".into())),
        ("swarm size", a.swarm_max.to_string()),
        ("swarm models", if a.swarm_models.is_empty() { "(subagent model)".into() } else { a.swarm_models.join(", ") }),
        ("swarm spread", format!("{:.1}", a.spread)),
        ("tips", if agent.tips { "on".into() } else { "off".into() }),
        ("context window", context::window().to_string()),
    ];
    for (k, v) in rows {
        println!("  {} {}", ui::dim(&format!("{k:<15}")), v);
    }
}

fn print_tokens(agent: &Agent) {
    let t = &agent.totals;
    println!(
        "  {} requests · ↑{} prompt · ↓{} completion · context {}%",
        t.requests,
        ui::human(t.prompt),
        ui::human(t.completion),
        agent.context_pct()
    );
    if let Some(avg) = t.prompt.checked_div(t.requests) {
        println!("  {}", ui::dim(&format!("avg prompt {} per request", ui::human(avg))));
    }
    for (m, (r, p, c)) in &t.by_model {
        println!(
            "  {} {}",
            ui::info(&format!("{m:<44}")),
            ui::dim(&format!("{r} req · ↑{} ↓{}", ui::human(*p), ui::human(*c)))
        );
    }
    let mut tools: Vec<_> = t.by_tool.iter().collect();
    tools.sort_by(|a, b| b.1.cmp(a.1));
    if !tools.is_empty() {
        let parts: Vec<String> = tools.iter().take(6).map(|(k, v)| format!("{k} ~{}", ui::human(**v))).collect();
        println!("  {} {}", ui::dim("tool output →"), ui::dim(&parts.join(" · ")));
    }
}

fn permissions_cmd(agent: &mut Agent, rest: &str) {
    let (sub, arg) = rest.split_once(char::is_whitespace).map(|(s, a)| (s, a.trim())).unwrap_or((rest, ""));
    let p = &mut agent.policy;
    match sub {
        "" => {
            println!("  mode {}", ui::bold(p.mode.name()));
            println!("    read-only  reads, searches and read-only commands (cloud reads too)");
            println!("    ask        asks before every edit or command that changes something");
            println!("    auto       project work, everyday git and cloud reads run; the rest asks");
            println!("    yolo       everything runs except deny rules and destructive commands");
            println!(
                "  {}",
                ui::dim("destructive (force push, cloud or database deletes, rm -rf outside) asks every time")
            );
            println!("  allow {}", if p.allow.is_empty() { "-".into() } else { p.allow.join(", ") });
            println!("  deny  {}", if p.deny.is_empty() { "-".into() } else { p.deny.join(", ") });
            println!(
                "{}",
                ui::dim("  /permissions <mode> · allow <rule> · deny <rule> · remove <rule> · reset · check <command>")
            );
        }
        "allow" | "deny" if !arg.is_empty() => {
            if sub == "allow" {
                p.allow.push(arg.into())
            } else {
                p.deny.push(arg.into())
            }
            p.save();
            println!("  {sub} `{arg}`");
        }
        "remove" if !arg.is_empty() => {
            p.allow.retain(|r| r != arg);
            p.deny.retain(|r| r != arg);
            p.save();
            println!("  removed `{arg}`");
        }
        "reset" => {
            p.allow.clear();
            p.deny.clear();
            p.save();
            println!("  rules cleared");
        }
        "check" if !arg.is_empty() => {
            let args = serde_json::json!({"command": arg});
            let class = permissions::classify("bash", &args, &agent.cwd);
            let verdict = match p.decide("bash", &args, &agent.cwd) {
                Verdict::Allow => ui::ok("✓ runs"),
                Verdict::Ask(_) => ui::warn("? asks first"),
                Verdict::Confirm(_) => ui::err("‼ needs you every time"),
                Verdict::Deny(why) => ui::err(&format!("✗ refused · {why}")),
            };
            println!("  {verdict} {}", ui::dim(&format!("· {} · {} mode", class.describe(), p.mode.name())));
        }
        m => match Mode::parse(m) {
            Some(mode) => {
                p.mode = mode;
                println!("  permissions {}", ui::bold(mode.name()));
            }
            None => println!("  unknown option `{m}`. /permissions for help"),
        },
    }
}

fn memory_cmd(agent: &mut Agent, rest: &str) {
    let (sub, arg) = rest.split_once(char::is_whitespace).map(|(s, a)| (s, a.trim())).unwrap_or((rest, ""));
    match sub {
        "" | "list" => {
            if agent.memory.entries.is_empty() {
                println!("{}", ui::dim("  no memories yet. rusty saves them as it learns, or use /remember"));
            }
            for e in &agent.memory.entries {
                let scope = if e.global { "global " } else { "project" };
                println!("  {} {} {:<10} {}", ui::dim(&e.id), ui::dim(scope), e.kind, e.text);
            }
        }
        "search" => {
            for (score, e) in agent.memory.search(arg, 10) {
                println!("  {} {:.2} {:<10} {}", ui::dim(&e.id), score, e.kind, e.text);
            }
        }
        "forget" => {
            let ok = agent.memory.forget(arg);
            println!("  {}", if ok { "forgotten" } else { "no such memory" });
        }
        "clear" => {
            agent.memory.clear_project();
            println!("  project memories cleared (global ones kept)");
        }
        _ => println!("  /memory [list | search <query> | forget <id> | clear]"),
    }
    agent.memory.save();
}

/// Parses `[30s|5m|1h] [x10] <prompt>`.
fn parse_loop(rest: &str) -> Option<(Option<u64>, Option<u32>, String)> {
    let mut words: Vec<&str> = rest.split_whitespace().collect();
    let mut interval = None;
    let mut max = None;
    if let Some(secs) = words.first().and_then(|w| parse_duration(w)) {
        interval = Some(secs);
        words.remove(0);
    }
    if let Some(n) = words.first().and_then(|w| w.strip_prefix('x')).and_then(|n| n.parse().ok()) {
        max = Some(n);
        words.remove(0);
    }
    let prompt = words.join(" ");
    (!prompt.is_empty()).then_some((interval, max, prompt))
}

fn parse_duration(s: &str) -> Option<u64> {
    let (num, unit) = s.split_at(s.len().checked_sub(1)?);
    let n: u64 = num.parse().ok()?;
    match unit {
        "s" => Some(n),
        "m" => Some(n * 60),
        "h" => Some(n * 3600),
        _ => None,
    }
}

fn print_help(cwd: &Path) {
    let sections: [(&str, &[(&str, &str)]); 4] = [
        (
            "work",
            &[
                ("/goal <objective>", "keep going until it's done and verified · status · resume · clear"),
                ("/loop [5m] [x10] <prompt>", "repeat on an interval, or let rusty pick the pace"),
                ("/agents off|sub|swarm|auto", "delegation · default follows /mode · model <id> for subagents"),
                ("/swarm size|models|spread", "tune parallel workers"),
                ("/mode careful|standard|vibe", "how hard rusty thinks and checks; permissions stay separate"),
                ("/plan", "the current plan"),
            ],
        ),
        (
            "memory & context",
            &[
                ("/memory", "list · search · forget · clear"),
                ("/remember <text>", "save one (prefix preference:, gotcha:, decision:)"),
                ("/context", "where the window is going"),
                ("/compact [focus]", "summarise now; say what matters most to keep it in detail"),
                ("/tokens", "spend by model and by tool"),
                ("/clear", "fresh conversation, memories kept"),
            ],
        ),
        (
            "setup",
            &[
                ("/permissions [mode]", "read-only · ask · auto · yolo · allow · deny · check"),
                ("/model [id] · /models", "switch or list models"),
                ("/view default|verbose|adhd", "how much you see"),
                ("/theme · /font", "themes rust neon matrix amber ice mono · fonts rust block thin classic"),
                ("/tips · /settings · /exit", ""),
            ],
        ),
        ("skills", &[]),
    ];
    for (title, rows) in sections {
        println!("  {}", ui::accent(title));
        if title == "skills" {
            for s in skills::all(cwd) {
                println!(
                    "    {} {}",
                    ui::info(&format!("{:<28}", format!("/{} [args]", s.name))),
                    ui::dim(&s.description)
                );
            }
            continue;
        }
        for (c, d) in rows.iter() {
            println!("    {} {}", ui::info(&format!("{c:<28}")), ui::dim(d));
        }
    }
    println!("  {}", ui::dim("end a line with \\ for more lines · ctrl-c stops a turn · ctrl-c twice quits"));
}

/// `/Users/me/code/x` → `~/code/x`.
fn tilde(p: &Path) -> String {
    let s = p.display().to_string();
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() && s.starts_with(&h) => format!("~{}", &s[h.len()..]),
        _ => s,
    }
}

fn project_dir(cwd: &Path) -> PathBuf {
    let name = cwd.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "root".into());
    let id = format!("{name}-{:08x}", memory::fnv(&cwd.display().to_string()) as u32);
    config::config_dir().unwrap_or_else(|| PathBuf::from(".rusty")).join("projects").join(id)
}

fn new_session_path(dir: &Path) -> PathBuf {
    dir.join(format!("{}.json", memory::now()))
}

fn latest_session(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json") && !p.to_string_lossy().ends_with(".trace.json"))
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loop_args() {
        assert_eq!(parse_loop("5m x3 check ci"), Some((Some(300), Some(3), "check ci".into())));
        assert_eq!(parse_loop("check ci"), Some((None, None, "check ci".into())));
        assert_eq!(parse_loop("30s"), None);
    }
}

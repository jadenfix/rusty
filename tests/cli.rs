//! End-to-end tests that drive the real binary.
//!
//! The default set runs offline: slash commands, settings, permissions,
//! memory, skills and argument handling, all with an isolated config dir.
//! Tests marked `#[ignore]` talk to the real model and need NVIDIA_API_KEY:
//!
//!     cargo test -- --ignored

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_rusty");

struct Sandbox {
    home: PathBuf,
    project: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("rusty-e2e-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let project = root.join("project");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        Self { home, project }
    }

    fn cmd(&self) -> Command {
        let mut c = Command::new(BIN);
        c.current_dir(&self.project)
            .env("RUSTY_HOME", &self.home)
            .env("RUSTY_NO_DOTENV", "1")
            .env("NVIDIA_API_KEY", "offline-test-key")
            .env("NO_COLOR", "1")
            .env_remove("RUSTY_MODEL")
            .env_remove("RUSTY_MODE")
            .env_remove("RUSTY_CAREFUL_MODEL")
            .env_remove("RUSTY_STANDARD_MODEL")
            .env_remove("RUSTY_VIBE_MODEL")
            .env_remove("RUSTY_AGENTS")
            .env_remove("RUSTY_PERMISSIONS");
        c
    }

    /// Runs the REPL with `lines` piped to stdin.
    fn repl(&self, lines: &[&str]) -> String {
        let mut child = self.cmd().stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        for l in lines {
            writeln!(stdin, "{l}").unwrap();
        }
        drop(stdin);
        let out = wait(child, Duration::from_secs(20));
        assert!(out.status.success(), "repl failed: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).to_string()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        if let Some(root) = self.home.parent() {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

fn wait(mut child: std::process::Child, limit: Duration) -> Output {
    let start = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > limit {
            let _ = child.kill();
            panic!("rusty did not exit within {limit:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    child.wait_with_output().unwrap()
}

#[test]
fn version_and_help() {
    let s = Sandbox::new("version");
    let out = s.cmd().arg("--version").output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("rusty "));
    let out = s.cmd().arg("--help").output().unwrap();
    let help = String::from_utf8_lossy(&out.stdout);
    for flag in ["--model", "--permissions", "--agents", "--view", "--goal", "--continue", "--stats"] {
        assert!(help.contains(flag), "--help is missing {flag}");
    }
}

#[test]
fn missing_key_is_a_clear_error() {
    let s = Sandbox::new("nokey");
    let mut c = s.cmd();
    c.env_remove("NVIDIA_API_KEY").env_remove("RUSTY_API_KEY");
    for i in 2..=9 {
        c.env_remove(format!("NVIDIA_API_KEY_{i}"));
    }
    let out = c.arg("hello").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no API key found"));
}

#[test]
fn bad_flags_fail_fast() {
    let s = Sandbox::new("flags");
    let out = s.cmd().args(["--view", "sideways", "hi"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown view"));
    let out = s.cmd().args(["--permissions", "maybe", "hi"]).output().unwrap();
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown permission mode"));
}

#[test]
fn banner_and_help_list_every_command() {
    let s = Sandbox::new("help");
    let out = s.repl(&["/help"]);
    assert!(out.contains("model") && out.contains("nemotron"), "banner missing model");
    for cmd in
        ["/goal", "/loop", "/agents", "/swarm", "/permissions", "/memory", "/context", "/view", "/theme", "/review"]
    {
        assert!(out.contains(cmd), "/help is missing {cmd}");
    }
    assert!(out.contains("link closed"), "no goodbye on EOF");
}

#[test]
fn settings_persist_across_sessions() {
    let s = Sandbox::new("settings");
    s.repl(&["/theme amber", "/view adhd", "/agents swarm", "/swarm size 16", "/swarm spread 0.8", "/tips off"]);
    let out = s.repl(&["/settings"]);
    for want in ["amber", "adhd", "swarm", "16", "0.8"] {
        assert!(out.contains(want), "setting {want} did not persist:\n{out}");
    }
    assert!(out.contains("tips") && out.contains("off"));
}

#[test]
fn swarm_validates_input() {
    let s = Sandbox::new("swarm");
    let out = s.repl(&["/swarm size 500", "/swarm spread 3", "/swarm models a/x, b/y", "/swarm"]);
    assert!(out.contains("size must be 1-64"));
    assert!(out.contains("spread must be between 0 and 1"));
    assert!(out.contains("a/x, b/y"));
}

#[test]
fn permission_classifier_is_inspectable() {
    let s = Sandbox::new("perms");
    let out = s.repl(&[
        "/permissions check git push --force origin main",
        "/permissions check cargo test 2>&1 | tail",
        "/permissions allow git push",
        "/permissions check git push origin main",
        "/permissions check aws s3 rm s3://prod --recursive",
        "/permissions yolo",
        "/permissions check terraform destroy",
        "/permissions read-only",
        "/permissions check touch x",
        "/permissions check kubectl get pods",
    ]);
    let lines: Vec<&str> = out.lines().map(str::trim_start).filter(|l| l.starts_with(['✓', '?', '‼', '✗'])).collect();
    assert!(lines[0].contains("needs you every time") && lines[0].contains("destructive"), "{out}");
    assert!(lines[1].contains("✓ runs") && lines[1].contains("changes the project"), "{out}");
    assert!(lines[2].contains("✓ runs"), "an allow rule lets a push to main through: {out}");
    assert!(lines[3].contains("needs you every time"), "{out}");
    assert!(lines[4].contains("needs you every time") && lines[4].contains("yolo"), "yolo can't skip it: {out}");
    assert!(lines[5].contains("refused"), "{out}");
    assert!(lines[6].contains("✓ runs") && lines[6].contains("read-only"), "{out}");
    // Allow rules are saved per project.
    let out = s.repl(&["/permissions"]);
    assert!(out.contains("allow git push"));
}

#[test]
fn memory_round_trip() {
    let s = Sandbox::new("memory");
    s.repl(&["/remember gotcha: the schema file is generated, never edit it", "/remember preference: tabs not spaces"]);
    let out = s.repl(&["/memory", "/memory search schema"]);
    assert!(out.contains("gotcha") && out.contains("schema file is generated"));
    assert!(out.contains("global") && out.contains("tabs not spaces"));
    let id =
        out.lines().find(|l| l.contains("schema file")).and_then(|l| l.split_whitespace().next()).unwrap().to_string();
    let out = s.repl(&[&format!("/forget {id}"), "/memory"]);
    assert!(out.contains(&format!("forgot {id}")));
    assert!(!out.lines().any(|l| l.contains("schema file is generated") && l.contains("project")));
}

#[test]
fn project_skills_override_builtins() {
    let s = Sandbox::new("skills");
    let dir = s.project.join(".rusty").join("skills");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("review.md"), "---\ndescription: our house review\n---\nReview {{args}}").unwrap();
    std::fs::write(dir.join("ship.md"), "---\ndescription: cut a release\n---\nShip {{args}}").unwrap();
    let out = s.repl(&["/skills"]);
    assert!(out.contains("our house review") && out.contains("project"));
    assert!(out.contains("/ship") && out.contains("cut a release"));
    assert!(out.contains("/explore"), "built-ins still listed");
}

#[test]
fn unknown_command_and_goal_status() {
    let s = Sandbox::new("unknown");
    let out = s.repl(&["/frobnicate", "/goal", "/loop", "/plan", "/context"]);
    assert!(out.contains("unknown command /frobnicate"));
    assert!(out.contains("no goal"));
    assert!(out.contains("usage: /loop"));
    assert!(out.contains("no plan yet"));
    assert!(out.contains("window"));
}

// ------------------------------------------------------------------ live

fn live(name: &str) -> Option<Sandbox> {
    // Live tests use the developer's real key from the environment or .env.
    let _ = dotenvy::from_path(Path::new(env!("CARGO_MANIFEST_DIR")).join(".env"));
    std::env::var("NVIDIA_API_KEY").ok()?;
    Some(Sandbox::new(name))
}

fn live_cmd(s: &Sandbox) -> Command {
    let mut c = Command::new(BIN);
    c.current_dir(&s.project).env("RUSTY_HOME", &s.home).env("NO_COLOR", "1");
    c
}

#[test]
#[ignore]
fn live_fixes_a_bug_and_reports_stats() {
    let Some(s) = live("live-fix") else { return };
    std::fs::write(s.project.join("calc.py"), "def add(a, b):\n    return a - b\n").unwrap();
    std::fs::write(s.project.join("test_calc.py"), "from calc import add\nassert add(2, 3) == 5\nprint('ok')\n")
        .unwrap();
    let out = live_cmd(&s)
        .args(["--stats", "fix calc.py so python3 test_calc.py passes, and run it to check"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let run = Command::new("python3").arg("test_calc.py").current_dir(&s.project).output().unwrap();
    assert!(run.status.success(), "the bug is still there");
    let stats = String::from_utf8_lossy(&out.stderr);
    let line = stats.lines().rev().find(|l| l.starts_with('{')).expect("no --stats line");
    let v: serde_json::Value = serde_json::from_str(line).unwrap();
    assert!(v["requests"].as_u64().unwrap() >= 2);
}

#[test]
#[ignore]
fn live_goal_mode_closes_with_evidence() {
    let Some(s) = live("live-goal") else { return };
    let out = live_cmd(&s)
        .args([
            "--stats",
            "--goal",
            "create hello.sh that prints hello, make it executable, and run it to prove it works",
        ])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("goal done"), "{text}");
    assert!(s.project.join("hello.sh").exists());
}

#[test]
#[ignore]
fn live_ctrl_c_stops_quickly() {
    let Some(s) = live("live-sigint") else { return };
    let child = live_cmd(&s)
        .arg("write the numbers one to five thousand in words, one per line")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_secs(4));
    let sent = Instant::now();
    unsafe {
        libc::kill(child.id() as i32, libc::SIGINT);
    }
    let out = wait(child, Duration::from_secs(10));
    assert_eq!(out.status.code(), Some(130));
    assert!(sent.elapsed() < Duration::from_secs(3), "took {:?} to stop", sent.elapsed());
}

#[test]
#[ignore]
fn live_swarm_runs_workers_in_parallel() {
    let Some(s) = live("live-swarm") else { return };
    for (f, body) in [("a.py", "def alpha():\n    return 1\n"), ("b.py", "def beta():\n    return 2\n")] {
        std::fs::write(s.project.join(f), body).unwrap();
    }
    let out = live_cmd(&s)
        .args([
            "--agents",
            "swarm",
            "use the swarm tool with two workers, one per file (a.py, b.py), to report the function each defines. Then list both names.",
        ])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("alpha") && text.contains("beta"), "{text}");
    assert!(text.contains("swarm"), "swarm tool was not used:\n{text}");
}

#[test]
fn execution_modes_persist_without_changing_permissions() {
    let s = Sandbox::new("execution-modes");
    let out = s.repl(&["/mode vibe", "/settings", "/permissions check git push origin main"]);
    assert!(out.contains("execution vibe"), "{out}");
    assert!(out.contains("workers auto (≤3)"), "{out}");
    assert!(out.contains("swarm size      8"), "a mode must not rewrite the saved swarm size: {out}");
    assert!(out.contains("? asks first"), "vibe must not authorize git push: {out}");
    let settings: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(s.home.join("settings.json")).unwrap()).unwrap();
    assert_eq!(settings["execution_mode"], "vibe");
    let out = s.repl(&["/mode", "/agents off", "/mode careful", "/mode vibe"]);
    assert!(out.contains("checker one read-only check"), "{out}");
    let out = s.repl(&["/mode"]);
    assert!(out.contains("workers off"), "explicit delegation choice must persist: {out}");
    let out = s.repl(&["/agents default", "/mode"]);
    assert!(out.contains("workers auto"), "/agents default hands delegation back to the mode: {out}");
}

#[test]
fn execution_mode_cli_and_model_precedence() {
    let s = Sandbox::new("mode-models");
    s.repl(&["/mode careful"]);
    let out = s
        .cmd()
        .args(["--mode", "vibe", "--model", "explicit-model"])
        .env("RUSTY_VIBE_MODEL", "fast-model")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut child = out;
    writeln!(child.stdin.take().unwrap(), "/mode\n/exit").unwrap();
    let out = wait(child, Duration::from_secs(20));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("execution vibe") && text.contains("model explicit-model"), "{text}");
    // CLI choice must not overwrite the saved mode.
    assert!(s.repl(&["/mode"]).contains("execution careful"));
    let out = s.cmd().args(["--mode", "nonsense", "hi"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown execution mode"));
}

/// A scripted SSE endpoint drives the real loop without paying for inference.
fn scripted_endpoint(replies: Vec<serde_json::Value>) -> (String, std::thread::JoinHandle<Vec<serde_json::Value>>) {
    use std::io::{BufRead, BufReader, Read};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for reply in replies {
            let start = Instant::now();
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(pair) => break pair,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(start.elapsed() < Duration::from_secs(15), "expected another request");
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut size = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                if let Some((key, value)) = line.split_once(':') {
                    if key.eq_ignore_ascii_case("content-length") {
                        size = value.trim().parse().unwrap();
                    }
                }
            }
            let mut body = vec![0; size];
            reader.read_exact(&mut body).unwrap();
            requests.push(serde_json::from_slice(&body).unwrap());
            let sse = format!("data: {}\n\ndata: [DONE]\n\n", reply);
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", sse.len(), sse).unwrap();
        }
        requests
    });
    (url, handle)
}

fn tool_reply(name: &str, args: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "call", "type": "function",
        "function": {"name": name, "arguments": args.to_string()}}]}, "finish_reason": "tool_calls"}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5}})
}

fn text_reply(text: &str) -> serde_json::Value {
    serde_json::json!({"choices": [{"delta": {"content": text}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5}})
}

/// Runs rusty against a scripted endpoint and returns (stdout, stderr, requests).
fn scripted_run(
    s: &Sandbox,
    replies: Vec<serde_json::Value>,
    args: &[&str],
) -> (String, String, Vec<serde_json::Value>) {
    let (url, server) = scripted_endpoint(replies);
    let out = s
        .cmd()
        .env("RUSTY_BASE_URL", url)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map(|child| wait(child, Duration::from_secs(20)))
        .unwrap();
    let (stdout, stderr) =
        (String::from_utf8_lossy(&out.stdout).to_string(), String::from_utf8_lossy(&out.stderr).to_string());
    assert!(out.status.success(), "{stderr}");
    (stdout, stderr, server.join().unwrap())
}

#[test]
fn careful_goal_gets_one_checker_review_then_closes() {
    let s = Sandbox::new("careful-goal");
    let done = tool_reply("goal_done", serde_json::json!({"evidence": "checked"}));
    let (stdout, stderr, requests) = scripted_run(
        &s,
        vec![
            done.clone(),
            // The checker, a separate read-only worker.
            text_reply("finding: the empty-input case is not handled (main.py:3)"),
            // The main agent fixes it, verifies, and claims done again: no second review.
            tool_reply("bash", serde_json::json!({"command": "printf fixed-and-tested"})),
            done,
        ],
        &["--mode", "careful", "--goal", "check the workspace", "--stats"],
    );
    assert_eq!(requests.len(), 4);
    let checker = requests[1]["messages"].to_string();
    assert!(checker.contains("read-only reviewer"), "{checker}");
    assert!(!requests[1]["tools"].to_string().contains("goal_done"), "the checker can't close the goal");
    let fix = requests[2]["messages"].to_string();
    assert!(fix.contains("checker's review") && fix.contains("empty-input"), "{fix}");
    assert!(stdout.contains("careful check") && stdout.contains("fixed-and-tested"), "{stdout}");
    assert!(stderr.contains("\"goal\":\"done\""), "{stderr}");
}

#[test]
fn standard_and_vibe_finish_without_extra_reviews() {
    for mode in ["standard", "vibe"] {
        let s = Sandbox::new(mode);
        let (url, server) =
            scripted_endpoint(vec![tool_reply("goal_done", serde_json::json!({"evidence": "checked"}))]);
        let out = s
            .cmd()
            .env("RUSTY_BASE_URL", url)
            .args(["--mode", mode, "--goal", "inspect", "--stats"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map(|child| wait(child, Duration::from_secs(20)))
            .unwrap();
        assert!(out.status.success());
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["max_tokens"], if mode == "vibe" { 8192 } else { 16384 });
        let tools = requests[0]["tools"].to_string();
        assert_eq!(tools.contains("Run several read-only workers"), mode == "vibe");
    }
}

#[test]
fn careful_blocked_goal_does_not_spend_review_calls() {
    let s = Sandbox::new("careful-blocked");
    let (url, server) = scripted_endpoint(vec![tool_reply(
        "goal_done",
        serde_json::json!({"evidence": "need a target", "blocked": true}),
    )]);
    let out = s
        .cmd()
        .env("RUSTY_BASE_URL", url)
        .args(["--mode", "careful", "--goal", "inspect", "--stats"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map(|child| wait(child, Duration::from_secs(20)))
        .unwrap();
    assert!(out.status.success());
    assert_eq!(server.join().unwrap().len(), 1);
    assert!(String::from_utf8_lossy(&out.stderr).contains("\"goal\":\"blocked\""));
}

#[test]
fn careful_does_not_review_a_plain_answer() {
    let s = Sandbox::new("careful-answer");
    let reply = serde_json::json!({"choices": [{"delta": {"content": "answer"}, "finish_reason": "stop"}]});
    let (url, server) = scripted_endpoint(vec![reply]);
    let out = s
        .cmd()
        .env("RUSTY_BASE_URL", url)
        .args(["--mode", "careful", "explain the code"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map(|child| wait(child, Duration::from_secs(20)))
        .unwrap();
    assert!(out.status.success());
    assert_eq!(server.join().unwrap().len(), 1, "a read-only answer needs no second look");
}

#[test]
fn careful_checks_an_answer_after_a_change() {
    let s = Sandbox::new("careful-change");
    let (_, _, requests) = scripted_run(
        &s,
        vec![
            tool_reply("write_file", serde_json::json!({"path": "note.txt", "content": "hi"})),
            text_reply("done"),
            text_reply("no findings; could not run anything for a text file"),
            text_reply("done, and the checker found nothing"),
        ],
        &["--mode", "careful", "write a note"],
    );
    assert_eq!(requests.len(), 4);
    assert!(requests[3]["messages"].to_string().contains("checker's review"));
}

#[test]
fn careful_checks_once_per_goal_not_once_per_goal_turn() {
    let s = Sandbox::new("careful-goal-turns");
    let done = tool_reply("goal_done", serde_json::json!({"evidence": "checked"}));
    // Turn 1: claims done, the checker reviews, the agent answers in text and the turn ends.
    // Turn 2: claims done again and the goal closes without another review.
    let (_, stderr, requests) = scripted_run(
        &s,
        vec![done.clone(), text_reply("looks fine"), text_reply("still checking"), done],
        &["--mode", "careful", "--goal", "inspect", "--stats"],
    );
    assert_eq!(requests.len(), 4);
    assert!(stderr.contains("\"goal\":\"done\""), "{stderr}");
}

#[test]
fn truncated_reply_after_the_check_leaves_the_goal_open() {
    let s = Sandbox::new("careful-truncated");
    let done = tool_reply("goal_done", serde_json::json!({"evidence": "checked"}));
    let mut truncated = text_reply("I'll fix the empty case by");
    truncated["choices"][0]["finish_reason"] = serde_json::json!("length");
    let (url, server) = scripted_endpoint(vec![done, text_reply("finding: empty case"), truncated]);
    let out = s
        .cmd()
        .env("RUSTY_BASE_URL", url)
        .env("RUSTY_GOAL_MAX_TURNS", "1")
        .args(["--mode", "careful", "--goal", "inspect", "--stats"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map(|child| wait(child, Duration::from_secs(20)))
        .unwrap();
    assert!(out.status.success());
    assert_eq!(server.join().unwrap().len(), 3);
    assert!(String::from_utf8_lossy(&out.stderr).contains("\"goal\":\"open\""));
}

#[test]
fn yolo_still_refuses_a_destructive_command_when_nobody_is_watching() {
    let s = Sandbox::new("yolo-destructive");
    // Something precious in a (fake) home directory outside the project.
    let user_home = s.home.parent().unwrap().join("user-home");
    let precious = user_home.join("precious");
    std::fs::create_dir_all(precious.join("data")).unwrap();
    std::fs::write(precious.join("data/keep.txt"), "irreplaceable").unwrap();
    let answer = serde_json::json!({"choices": [{"delta": {"content": "ok"}, "finish_reason": "stop"}]});
    let (url, server) = scripted_endpoint(vec![
        tool_reply("bash", serde_json::json!({"command": "rm -rf ~/precious"})),
        tool_reply("bash", serde_json::json!({"command": "rm -rf build && mkdir build && echo hi > build/x"})),
        answer.clone(),
        // The destructive request moved the turn up to careful, so a checker reviews it.
        serde_json::json!({"choices": [{"delta": {"content": "no findings"}, "finish_reason": "stop"}]}),
        answer,
    ]);
    let out = s
        .cmd()
        .env("RUSTY_BASE_URL", url)
        .env("HOME", &user_home)
        .args(["--yolo", "clean up"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map(|child| wait(child, Duration::from_secs(20)))
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let requests = server.join().unwrap();
    assert!(precious.join("data/keep.txt").exists(), "a destructive command ran unattended");
    let told = requests[1]["messages"].to_string();
    assert!(told.contains("refused") && told.contains("needs a person"), "{told}");
    // Routine work in the same session still ran.
    assert!(s.project.join("build/x").exists());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("switched up") && stdout.contains("careful check"), "{stdout}");
}

#[test]
fn auto_mode_picks_per_request_and_an_explicit_mode_wins() {
    let s = Sandbox::new("auto-mode");
    let answer = || serde_json::json!({"choices": [{"delta": {"content": "ok"}, "finish_reason": "stop"}]});
    let (stdout, stderr, requests) = scripted_run(&s, vec![answer()], &["--stats", "plan the database migration"]);
    assert!(stdout.contains("◇ careful") && stdout.contains("mentions a migration and data"), "{stdout}");
    assert!(stderr.contains("\"mode_reason\":\"mentions a migration and data\""), "{stderr}");
    assert_eq!(requests.len(), 1, "a plan with no changes gets no checker");
    let (stdout, _, _) = scripted_run(&s, vec![answer()], &["sketch a quick landing page"]);
    assert!(stdout.contains("◇ vibe"), "{stdout}");
    let (stdout, _, _) = scripted_run(&s, vec![answer()], &["--mode", "standard", "plan the database migration"]);
    assert!(!stdout.contains("◇ careful") && !stdout.contains("auto ·"), "{stdout}");
    // Picking a mode never changes permissions.
    let out = s.repl(&["/mode auto", "/permissions check git push origin main", "/mode"]);
    assert!(out.contains("? asks first") && out.contains("execution auto"), "{out}");
}

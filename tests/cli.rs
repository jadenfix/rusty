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
    assert!(out.contains("orbit closed"), "no goodbye on EOF");
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
        "/permissions read-only",
        "/permissions check touch x",
    ]);
    assert!(out.contains("Ask(\"forced operation\")"), "{out}");
    assert!(out.contains("WorkspaceWrite"));
    assert!(out.contains("in auto mode → Allow"));
    assert!(out.contains("Deny(\"read-only mode"));
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

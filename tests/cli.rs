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
    /// Fake `kubectl`, `terraform` and friends go here, first on PATH, so no
    /// test can reach a real cluster or account.
    bin: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("rusty-e2e-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let project = root.join("project");
        let bin = root.join("bin");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        Self { home, project, bin }
    }

    /// Installs a fake command. It appends its arguments to `calls.log` in
    /// the sandbox root, then runs `body` with them.
    fn fake(&self, name: &str, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        let path = self.bin.join(name);
        std::fs::write(&path, format!("#!/usr/bin/env bash\necho \"{name} $*\" >> \"$RUSTY_TEST_CALLS\"\n{body}\n"))
            .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn calls(&self) -> String {
        std::fs::read_to_string(self.bin.parent().unwrap().join("calls.log")).unwrap_or_default()
    }

    fn cmd(&self) -> Command {
        let mut c = Command::new(BIN);
        c.current_dir(&self.project)
            .env("RUSTY_HOME", &self.home)
            .env("HOME", &self.home)
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env("RUSTY_TEST_CALLS", self.bin.parent().unwrap().join("calls.log"))
            .env("RUSTY_NO_DOTENV", "1")
            .env("NVIDIA_API_KEY", "offline-test-key")
            .env("NO_COLOR", "1")
            .env_remove("AWS_PROFILE")
            .env_remove("AWS_DEFAULT_PROFILE")
            .env_remove("AWS_ACCESS_KEY_ID")
            .env_remove("AWS_REGION")
            .env_remove("AWS_DEFAULT_REGION")
            .env_remove("CLOUDSDK_CORE_PROJECT")
            .env_remove("TF_WORKSPACE")
            .env_remove("KUBECONFIG")
            .env_remove("RUSTY_INFRA")
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
fn saved_memory_and_history_do_not_contain_credentials() {
    use std::os::unix::fs::PermissionsExt;
    let s = Sandbox::new("private-persistence");
    s.repl(&["/remember DB_PASSWORD=cli-secret-canary-12345", "/agents off", "/allow read_file", "/exit"]);
    fn inspect(dir: &Path) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                inspect(&p);
            } else {
                let text = std::fs::read_to_string(&p).unwrap();
                assert!(!text.contains("cli-secret-canary-12345"), "credential persisted in {}", p.display());
                if p.file_name().unwrap() == "history"
                    || p.file_name().unwrap() == "memory.jsonl"
                    || p.extension().is_some_and(|s| s == "json")
                {
                    assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600, "{}", p.display());
                }
            }
        }
    }
    inspect(&s.home);
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
    for cmd in [
        "/goal",
        "/loop",
        "/agents",
        "/swarm",
        "/permissions",
        "/memory",
        "/context",
        "/view",
        "/theme",
        "/review",
        "/target",
        "/audit",
        "/triage",
        "/change",
        "/postmortem",
    ] {
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
    let lines: Vec<&str> = out
        .lines()
        .map(|l| l.trim_start().trim_start_matches('›').trim_start())
        .filter(|l| l.starts_with(['✓', '?', '‼', '✗']))
        .collect();
    assert_eq!(lines.len(), 7, "{out}");
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

#[test]
fn target_is_detected_without_escalating_an_idle_session() {
    let s = Sandbox::new("target");
    s.fake("kubectl", "printf 'prod-eu|payments'");
    s.fake("aws", "case \"$*\" in *Account*) printf '123456789012\\n';; *region*) printf 'eu-west-1\\n';; esac");
    std::fs::write(s.project.join("main.tf"), "").unwrap();
    std::fs::create_dir_all(s.project.join(".terraform")).unwrap();
    std::fs::write(s.project.join(".terraform/environment"), "staging").unwrap();
    let out = s.repl(&["/mode", "/target", "/settings"]);
    assert!(out.contains("target looks like production: kube prod-eu/payments"), "{out}");
    assert!(
        out.contains("execution auto") && !out.contains("last pick careful"),
        "an idle production session must stay auto: {out}"
    );
    assert!(out.contains("terraform staging"), "{out}");
    assert!(!out.contains("aws "), "no aws profile is configured in the sandbox: {out}");
    // Nothing was saved: the next session with a harmless target is standard again.
    s.fake("kubectl", "printf 'kind-dev|default'");
    let out = s.repl(&["/mode", "/target"]);
    assert!(out.contains("execution auto") && out.contains("kube kind-dev/default"), "{out}");
    assert!(!out.contains("production"), "{out}");
    // The flag wins over the heuristic.
    s.fake("kubectl", "printf 'prod|default'");
    let mut child = s.cmd().args(["--mode", "vibe"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    writeln!(child.stdin.take().unwrap(), "/mode").unwrap();
    let text = String::from_utf8_lossy(&wait(child, Duration::from_secs(20)).stdout).to_string();
    assert!(text.contains("execution vibe") && text.contains("production"), "{text}");
}

#[test]
fn aws_identity_comes_from_one_sts_call() {
    let s = Sandbox::new("target-aws");
    s.fake("aws", "case \"$*\" in *Account*) printf '123456789012\\n';; esac");
    // Startup reads local config only: no aws CLI, no network.
    let out = s
        .cmd()
        .env("AWS_PROFILE", "ops")
        .env("AWS_DEFAULT_REGION", "eu-west-1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut child = out;
    writeln!(child.stdin.take().unwrap(), "/exit").unwrap();
    let text = String::from_utf8_lossy(&wait(child, Duration::from_secs(20)).stdout).to_string();
    assert!(text.contains("aws ops (eu-west-1)"), "{text}");
    assert!(!s.calls().contains("aws"), "startup ran the aws CLI: {}", s.calls());
    // /target asks for the identity, with exactly one STS call.
    let out = s
        .cmd()
        .env("AWS_PROFILE", "ops")
        .env("AWS_DEFAULT_REGION", "eu-west-1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut child = out;
    writeln!(child.stdin.take().unwrap(), "/target").unwrap();
    let text = String::from_utf8_lossy(&wait(child, Duration::from_secs(20)).stdout).to_string();
    assert!(text.contains("aws ops (123456789012, eu-west-1)"), "{text}");
    assert_eq!(s.calls().matches("aws sts get-caller-identity").count(), 1, "{}", s.calls());
    let out = s.repl(&["/target"]);
    assert!(out.contains("none detected"), "{out}");
}

#[test]
fn secrets_in_tool_output_never_reach_the_model_or_the_session_file() {
    let s = Sandbox::new("redact");
    std::fs::write(
        s.project.join(".env"),
        "DB_HOST=db.internal\nDB_PASSWORD=hunter22-real\nAWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE\n",
    )
    .unwrap();
    let answer = serde_json::json!({"choices": [{"delta": {"content": "done"}, "finish_reason": "stop"}]});
    let (url, server) = scripted_endpoint(vec![tool_reply("bash", serde_json::json!({"command": "cat .env"})), answer]);
    let out = s
        .cmd()
        .env("RUSTY_BASE_URL", url)
        .args(["--yolo", "show me the env file"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map(|child| wait(child, Duration::from_secs(20)))
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let requests = server.join().unwrap();
    let seen = requests[1]["messages"].to_string();
    assert!(!seen.contains("hunter22-real") && !seen.contains("AKIAIOSFODNN7EXAMPLE"), "{seen}");
    assert!(seen.contains("DB_HOST=db.internal") && seen.contains("DB_PASSWORD=[redacted]"), "{seen}");
    assert!(seen.contains("redacted 2 secret values"), "{seen}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("2 secrets redacted"));
    let mut sessions = Vec::new();
    find_files(&s.home, "json", &mut sessions);
    assert!(!sessions.is_empty());
    for p in sessions {
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(!text.contains("hunter22-real"), "{} leaks the secret", p.display());
    }
}

/// Runs one headless prompt against a scripted endpoint and returns
/// (stdout, stderr, the requests the endpoint saw).
fn scripted_run(
    s: &Sandbox,
    args: &[&str],
    replies: Vec<serde_json::Value>,
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
        .map(|child| wait(child, Duration::from_secs(30)))
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let requests = server.join().unwrap();
    (String::from_utf8_lossy(&out.stdout).to_string(), String::from_utf8_lossy(&out.stderr).to_string(), requests)
}

fn text_reply(text: &str) -> serde_json::Value {
    serde_json::json!({"choices": [{"delta": {"content": text}, "finish_reason": "stop"}]})
}

fn bash_reply(command: &str) -> serde_json::Value {
    tool_reply("bash", serde_json::json!({"command": command}))
}

#[test]
fn infrastructure_commands_are_audited_and_summarised() {
    let s = Sandbox::new("audit");
    s.fake(
        "kubectl",
        "case \"$*\" in get*) printf 'api-1 Running\\n';; apply*) printf 'deployment.apps/api configured\\n';; esac",
    );
    std::fs::write(s.project.join("deploy.yaml"), "kind: Deployment\n").unwrap();
    let (out, _, _) = scripted_run(
        &s,
        &["--yolo", "--mode", "standard", "roll out the deployment"],
        vec![
            bash_reply("kubectl get pods"),
            bash_reply("kubectl apply -f deploy.yaml"),
            bash_reply("ls"),
            text_reply("done"),
        ],
    );
    assert!(out.contains("change record · 1 change"), "{out}");
    assert!(out.contains("✓ kubectl apply -f deploy.yaml") && out.contains("not verified"), "{out}");
    let mut logs = Vec::new();
    find_files(&s.home, "jsonl", &mut logs);
    let audit = logs.iter().find(|p| p.ends_with("audit.jsonl")).expect("no audit log");
    let lines: Vec<serde_json::Value> =
        std::fs::read_to_string(audit).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), 2, "ls is not an infrastructure command: {lines:?}");
    assert_eq!(lines[0]["command"], "kubectl get pods");
    assert_eq!(lines[0]["mutating"], false);
    assert_eq!(lines[1]["verb"], "apply");
    assert_eq!(lines[1]["mutating"], true);
    assert_eq!(lines[1]["outcome"], "ran");
    assert_eq!(lines[1]["exit"], 0);
    assert_eq!(lines[1]["mode"], "standard");
    let mut records = Vec::new();
    find_files(&s.home, "txt", &mut records);
    assert!(records.iter().any(|p| p.to_string_lossy().ends_with(".changes.txt")), "no change record saved");
    // Refusals are logged too, and the log survives across sessions.
    let (out, _, _) = scripted_run(
        &s,
        &["--permissions", "read-only", "delete it"],
        vec![bash_reply("kubectl delete deploy api"), text_reply("could not")],
    );
    assert!(out.contains("denied by permissions"), "{out}");
    // A destructive command needs a person; with nobody watching it is declined and logged.
    let (out, _, _) =
        scripted_run(&s, &["drop it"], vec![bash_reply("terraform destroy -auto-approve"), text_reply("no")]);
    assert!(out.contains("refused"), "{out}");
    assert!(!s.calls().contains("terraform destroy"), "{}", s.calls());
    let out = s.repl(&["/audit", "/changes"]);
    assert!(out.contains("terraform destroy -auto-approve") && out.contains("declined"), "{out}");
    assert!(out.contains("kubectl get pods") && out.contains("read"), "{out}");
    assert!(out.contains("kubectl apply -f deploy.yaml") && out.contains("change"), "{out}");
    assert!(out.contains("kubectl delete deploy api") && out.contains("denied"), "{out}");
    assert!(out.contains("nothing changed in infrastructure this session"), "{out}");
}

#[test]
fn a_change_is_snapshotted_before_it_runs() {
    use std::os::unix::fs::PermissionsExt;
    let s = Sandbox::new("snapshot");
    s.fake(
        "kubectl",
        "case \"$*\" in \
         \"get -f deploy.yaml -o yaml --ignore-not-found -n payments\") printf 'apiVersion: apps/v1\\nkind: Deployment\\nmetadata:\\n  name: api\\n  resourceVersion: \"7\"\\nspec:\\n  replicas: 1\\nstatus:\\n  readyReplicas: 1\\n';; \
         \"get -f new.yaml\"*) exit 0;; \
         apply*) printf 'deployment.apps/api configured\\n';; esac",
    );
    s.fake("helm", "case \"$*\" in \"get values\"*) printf 'image:\\n  tag: v1\\n';; \"get manifest\"*) printf 'kind: Deployment\\n';; upgrade*) printf 'Release api has been upgraded\\n';; esac");
    std::fs::write(s.project.join("deploy.yaml"), "kind: Deployment\n").unwrap();
    let (out, _, requests) = scripted_run(
        &s,
        &["--yolo", "--mode", "standard", "deploy"],
        vec![
            bash_reply("kubectl apply -f deploy.yaml -n payments"),
            bash_reply("kubectl apply -f new.yaml"),
            bash_reply("helm upgrade api ./chart -n payments"),
            text_reply("done"),
        ],
    );
    let log = s.calls();
    // The first call is the target probe at startup.
    let calls: Vec<&str> = log.lines().skip(1).collect();
    assert_eq!(calls[0], "kubectl get -f deploy.yaml -o yaml --ignore-not-found -n payments", "{calls:?}");
    assert_eq!(calls[1], "kubectl apply -f deploy.yaml -n payments");
    assert_eq!(calls[4], "helm get values api -o yaml -n payments");
    assert_eq!(calls[5], "helm get manifest api -n payments");
    assert_eq!(calls[6], "helm upgrade api ./chart -n payments");
    assert!(out.contains("⎘ snapshot") && out.contains("nothing to snapshot: the objects don't exist yet"), "{out}");
    let seen = requests[1]["messages"].to_string();
    assert!(
        seen.contains("pre-change snapshot of the previous state") && seen.contains("Rollback: kubectl apply -f "),
        "{seen}"
    );
    let mut snaps = Vec::new();
    find_files(&s.home, "yaml", &mut snaps);
    let before = snaps.iter().find(|p| p.ends_with("before.yaml")).expect("no before.yaml");
    let dir = before.parent().unwrap();
    assert!(dir.file_name().unwrap().to_string_lossy().ends_with("-kubectl-apply"));
    assert!(std::fs::read_to_string(before).unwrap().contains("resourceVersion"));
    let rollback = std::fs::read_to_string(dir.join("rollback.yaml")).unwrap();
    assert!(
        !rollback.contains("resourceVersion")
            && !rollback.contains("readyReplicas")
            && rollback.contains("replicas: 1"),
        "{rollback}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("command.txt")).unwrap().trim(),
        "kubectl apply -f deploy.yaml -n payments"
    );
    assert_eq!(std::fs::metadata(before).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(std::fs::metadata(dir).unwrap().permissions().mode() & 0o777, 0o700);
    assert!(snaps.iter().any(|p| p.ends_with("values.yaml")) && snaps.iter().any(|p| p.ends_with("manifest.yaml")));
    assert!(out.contains("change record · 3 changes"), "{out}");
    assert!(out.contains("snapshot ") && out.contains("nothing to snapshot"), "{out}");
}

#[test]
fn careful_mode_refuses_an_apply_without_a_fresh_diff() {
    let s = Sandbox::new("gate");
    s.fake("kubectl", "case \"$*\" in diff*) printf 'diff'; exit 1;; apply*) printf 'configured\\n';; esac");
    s.fake("terraform", "case \"$*\" in plan*) touch rusty.tfplan;; esac; printf 'ok\\n'");
    std::fs::write(s.project.join("deploy.yaml"), "replicas: 1\n").unwrap();
    let (out, _, requests) = scripted_run(
        &s,
        &["--yolo", "--mode", "careful", "deploy"],
        vec![
            bash_reply("kubectl apply -f deploy.yaml"),
            bash_reply("kubectl diff -f deploy.yaml"),
            bash_reply("kubectl apply -f deploy.yaml"),
            tool_reply("edit_file", serde_json::json!({"path": "deploy.yaml", "old_string": "1", "new_string": "2"})),
            bash_reply("kubectl apply -f deploy.yaml"),
            bash_reply("terraform apply"),
            bash_reply("terraform plan -out=rusty.tfplan"),
            bash_reply("terraform apply rusty.tfplan"),
            text_reply("done"),
            text_reply("done"),
            text_reply("done"),
        ],
    );
    let log = s.calls();
    // Leave out the target probe and careful mode's verification commands.
    let calls: Vec<&str> = log
        .lines()
        .filter(|l| !l.contains("config view") && !l.contains("-o name") && !l.contains("-detailed-exitcode"))
        .collect();
    assert_eq!(
        calls,
        [
            "kubectl diff -f deploy.yaml",
            "kubectl get -f deploy.yaml -o yaml --ignore-not-found",
            "kubectl apply -f deploy.yaml",
            "terraform plan -out=rusty.tfplan",
            "terraform state pull",
            "terraform apply rusty.tfplan",
        ],
        "blocked commands must never reach the tool"
    );
    let result = |i: usize| requests[i]["messages"].as_array().unwrap().last().unwrap()["content"].to_string();
    assert!(
        result(1).contains("blocked by careful mode: no diff or plan") && result(1).contains("--dry-run=server"),
        "{}",
        result(1)
    );
    assert!(result(3).contains("exit code: 0"), "{}", result(3));
    assert!(result(5).contains("stale"), "{}", result(5));
    assert!(result(6).contains("plan -out=rusty.tfplan"), "{}", result(6));
    assert!(out.contains("blocked by careful mode"), "{out}");
    assert!(out.contains("change record · 5 changes"), "{out}");
    assert!(out.matches("blocked").count() >= 3, "{out}");
    // Standard mode only advises; it does not refuse.
    let s2 = Sandbox::new("gate-standard");
    s2.fake("kubectl", "printf 'configured\\n'");
    std::fs::write(s2.project.join("deploy.yaml"), "replicas: 1\n").unwrap();
    scripted_run(
        &s2,
        &["--yolo", "--mode", "standard", "deploy"],
        vec![bash_reply("kubectl apply -f deploy.yaml"), text_reply("done")],
    );
    assert!(s2.calls().contains("kubectl apply -f deploy.yaml"));
}

#[test]
fn careful_mode_verifies_rollouts_after_a_change() {
    let s = Sandbox::new("verify");
    s.fake(
        "kubectl",
        "case \"$*\" in \
         diff*) exit 1;; \
         \"get -f deploy.yaml -o name\"*) printf 'deployment.apps/api\\nservice/api\\n';; \
         \"rollout status deployment.apps/api\"*) printf 'deployment \"api\" successfully rolled out\\n';; \
         \"rollout status deploy/web\"*) printf 'error: timed out waiting for the condition\\n' >&2; exit 1;; \
         apply*|scale*) printf 'ok\\n';; esac",
    );
    s.fake("terraform", "case \"$*\" in plan*-detailed-exitcode*) printf 'No changes.\\n';; *) printf 'ok\\n';; esac");
    std::fs::write(s.project.join("deploy.yaml"), "kind: Deployment\n").unwrap();
    let (out, _, requests) = scripted_run(
        &s,
        &["--yolo", "--mode", "careful", "deploy"],
        vec![
            bash_reply("kubectl diff -f deploy.yaml"),
            bash_reply("kubectl apply -f deploy.yaml"),
            bash_reply("kubectl scale deploy/web --replicas=3"),
            bash_reply("terraform plan -out=p.tfplan"),
            bash_reply("terraform apply p.tfplan"),
            text_reply("done"),
            text_reply("done"),
            text_reply("done"),
        ],
    );
    let log = s.calls();
    assert!(log.contains("kubectl rollout status deployment.apps/api --timeout=180s"), "{log}");
    assert!(log.contains("kubectl rollout status deploy/web --timeout=180s"), "{log}");
    assert!(log.contains("terraform plan -detailed-exitcode -input=false -no-color"), "{log}");
    let result = |i: usize| requests[i]["messages"].as_array().unwrap().last().unwrap()["content"].to_string();
    assert!(
        result(2).contains("[verified: deployment.apps/api: deployment \\\"api\\\" successfully rolled out]"),
        "{}",
        result(2)
    );
    assert!(
        result(3).contains("verification FAILED: deploy/web: error: timed out") && result(3).contains("NOT healthy"),
        "{}",
        result(3)
    );
    assert!(result(5).contains("[verified: post-apply plan is empty"), "{}", result(5));
    assert!(out.contains("✓ verified") && out.contains("✗ not verified"), "{out}");
    assert!(out.contains("kubectl apply -f deploy.yaml") && out.contains("· verified"), "{out}");
    assert!(out.contains("kubectl scale deploy/web") && out.contains("NOT verified"), "{out}");
    // Standard mode does not wait.
    let s2 = Sandbox::new("verify-standard");
    s2.fake("kubectl", "printf 'ok\\n'");
    scripted_run(
        &s2,
        &["--yolo", "--mode", "standard", "x"],
        vec![bash_reply("kubectl scale deploy/web --replicas=3"), text_reply("done")],
    );
    assert!(!s2.calls().contains("rollout status"), "{}", s2.calls());
}

fn find_files(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            find_files(&p, ext, out);
        } else if p.extension().is_some_and(|x| x == ext) {
            out.push(p);
        }
    }
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
    c.current_dir(&s.project).env("RUSTY_HOME", &s.home).env("NO_COLOR", "1").env("RUSTY_INFRA", "off");
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

#[test]
fn careful_goal_gets_one_checker_review_then_closes() {
    let s = Sandbox::new("careful-goal");
    let done = tool_reply("goal_done", serde_json::json!({"evidence": "checked"}));
    let (stdout, stderr, requests) = scripted_run(
        &s,
        &["--mode", "careful", "--goal", "check the workspace", "--stats"],
        vec![
            done.clone(),
            // The checker, a separate read-only worker.
            text_reply("finding: the empty-input case is not handled (main.py:3)"),
            // The main agent fixes it, verifies, and claims done again: no second review.
            tool_reply("bash", serde_json::json!({"command": "printf fixed-and-tested"})),
            done,
        ],
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
        &["--mode", "careful", "write a note"],
        vec![
            tool_reply("write_file", serde_json::json!({"path": "note.txt", "content": "hi"})),
            text_reply("done"),
            text_reply("no findings; could not run anything for a text file"),
            text_reply("done, and the checker found nothing"),
        ],
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
        &["--mode", "careful", "--goal", "inspect", "--stats"],
        vec![done.clone(), text_reply("looks fine"), text_reply("still checking"), done],
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
    let (stdout, stderr, requests) = scripted_run(&s, &["--stats", "plan the database migration"], vec![answer()]);
    assert!(stdout.contains("◇ careful") && stdout.contains("mentions a migration and data"), "{stdout}");
    assert!(stderr.contains("\"mode_reason\":\"mentions a migration and data\""), "{stderr}");
    assert_eq!(requests.len(), 1, "a plan with no changes gets no checker");
    let (stdout, _, _) = scripted_run(&s, &["sketch a quick landing page"], vec![answer()]);
    assert!(stdout.contains("◇ vibe"), "{stdout}");
    let (stdout, _, _) = scripted_run(&s, &["--mode", "standard", "plan the database migration"], vec![answer()]);
    assert!(!stdout.contains("◇ careful") && !stdout.contains("auto ·"), "{stdout}");
    // Picking a mode never changes permissions.
    let out = s.repl(&["/mode auto", "/permissions check git push origin main", "/mode"]);
    assert!(out.contains("? asks first") && out.contains("execution auto"), "{out}");
}

#[test]
fn auto_keeps_questions_standard_even_with_a_production_target() {
    let s = Sandbox::new("auto-production-question");
    s.fake("kubectl", "printf 'prod-eu|default'");
    let (stdout, _, requests) = scripted_run(&s, &["explain the database migration"], vec![text_reply("explanation")]);
    assert!(stdout.contains("◇ standard"), "{stdout}");
    assert_eq!(requests.len(), 1);
    let (stdout, _, _) = scripted_run(&s, &["update the service configuration"], vec![text_reply("plan")]);
    assert!(stdout.contains("◇ careful"), "{stdout}");
}

#[test]
fn saved_response_style_reaches_every_request_through_the_real_repl() {
    struct Daemon(std::process::Child, PathBuf);
    impl Drop for Daemon {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
            let _ = std::fs::remove_dir_all(&self.1);
        }
    }
    let s = Sandbox::new("repl-tone");
    std::fs::write(s.project.join("hello.txt"), "hello").unwrap();
    let root = PathBuf::from(format!("/tmp/rusty-repl-tone-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let _daemon = Daemon(
        Command::new(env!("CARGO_BIN_EXE_rusty-memoryd"))
            .args(["--home", root.to_str().unwrap()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
        root.clone(),
    );
    let start = Instant::now();
    while !root.join("memory/advisor.sock").exists() {
        assert!(start.elapsed() < Duration::from_secs(3));
        std::thread::sleep(Duration::from_millis(10));
    }
    let (url, server) = scripted_endpoint(vec![
        tool_reply("read_file", serde_json::json!({"path":"hello.txt"})),
        text_reply("The file says hello."),
    ]);
    let mut child = s
        .cmd()
        .env("RUSTY_HOME", &root)
        .env("RUSTY_BASE_URL", url)
        .args(["--memory", "on", "--agents", "off"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(child.stdin.take().unwrap(), "/remember preference: Response style: concise, warm, three bullets.\nRead hello.txt and answer in one sentence this time.\n/exit").unwrap();
    let out = wait(child, Duration::from_secs(10));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 2);
    for body in requests {
        let system = body["messages"][0]["content"].as_str().unwrap();
        assert!(system.contains("Response style: concise, warm, three bullets."));
        assert!(system.contains("current instructions and tool permissions take precedence"));
        assert!(body["messages"].to_string().contains("one sentence this time"));
    }
}

#[test]
fn blank_directory_uses_default_and_one_empty_reply_recovers() {
    let s = Sandbox::new("empty-recovery");
    std::fs::write(s.project.join("hello.txt"), "hello").unwrap();
    let (stdout, _, requests) = scripted_run(
        &s,
        &["--mode", "vibe", "inspect files"],
        vec![tool_reply("list_files", serde_json::json!({"path":""})), text_reply(""), text_reply("Found hello.txt")],
    );
    assert!(stdout.contains("Found hello.txt"));
    assert_eq!(requests.len(), 3);
    assert!(requests[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["role"] == "tool" && m["content"].as_str().is_some_and(|s| s.contains("hello.txt"))));
}

#[test]
fn two_empty_replies_fail_without_an_unbounded_retry_loop() {
    let s = Sandbox::new("empty-fail");
    let (url, server) = scripted_endpoint(vec![text_reply(""), text_reply("")]);
    let out = s.cmd().env("RUSTY_BASE_URL", url).args(["--mode", "vibe", "inspect files"]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("two empty replies"));
    assert_eq!(server.join().unwrap().len(), 2);
}

#[test]
fn swarm_overlaps_three_workers_and_keeps_them_read_only() {
    use serde_json::json;
    use std::io::{BufRead, BufReader, Read};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Condvar, Mutex};
    let s = Sandbox::new("parallel-swarm");
    for i in 0..3 {
        std::fs::write(s.project.join(format!("file{i}.txt")), format!("FACT-{i}")).unwrap();
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
    let gate = Arc::new((Mutex::new(0usize), Condvar::new()));
    let gate_server = gate.clone();
    let requests = Arc::new(AtomicUsize::new(0));
    let requests_server = requests.clone();
    let server = std::thread::spawn(move || {
        let start = Instant::now();
        let mut handlers = Vec::new();
        // Four lead requests and three requests per worker.
        while requests_server.load(Ordering::SeqCst) < 13 {
            let (mut stream, _) = match listener.accept() {
                Ok(p) => p,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(start.elapsed() < Duration::from_secs(15), "missing swarm request");
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(e) => panic!("{e}"),
            };
            let gate = gate_server.clone();
            let requests = requests_server.clone();
            handlers.push(std::thread::spawn(move || {
                stream.set_nonblocking(false).unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut size = 0;
                let mut headers = 0;
                loop {
                    let mut line = String::new();
                    match reader.read_line(&mut line) {
                        // A client may open a speculative idle connection.
                        Ok(0) if headers == 0 => return,
                        Err(e) if headers == 0 && matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => return,
                        Ok(0) => panic!("incomplete request headers"),
                        Err(e) => panic!("{e}"),
                        Ok(_) => headers += 1,
                    }
                    if line == "\r\n" { break; }
                    if let Some((key, val)) = line.split_once(':') {
                        if key.eq_ignore_ascii_case("content-length") { size = val.trim().parse().unwrap(); }
                    }
                }
                let mut bytes = vec![0; size]; reader.read_exact(&mut bytes).unwrap();
                let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                requests.fetch_add(1, Ordering::SeqCst);
                let messages = body["messages"].as_array().unwrap();
                let prompt = messages.iter().rev().find(|m| m["role"] == "user").unwrap()["content"].as_str().unwrap();
                let steps = messages.iter().filter(|m| m["role"] == "tool").count();
                let reply = if let Some(i) = prompt.strip_prefix("WORKER-").and_then(|p| p.chars().next()).and_then(|c| c.to_digit(10)) {
                    assert!(!body["tools"].to_string().contains("\"write_file\""), "write tool exposed to worker");
                    match steps {
                        0 => {
                            let (lock, cv) = &*gate;
                            let mut entered = lock.lock().unwrap(); *entered += 1; cv.notify_all();
                            let (entered, _) = cv.wait_timeout_while(entered, Duration::from_secs(5), |n| *n < 3).unwrap();
                            assert_eq!(*entered, 3, "workers did not overlap");
                            // Even a forged write tool call must be denied by policy.
                            tool_reply("write_file", json!({"path":"forbidden.txt","content":"worker wrote"}))
                        }
                        1 => {
                            assert!(messages.last().unwrap()["content"].as_str().unwrap().contains("denied"));
                            tool_reply("read_file", json!({"path":format!("file{i}.txt")}))
                        }
                        _ => {
                            assert!(messages.last().unwrap()["content"].as_str().unwrap().contains(&format!("FACT-{i}")));
                            text_reply(&format!("FACT-{i}"))
                        }
                    }
                } else {
                    match steps {
                        0 => tool_reply("swarm", json!({"tasks": (0..3).map(|i| json!({"description":format!("slice-{i}"),"prompt":format!("WORKER-{i}: inspect file{i}.txt")})).collect::<Vec<_>>()})),
                        1 => {
                            let report = messages.last().unwrap()["content"].as_str().unwrap();
                            assert_eq!(report.matches("(done)").count(), 3);
                            assert!((0..3).all(|i| report.contains(&format!("FACT-{i}"))));
                            tool_reply("write_file", json!({"path":"report.md","content":report}))
                        }
                        2 => tool_reply("bash", json!({"command":"test -s report.md && printf verified > proof.txt"})),
                        _ => text_reply("Done and verified."),
                    }
                };
                let data = format!("data: {reply}\n\ndata: [DONE]\n\n");
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",data.len(),data).unwrap();
            }));
        }
        for h in handlers {
            h.join().unwrap();
        }
    });
    let child = s
        .cmd()
        .env("RUSTY_BASE_URL", endpoint)
        .args(["--yolo", "--mode", "standard", "--agents", "swarm", "inspect three slices with a swarm"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let out = wait(child, Duration::from_secs(20));
    server.join().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(*gate.0.lock().unwrap(), 3);
    assert_eq!(requests.load(Ordering::SeqCst), 13);
    assert!(!s.project.join("forbidden.txt").exists());
    assert_eq!(std::fs::read_to_string(s.project.join("proof.txt")).unwrap(), "verified");
}

#[test]
fn unchanged_call_loop_pauses_all_profiles_without_closing_the_goal() {
    use serde_json::json;
    for mode in ["careful", "standard", "vibe"] {
        let s = Sandbox::new(&format!("stalled-{mode}"));
        s.fake("probe", "printf unchanged");
        let calls: Vec<_> = (0..6)
            .map(|i| {
                let (name, args) = if i < 4 {
                    ("bash", if i % 2 == 0 { "{\"command\":\"probe\"}" } else { "{ \"command\" : \"probe\" }" })
                } else if i == 4 {
                    ("write_file", "{\"path\":\"must-not-write.txt\",\"content\":\"wrong\"}")
                } else {
                    ("goal_done", "{\"evidence\":\"done\"}")
                };
                json!({"index":i,"id":format!("call-{i}"),"type":"function","function":{"name":name,"arguments":args}})
            })
            .collect();
        let reply = json!({"choices":[{"delta":{"tool_calls":calls},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":5}});
        let (url, server) = scripted_endpoint(vec![reply]);
        let out = s
            .cmd()
            .env("RUSTY_BASE_URL", url)
            .args(["--yolo", "--mode", mode, "--goal", "inspect", "--stats"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map(|c| wait(c, Duration::from_secs(20)))
            .unwrap();
        assert!(!out.status.success(), "{mode} marked a stalled task successful");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("four consecutive identical") && stderr.contains("\"goal\":\"open\""), "{stderr}");
        assert_eq!(s.calls().lines().count(), 4, "the guard should stop later tools in the batch");
        assert!(!s.project.join("must-not-write.txt").exists());
        assert_eq!(server.join().unwrap().len(), 1, "stopping does not spend another review/request");
    }
}

#[test]
fn repeated_call_with_new_results_is_progress() {
    use serde_json::json;
    let s = Sandbox::new("changing-results");
    s.fake("probe", "wc -l < \"$RUSTY_TEST_CALLS\"");
    let calls: Vec<_> = (0..6).map(|i| json!({"index":i,"id":format!("call-{i}"),"type":"function","function":{"name":"bash","arguments":"{\"command\":\"probe\"}"}}))
        .chain(std::iter::once(json!({"index":6,"id":"done","type":"function","function":{"name":"goal_done","arguments":"{\"evidence\":\"six new observations\"}"}}))).collect();
    let reply = json!({"choices":[{"delta":{"tool_calls":calls},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":5}});
    let (stdout, stderr, requests) =
        scripted_run(&s, &["--yolo", "--mode", "standard", "--goal", "inspect", "--stats"], vec![reply]);
    assert_eq!(requests.len(), 1);
    assert_eq!(s.calls().lines().count(), 6);
    assert!(stderr.contains("\"goal\":\"done\""), "{stderr}");
    assert!(!stdout.contains("returned the same result"), "new observations must not trigger repetition warnings");
}

#[test]
fn careful_goal_keeps_its_single_checker_budget_after_resume() {
    use serde_json::json;
    let s = Sandbox::new("resumed-checker");
    let done = tool_reply("goal_done", json!({"evidence":"checked"}));
    let mut partial = text_reply("fixes still in progress");
    partial["choices"][0]["finish_reason"] = json!("length");
    let (url, server) = scripted_endpoint(vec![done.clone(), text_reply("reviewed once; fix edge case"), partial]);
    let out = s
        .cmd()
        .env("RUSTY_BASE_URL", url)
        .env("RUSTY_GOAL_MAX_TURNS", "1")
        .args(["--mode", "careful", "--goal", "inspect", "--stats"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map(|c| wait(c, Duration::from_secs(20)))
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("\"goal\":\"open\""));
    assert_eq!(server.join().unwrap().len(), 3);
    let (url, server) = scripted_endpoint(vec![done]);
    let mut child = s
        .cmd()
        .env("RUSTY_BASE_URL", url)
        .args(["--mode", "careful", "--continue"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(child.stdin.take().unwrap(), "/goal resume\n/exit").unwrap();
    let out = wait(child, Duration::from_secs(20));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("goal done"));
    assert_eq!(server.join().unwrap().len(), 1, "resume must not purchase a second checker");
}

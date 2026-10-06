//! Real-process service tests. No model or cloud account is involved.
use rusty::advisor::{rpc, Hooks, Mode, Request, Response};
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
static NEXT: AtomicU64 = AtomicU64::new(1);

struct Service {
    root: PathBuf,
    child: Child,
}
impl Service {
    fn new() -> Self {
        let root = PathBuf::from("/tmp").join(format!(
            "rm-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_rusty-memoryd"))
            .arg("--home")
            .arg(&root)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let s = Self { root, child };
        s.ready();
        s
    }
    fn ready(&self) {
        let start = Instant::now();
        while rpc(
            &self.root.join("memory/advisor.sock"),
            &Request {
                op: "status".into(),
                scope: "ready".into(),
                session: "ready".into(),
                seq: 0,
                mode: Mode::On,
                data: Value::Null,
            },
        )
        .is_err()
        {
            assert!(start.elapsed() < Duration::from_secs(3));
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn call(&self, op: &str, scope: &str, seq: u64, data: Value) -> Response {
        rpc(
            &self.root.join("memory/advisor.sock"),
            &Request { op: op.into(), scope: scope.into(), session: "session".into(), seq, mode: Mode::On, data },
        )
        .unwrap()
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn daemon_restarts_preserve_l2_but_not_temporary_observations() {
    let mut s = Service::new();
    for _ in 0..30 {
        drop(std::os::unix::net::UnixStream::connect(s.root.join("memory/advisor.sock")).unwrap());
        assert!(!s.call("status", "ready", 0, Value::Null).error);
    }
    let id = s
        .call("remember", "repo-a", 0, json!({"kind":"preference","text":"Always inspect workspace packages manifest"}))
        .id;
    assert!(!id.is_empty());
    assert!(!s.call("begin", "repo-a", 1, json!({"text":"inspect workspace packages manifest"})).error);
    assert_eq!(s.call("advice", "repo-a", 1, Value::Null).id, id);
    assert!(s.call("advice", "repo-a", 1, Value::Null).text.is_empty(), "duplicate advice");
    assert!(!s.call("feedback", "repo-a", 1, json!({"helpful":true})).error);
    let snapshot = s.call("export", "repo-a", 1, Value::Null).data;
    assert!(s.call("list", "repo-b", 0, Value::Null).text.is_empty());
    assert!(s.call("event", "repo-a", 1, json!({})).error, "stale observation accepted");
    let second = Command::new(env!("CARGO_BIN_EXE_rusty-memoryd")).arg("--home").arg(&s.root).output().unwrap();
    assert!(!second.status.success());
    assert_eq!(std::fs::metadata(s.root.join("memory")).unwrap().permissions().mode() & 0o777, 0o700);
    assert_eq!(std::fs::metadata(s.root.join("memory/advisor.sock")).unwrap().permissions().mode() & 0o777, 0o600);
    s.child.kill().unwrap();
    s.child.wait().unwrap();
    s.child = Command::new(env!("CARGO_BIN_EXE_rusty-memoryd")).arg("--home").arg(&s.root).spawn().unwrap();
    let start = Instant::now();
    loop {
        if rpc(
            &s.root.join("memory/advisor.sock"),
            &Request {
                op: "status".into(),
                scope: "repo-a".into(),
                session: "session".into(),
                seq: 1,
                mode: Mode::On,
                data: Value::Null,
            },
        )
        .is_ok()
        {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(3));
        std::thread::sleep(Duration::from_millis(10));
    }
    let status = s.call("status", "repo-a", 1, Value::Null).data;
    assert_eq!(status["policy_updates"], 1);
    assert!(status["l1"].is_null(), "short-term observations survived restart");
    s.call("forget", "repo-a", 1, json!({"id":id}));
    assert!(!s.call("import", "repo-a", 1, snapshot).error);
    assert!(s.call("list", "repo-a", 1, Value::Null).text.is_empty(), "deleted lesson resurrected");
}

#[test]
fn stalled_service_does_not_hold_coding_hook() {
    let root = std::env::temp_dir().join(format!("rm-stall-{}", std::process::id()));
    std::fs::create_dir_all(root.join("memory")).unwrap();
    let listener = UnixListener::bind(root.join("memory/advisor.sock")).unwrap();
    let mut hooks = Hooks::connect(&root, &root, Mode::On).unwrap();
    let start = Instant::now();
    assert!(hooks.advice().is_empty());
    assert!(start.elapsed() < Duration::from_millis(150), "hook exceeded deadline tolerance");
    assert_eq!(hooks.metrics.timeouts, 1);
    drop(hooks);
    drop(listener);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn response_style_survives_tools_but_not_the_turn_boundary() {
    let s = Service::new();
    let mut h = Hooks::connect(&s.root, &s.root, Mode::On).unwrap();
    let r = h.control("remember", json!({"kind":"preference","text":"Response style: concise, warm, three bullets."}));
    assert!(!r.error);
    h.begin("fix a parser");
    assert!(h.advice().contains("concise, warm, three bullets"));
    for n in 0..6 {
        h.event("bash", "cargo test", &format!("exit code: 0\ncheck {n}"));
        assert!(h.advice().contains("concise, warm, three bullets"));
    }
    assert_eq!(h.metrics.injections, 1, "cached guidance is not a new intervention");
    h.finish(true);
    assert!(h.advice().is_empty());
    h.begin("explain the parser");
    assert!(h.advice().contains("concise, warm, three bullets"));
    assert_eq!(h.metrics.injections, 2, "the new turn independently selects the preference");
    assert!(!h.control("forget", json!({"id":r.id})).error);
    assert!(h.advice().is_empty(), "forgotten guidance must leave the current prompt immediately");
}

#[test]
fn management_transport_honors_its_one_second_budget() {
    use std::io::{BufRead, BufReader, Write};
    let root = PathBuf::from("/tmp").join(format!(
        "rm-control-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&root).unwrap();
    let socket = root.join("advisor.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        BufReader::new(&stream).read_line(&mut request).unwrap();
        assert_eq!(serde_json::from_str::<Request>(&request).unwrap().op, "status");
        std::thread::sleep(Duration::from_millis(350));
        // The old 200 ms transport closes before this legitimate control reply.
        let _ = writeln!(
            stream,
            "{}",
            serde_json::to_string(&Response { text: "ready".into(), ..Response::default() }).unwrap()
        );
    });
    let response = rpc(
        &socket,
        &Request {
            op: "status".into(),
            scope: "test".into(),
            session: "test".into(),
            seq: 0,
            mode: Mode::On,
            data: Value::Null,
        },
    );
    server.join().unwrap();
    std::fs::remove_dir_all(root).unwrap();
    assert_eq!(response.unwrap().text, "ready");
}

#[test]
fn export_larger_than_hook_frame_roundtrips_and_invalid_import_rolls_back() {
    let s = Service::new();
    for i in 0..40 {
        assert!(!s.call("remember", "a", 0, json!({"text":format!("{i} {}","x".repeat(1500))})).error);
    }
    let mut snapshot = s.call("export", "a", 0, Value::Null).data;
    assert!(serde_json::to_vec(&snapshot).unwrap().len() > 32768);
    assert!(!s.call("import", "a", 0, snapshot.clone()).error);
    snapshot["lessons"][0]["id"] = json!("0000000000000000");
    assert!(s.call("import", "a", 0, snapshot).error);
    assert_eq!(s.call("status", "a", 0, Value::Null).data["lessons"], 40);
}

#[test]
fn direct_ingress_redacts_secrets_and_l1_never_reaches_disk() {
    let s = Service::new();
    let saved = s.call(
        "remember",
        "privacy",
        0,
        json!({"text":"DB_PASSWORD=secret-canary-12345","evidence":"Authorization: Bearer evidence-canary-12345"}),
    );
    assert!(!saved.error);
    let exported = s.call("export", "privacy", 0, Value::Null).data.to_string();
    assert!(exported.contains("[redacted]"));
    assert!(!exported.contains("secret-canary") && !exported.contains("evidence-canary"));
    s.call("begin", "privacy", 1, json!({"text":"temporary-request-canary"}));
    s.call(
        "event",
        "privacy",
        2,
        json!({"tool":"bash","args":"temporary-args-canary","output":"temporary-output-canary"}),
    );
    let db = rusqlite::Connection::open(s.root.join("memory/memory.sqlite")).unwrap();
    let traces: i64 =
        db.query_row("SELECT count(*) FROM sqlite_master WHERE name IN ('boards','events')", [], |r| r.get(0)).unwrap();
    assert_eq!(traces, 0);
    drop(db);
    for entry in std::fs::read_dir(s.root.join("memory")).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
            let bytes = std::fs::read(path).unwrap();
            for canary in
                ["secret-canary-12345", "temporary-request-canary", "temporary-args-canary", "temporary-output-canary"]
            {
                assert!(!bytes.windows(canary.len()).any(|w| w == canary.as_bytes()));
            }
        }
    }
    assert!(!s.call("finish", "privacy", 3, json!({"complete":true})).error);
    assert_eq!(s.call("status", "privacy", 3, Value::Null).data["l1"]["observations"], 0);
}

#[test]
fn daemon_refuses_symlinked_persistence() {
    use std::os::unix::fs::symlink;
    let s = Service::new();
    let root = s.root.join("symlink-test");
    std::fs::create_dir(&root).unwrap();
    symlink(s.root.join("memory"), root.join("memory")).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_rusty-memoryd")).arg("--home").arg(root).output().unwrap();
    assert!(!out.status.success());
}

#[test]
fn terminal_interrupt_does_not_kill_autostarted_memory() {
    use std::io::Write;
    use std::os::unix::process::CommandExt;
    let root =
        PathBuf::from("/tmp").join(format!("rm-auto-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir_all(&root).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_rusty"))
        .args(["--memory", "on", "--mode", "vibe"])
        .current_dir(&root)
        .env("RUSTY_HOME", &root)
        .env("HOME", &root)
        .env("RUSTY_NO_DOTENV", "1")
        .env("RUSTY_INFRA", "off")
        .env("NVIDIA_API_KEY", "offline-test-key")
        .env_remove("RUSTY_API_KEY")
        .env("PATH", "/usr/bin:/bin")
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut s = Service { root, child };
    s.ready();
    assert!(!s.call("status", "test", 0, Value::Null).error);
    unsafe {
        assert_eq!(libc::kill(-(s.child.id() as i32), libc::SIGINT), 0);
    }
    std::thread::sleep(Duration::from_millis(30));
    assert!(!s.call("status", "test", 0, Value::Null).error, "Ctrl-C killed the memory service");
    writeln!(s.child.stdin.take().unwrap(), "/exit").unwrap();
    s.child.wait().unwrap();
    let stopped =
        Command::new(env!("CARGO_BIN_EXE_rusty-memoryd")).arg("--home").arg(&s.root).arg("stop").output().unwrap();
    assert!(stopped.status.success(), "{}", String::from_utf8_lossy(&stopped.stderr));
}

#[test]
fn failed_database_initialization_never_publishes_ready_socket() {
    let root = PathBuf::from("/tmp").join(format!(
        "rm-invalid-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(root.join("memory")).unwrap();
    std::fs::write(root.join("memory/memory.sqlite"), b"invalid SQLite fixture").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_rusty-memoryd")).arg("--home").arg(&root).output().unwrap();
    assert!(!result.status.success());
    assert!(!root.join("memory/advisor.sock").exists(), "failed initialization published a readiness socket");
    std::fs::remove_dir_all(root).unwrap();
}

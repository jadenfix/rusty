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
        while !self.root.join("memory/advisor.sock").exists() {
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
fn daemon_restarts_preserves_scoped_board_feedback_and_deletions() {
    let mut s = Service::new();
    for _ in 0..30 {
        drop(std::os::unix::net::UnixStream::connect(s.root.join("memory/advisor.sock")).unwrap());
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
    assert_eq!(status["l1"]["seq"], 1);
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

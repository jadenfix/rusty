//! Optional local advisor. Hooks have a deadline; learning never executes tools.
//! L1 is per session, L2 is scoped, and only explicit feedback trains the selector.
use anyhow::{bail, Context, Result};
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const FRAME: usize = 32_768;
const TRANSFER: usize = 4 * 1024 * 1024;
const LESSONS: usize = 512;
const SESSIONS: usize = 32;
const HOOK_MS: u64 = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Off,
    Legacy,
    On,
    Deep,
}
impl Mode {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "off" => Some(Self::Off),
            "legacy" => Some(Self::Legacy),
            "on" => Some(Self::On),
            "deep" => Some(Self::Deep),
            _ => None,
        }
    }
}

pub fn clip(s: &str, bytes: usize) -> String {
    let mut n = bytes.min(s.len());
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    s[..n].to_owned()
}
fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}
fn hash(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

/// Same repository across worktrees; explicit identity also works across cloud clones.
pub fn scope(cwd: &Path) -> String {
    if let Ok(id) = std::env::var("RUSTY_PROJECT_ID") {
        if !id.trim().is_empty() {
            return format!("project:{:016x}", hash(id.trim()));
        }
    }
    let out = Command::new("git").args(["rev-parse", "--git-common-dir"]).current_dir(cwd).output();
    let root = out
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            let p = String::from_utf8(o.stdout).ok()?;
            cwd.join(p.trim()).canonicalize().ok()
        })
        .unwrap_or_else(|| cwd.to_owned());
    format!("project:{:016x}", hash(&root.to_string_lossy()))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub op: String,
    pub scope: String,
    pub session: String,
    pub seq: u64,
    pub mode: Mode,
    #[serde(default)]
    pub data: Value,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Response {
    pub text: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub seq: u64,
    #[serde(default)]
    pub error: bool,
    #[serde(default)]
    pub data: Value,
}

pub fn rpc(socket: &Path, req: &Request) -> Result<Response> {
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(Duration::from_millis(200)))?;
    stream.set_write_timeout(Some(Duration::from_millis(200)))?;
    let mut body = serde_json::to_vec(req)?;
    if body.len() > if req.op == "import" { TRANSFER } else { FRAME } {
        bail!("memory frame too large");
    }
    body.push(b'\n');
    stream.write_all(&body)?;
    let mut text = String::new();
    BufReader::new(stream.take(TRANSFER as u64)).read_line(&mut text)?;
    Ok(serde_json::from_str(&text)?)
}

#[derive(Default, Serialize)]
pub struct Metrics {
    pub hooks: u64,
    pub timeouts: u64,
    pub injections: u64,
    pub hook_us: u64,
    pub max_hook_us: u64,
}
type Work = (Request, mpsc::Sender<Result<Response>>);
pub struct Hooks {
    tx: mpsc::SyncSender<Work>,
    pub mode: Mode,
    scope: String,
    session: String,
    seq: u64,
    pub metrics: Metrics,
}
impl Hooks {
    pub fn connect(home: &Path, cwd: &Path, mode: Mode) -> Result<Self> {
        let socket = home.join("memory/advisor.sock");
        if socket.as_os_str().len() > 100 {
            bail!("RUSTY_HOME is too long for a local memory socket");
        }
        if UnixStream::connect(&socket).is_err() {
            let bin = std::env::current_exe()?.with_file_name("rusty-memoryd");
            let mut child = Command::new(bin)
                .arg("--home")
                .arg(home)
                .process_group(0)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .context("install rusty-memoryd alongside rusty")?;
            let start = Instant::now();
            loop {
                if UnixStream::connect(&socket).is_ok() {
                    break;
                }
                if child.try_wait()?.is_some() || start.elapsed() > Duration::from_secs(2) {
                    bail!("memory daemon did not start");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let (tx, rx) = mpsc::sync_channel::<Work>(8);
        std::thread::spawn(move || {
            for (req, reply) in rx {
                let _ = reply.send(rpc(&socket, &req));
            }
        });
        Ok(Self {
            tx,
            mode,
            scope: scope(cwd),
            session: format!("{}-{}", std::process::id(), now()),
            seq: 0,
            metrics: Metrics::default(),
        })
    }
    fn call(&mut self, op: &str, data: Value, ms: u64) -> Option<Response> {
        let start = Instant::now();
        let req = Request {
            op: op.into(),
            scope: self.scope.clone(),
            session: self.session.clone(),
            seq: self.seq,
            mode: self.mode,
            data,
        };
        let (tx, rx) = mpsc::channel();
        let result = self
            .tx
            .try_send((req, tx))
            .ok()
            .and_then(|_| rx.recv_timeout(Duration::from_millis(ms)).ok())
            .and_then(Result::ok);
        self.metrics.hooks += 1;
        let us = start.elapsed().as_micros() as u64;
        self.metrics.hook_us += us;
        self.metrics.max_hook_us = self.metrics.max_hook_us.max(us);
        if result.is_none() {
            self.metrics.timeouts += 1;
        }
        result
    }
    pub fn begin(&mut self, request: &str) {
        self.seq += 1;
        self.call("begin", json!({"text":clip(request,4096)}), HOOK_MS);
    }
    pub fn event(&mut self, tool: &str, args: &str, output: &str) {
        self.seq += 1;
        self.call("event", json!({"tool":tool,"args":clip(args,1024),"output":clip(output,2048)}), HOOK_MS);
    }
    pub fn finish(&mut self, complete: bool) {
        self.seq += 1;
        self.call("finish", json!({"complete":complete}), HOOK_MS);
    }
    pub fn advice(&mut self) -> String {
        let Some(r) = self.call("advice", Value::Null, HOOK_MS) else { return String::new() };
        if r.error || r.seq != self.seq || r.text.is_empty() {
            return String::new();
        }
        self.metrics.injections += 1;
        format!(
            "\nMemory advisor (optional guidance; current instructions and tool permissions take precedence):\n{}\n",
            clip(&r.text, if self.mode == Mode::Deep { 1600 } else { 800 })
        )
    }
    pub fn late_advice(&mut self) -> String {
        if self.mode != Mode::Deep {
            return String::new();
        }
        let Some(r) = self.call("prepared", Value::Null, HOOK_MS) else { return String::new() };
        if r.error || r.seq != self.seq || r.text.is_empty() {
            return String::new();
        }
        self.metrics.injections += 1;
        format!(
            "Memory advisor: {}. This is optional guidance; current instructions and permissions take precedence.",
            clip(&r.text, 1600)
        )
    }
    pub fn control(&mut self, op: &str, data: Value) -> Response {
        self.call(op, data, 1000).unwrap_or_else(|| Response {
            text: "memory service unavailable; no operation confirmed".into(),
            error: true,
            ..Response::default()
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Observation {
    id: u64,
    tool: String,
    args: String,
    output: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Board {
    seq: u64,
    request: String,
    observations: Vec<Observation>,
    #[serde(default)]
    complete: bool,
    #[serde(default)]
    teacher_jobs: u8,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Lesson {
    id: String,
    scope: String,
    kind: String,
    text: String,
    evidence: String,
    helpful: u64,
    harmful: u64,
}
#[derive(Clone, Serialize, Deserialize)]
struct Policy {
    weights: [f64; 4],
    updates: u64,
}
impl Default for Policy {
    fn default() -> Self {
        Self { weights: [-2.0, 4.0, 1.0, 1.5], updates: 0 }
    }
}
impl Policy {
    fn score(&self, x: [f64; 4]) -> f64 {
        1.0 / (1.0 + (-self.weights.iter().zip(x).map(|(w, x)| w * x).sum::<f64>()).exp())
    }
    fn update(&mut self, x: [f64; 4], helpful: bool) {
        let err = if helpful { 1.0 } else { 0.0 } - self.score(x);
        for (w, x) in self.weights.iter_mut().zip(x) {
            *w = (*w + 0.1 * err * x).clamp(-8.0, 8.0);
        }
        self.updates += 1;
    }
}
fn words(s: &str) -> HashSet<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|s| s.len() > 2)
        .filter(|s| !["the", "and", "for", "this", "that", "please", "with", "from"].contains(s))
        .map(str::to_owned)
        .collect()
}
fn features(query: &str, lesson: &Lesson) -> [f64; 4] {
    let q = words(query);
    let t = words(&lesson.text);
    features_terms(&q, lesson, &t)
}
fn features_terms(q: &HashSet<String>, lesson: &Lesson, t: &HashSet<String>) -> [f64; 4] {
    let overlap = q.intersection(t).count() as f64 / q.len().min(t.len()).max(1) as f64;
    [
        1.0,
        overlap,
        (lesson.helpful as f64 + 1.0) / (lesson.helpful + lesson.harmful + 2) as f64,
        if lesson.kind == "preference" { 1.0 } else { 0.0 },
    ]
}
fn pack(s: &str) -> Result<Vec<u8>> {
    // Tiny notes would grow under gzip. A zero tag denotes bounded raw UTF-8;
    // existing gzip blobs keep their original header and remain readable.
    let raw = || {
        let mut b = Vec::with_capacity(s.len() + 1);
        b.push(0);
        b.extend_from_slice(s.as_bytes());
        b
    };
    if s.len() < 32 {
        return Ok(raw());
    }
    let mut w = GzEncoder::new(Vec::new(), Compression::fast());
    w.write_all(s.as_bytes())?;
    let packed = w.finish()?;
    Ok(if packed.len() < s.len() + 1 { packed } else { raw() })
}
fn unpack(b: &[u8]) -> Result<String> {
    if b.first() == Some(&0) {
        if b.len() > FRAME + 1 {
            bail!("raw memory exceeds limit");
        }
        return Ok(std::str::from_utf8(&b[1..])?.to_owned());
    }
    let mut s = String::new();
    GzDecoder::new(b).take(FRAME as u64 + 1).read_to_string(&mut s)?;
    if s.len() > FRAME {
        bail!("decompressed memory exceeds limit");
    }
    Ok(s)
}

// SQLite's dynamic type permits a backwards-compatible upgrade from plain
// TEXT to gzip BLOBs without a second table or an in-memory storage engine.
fn stored_text(row: &rusqlite::Row<'_>, column: usize) -> rusqlite::Result<String> {
    match row.get::<_, rusqlite::types::Value>(column)? {
        rusqlite::types::Value::Text(s) => Ok(s),
        rusqlite::types::Value::Blob(b) => unpack(&b)
            .map_err(|e| rusqlite::Error::FromSqlConversionFailure(column, rusqlite::types::Type::Blob, e.into())),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

struct Store {
    db: Connection,
}
impl Store {
    fn open(home: &Path) -> Result<Self> {
        let dir = home.join("memory");
        crate::privacy::private_dir(&dir)?;
        for name in ["memory.sqlite", "memory.sqlite-wal", "memory.sqlite-shm"] {
            let path = dir.join(name);
            if path.exists() || path.is_symlink() {
                let f = std::fs::OpenOptions::new().write(true).custom_flags(libc::O_NOFOLLOW).open(path)?;
                f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            }
        }
        drop(
            std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(dir.join("memory.sqlite"))?,
        );
        let db = Connection::open(dir.join("memory.sqlite"))?;
        db.execute_batch("PRAGMA secure_delete=ON; PRAGMA temp_store=MEMORY; PRAGMA cache_size=-512; PRAGMA mmap_size=0;
          PRAGMA journal_mode=WAL; PRAGMA busy_timeout=1000; PRAGMA journal_size_limit=65536; PRAGMA wal_autocheckpoint=16; PRAGMA max_page_count=4096;
          CREATE TABLE IF NOT EXISTS lessons(id TEXT PRIMARY KEY,scope TEXT NOT NULL,kind TEXT NOT NULL,text TEXT NOT NULL,evidence BLOB NOT NULL,helpful INTEGER DEFAULT 0,harmful INTEGER DEFAULT 0);
          CREATE TABLE IF NOT EXISTS policies(scope TEXT PRIMARY KEY,payload TEXT);
          CREATE TABLE IF NOT EXISTS deleted(id TEXT PRIMARY KEY,scope TEXT NOT NULL);")?;
        // Migrate the first version's duplicated L1 journals. L1 now stays in
        // RAM, so tool arguments/output never enter the advisor database.
        let old: bool =
            db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name IN ('events','boards'))", [], |r| {
                r.get(0)
            })?;
        if old {
            db.execute_batch(
                "DROP TABLE IF EXISTS events; DROP TABLE IF EXISTS boards; VACUUM; PRAGMA wal_checkpoint(TRUNCATE);",
            )?;
        }
        let mut store = Self { db };
        // Upgrade lessons saved through older direct CLI paths, tombstoning
        // unsafe old identities when redaction changes canonical text.
        let rows = {
            let mut q =
                store.db.prepare("SELECT id,scope,kind,text,evidence,helpful,harmful FROM lessons LIMIT 513")?;
            let rows = q.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    stored_text(r, 3)?,
                    r.get::<_, Vec<u8>>(4)?,
                    r.get::<_, u64>(5)?,
                    r.get::<_, u64>(6)?,
                    r.get_ref(3)?.data_type() == rusqlite::types::Type::Text,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        if rows.len() > LESSONS {
            bail!("existing store exceeds lesson limit");
        }
        let mut changed = false;
        for (id, scope, kind, text, evidence, helpful, harmful, plain) in rows {
            let evidence = unpack(&evidence)?;
            let clean_text = crate::privacy::redact(&text).0;
            let clean_evidence = crate::privacy::redact(&evidence).0;
            if !plain && text == clean_text && evidence == clean_evidence {
                continue;
            }
            let new_id = format!("{:016x}", hash(&format!("{scope}:{kind}:{clean_text}")));
            let tx = store.db.transaction()?;
            tx.execute("DELETE FROM lessons WHERE id=?1", [&id])?;
            if new_id != id {
                tx.execute("INSERT OR IGNORE INTO deleted VALUES(?1,?2)", params![id, scope])?;
            }
            tx.execute(
                "INSERT OR IGNORE INTO lessons VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![new_id, scope, kind, pack(&clean_text)?, pack(&clean_evidence)?, helpful, harmful],
            )?;
            tx.commit()?;
            changed = true;
        }
        if changed {
            store.db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        }
        Ok(store)
    }
    fn lessons(&self, scope: &str) -> Result<Vec<Lesson>> {
        let mut q =
            self.db.prepare("SELECT id,scope,kind,text,evidence,helpful,harmful FROM lessons WHERE scope=?1")?;
        let rows = q.query_map([scope], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                stored_text(r, 3)?,
                r.get::<_, Vec<u8>>(4)?,
                r.get::<_, u64>(5)?,
                r.get::<_, u64>(6)?,
            ))
        })?;
        rows.map(|r| {
            let (id, scope, kind, text, e, helpful, harmful) = r?;
            Ok(Lesson { id, scope, kind, text, evidence: unpack(&e)?, helpful, harmful })
        })
        .collect()
    }
    fn save_lesson(&mut self, l: &Lesson) -> Result<()> {
        let tx = self.db.transaction()?;
        if tx.query_row("SELECT EXISTS(SELECT 1 FROM deleted WHERE id=?1)", [&l.id], |r| r.get::<_, bool>(0))? {
            bail!("this lesson was forgotten; save revised text instead");
        }
        let n: i64 = tx.query_row("SELECT count(*) FROM lessons", [], |r| r.get(0))?;
        if n >= LESSONS as i64
            && !tx.query_row("SELECT EXISTS(SELECT 1 FROM lessons WHERE id=?1)", [&l.id], |r| r.get::<_, bool>(0))?
        {
            bail!("memory capacity reached; forget unused lessons first");
        }
        tx.execute("INSERT INTO lessons(id,scope,kind,text,evidence,helpful,harmful) SELECT ?1,?2,?3,?4,?5,?6,?7 WHERE NOT EXISTS(SELECT 1 FROM deleted WHERE id=?1) ON CONFLICT(id) DO NOTHING",params![l.id,l.scope,l.kind,pack(&l.text)?,pack(&l.evidence)?,l.helpful,l.harmful])?;
        tx.commit()?;
        Ok(())
    }
    fn policy(&self, scope: &str) -> Policy {
        self.db
            .query_row("SELECT payload FROM policies WHERE scope=?1", [scope], |r| r.get::<_, String>(0))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
}

#[derive(Clone)]
struct Prepared {
    seq: u64,
    text: String,
}
struct Job {
    key: String,
    board: Board,
}
#[derive(Default, Serialize)]
struct TeacherMetrics {
    requests: u64,
    prompt_tokens: u64,
    completion_tokens: u64,
    call_us: u64,
    hints: u64,
}

fn teacher(board: &Board, metrics: &Arc<Mutex<TeacherMetrics>>, client: &reqwest::blocking::Client) -> Option<String> {
    let key = std::env::var("RUSTY_API_KEY").or_else(|_| std::env::var("NVIDIA_API_KEY")).ok()?;
    let base = std::env::var("RUSTY_BASE_URL").unwrap_or_else(|_| "https://integrate.api.nvidia.com/v1".into());
    let model = std::env::var("RUSTY_MEMORY_MODEL")
        .or_else(|_| std::env::var("RUSTY_MODEL"))
        .unwrap_or_else(|_| "nvidia/nemotron-3-super-120b-a12b".into());
    let prompt="You advise a coding agent using only the supplied request and observed evidence. Choose one specific useful NEXT step or remain silent. Do not invent preferences, repositories, credentials or permissions. Return JSON only: {\"next_step\":\"...\",\"evidence_ids\":[1]}. Each ID must reference an observation that supports the step. If no grounded intervention is needed return {\"next_step\":\"\",\"evidence_ids\":[]}. Do not repeat a successful action or prescribe external mutations.";
    let mut body = json!({"model":model,"stream":false,"temperature":0.0,"max_tokens":2048,"messages":[{"role":"system","content":prompt},{"role":"user","content":serde_json::to_string(board).ok()?}]});
    if base.trim_end_matches('/') == "https://integrate.api.nvidia.com/v1"
        && model == "nvidia/nemotron-3-super-120b-a12b"
    {
        body["reasoning_effort"] = json!("low");
        body["reasoning_budget"] = json!(512);
    }
    metrics.lock().unwrap().requests += 1;
    let response = client
        .post(format!("{}/chat/completions", base.trim_end_matches('/')))
        .bearer_auth(key)
        .json(&body)
        .send()
        .ok()?
        .error_for_status()
        .ok()?;
    let mut text = String::new();
    response.take(65_537).read_to_string(&mut text).ok()?;
    if text.len() > 65_536 {
        return None;
    }
    let v: Value = serde_json::from_str(&text).ok()?;
    {
        let mut m = metrics.lock().unwrap();
        m.prompt_tokens += v["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
        m.completion_tokens += v["usage"]["completion_tokens"].as_u64().unwrap_or(0);
    }
    let s = v["choices"][0]["message"]["content"].as_str()?;
    let v: Value = serde_json::from_str(
        s.trim().trim_start_matches("```json").trim_start_matches("```").trim_end_matches("```").trim(),
    )
    .ok()?;
    let text = v["next_step"].as_str()?;
    let ids = v["evidence_ids"].as_array()?;
    if text.is_empty()
        || ids.is_empty()
        || ids.iter().any(|id| !board.observations.iter().any(|o| Some(o.id) == id.as_u64()))
    {
        return None;
    }
    Some(format!(
        "Suggested next step (background analysis; verify applicability): {} [events: {}]",
        clip(&crate::privacy::redact(text).0, 1200),
        ids.iter().filter_map(Value::as_u64).map(|n| n.to_string()).collect::<Vec<_>>().join(",")
    ))
}

struct Engine {
    store: Store,
    boards: HashMap<String, Board>,
    used: HashMap<String, (String, [f64; 4])>,
    delivered: HashMap<String, HashSet<String>>,
    cache: HashMap<String, Vec<(Lesson, HashSet<String>)>>,
    prepared: Arc<Mutex<HashMap<String, Prepared>>>,
    jobs: mpsc::SyncSender<Job>,
    teacher_metrics: Arc<Mutex<TeacherMetrics>>,
    versions: Arc<Mutex<HashMap<String, u64>>>,
}
impl Engine {
    fn new(home: &Path) -> Result<Self> {
        let prepared: Arc<Mutex<HashMap<String, Prepared>>> = Arc::default();
        let ready = prepared.clone();
        let versions: Arc<Mutex<HashMap<String, u64>>> = Arc::default();
        let current = versions.clone();
        let teacher_metrics: Arc<Mutex<TeacherMetrics>> = Arc::default();
        let measurements = teacher_metrics.clone();
        let (tx, rx) = mpsc::sync_channel::<Job>(4);
        std::thread::spawn(move || {
            let mut client = None;
            for j in rx {
                if current.lock().unwrap().get(&j.key) != Some(&j.board.seq) {
                    continue;
                }
                // The on/off paths never initialize TLS or a provider client.
                if client.is_none() {
                    client = reqwest::blocking::Client::builder()
                        .connect_timeout(Duration::from_secs(5))
                        .timeout(Duration::from_secs(20))
                        .build()
                        .ok();
                }
                let start = Instant::now();
                let answer = client.as_ref().and_then(|client| teacher(&j.board, &measurements, client));
                {
                    let mut m = measurements.lock().unwrap();
                    m.call_us += start.elapsed().as_micros() as u64;
                    m.hints += u64::from(answer.is_some());
                }
                if let Some(text) = answer {
                    if current.lock().unwrap().get(&j.key) != Some(&j.board.seq) {
                        continue;
                    }
                    let mut ready = ready.lock().unwrap();
                    if ready.len() >= SESSIONS {
                        ready.clear();
                    }
                    ready.insert(j.key, Prepared { seq: j.board.seq, text });
                }
            }
        });
        let store = Store::open(home)?;
        Ok(Self {
            store,
            boards: HashMap::new(),
            used: HashMap::new(),
            delivered: HashMap::new(),
            cache: HashMap::new(),
            prepared,
            jobs: tx,
            teacher_metrics,
            versions,
        })
    }
    fn handle(&mut self, mut r: Request) -> Result<Response> {
        if r.op != "import" && serde_json::to_vec(&r)?.len() > FRAME {
            bail!("memory frame too large");
        }
        if r.scope.len() > 128 || r.session.len() > 128 || r.scope.is_empty() || r.session.is_empty() {
            bail!("invalid memory identity");
        }
        let key = format!("{}:{}", r.scope, r.session);
        let mut reply = Response { seq: r.seq, ..Response::default() };
        if r.mode == Mode::Off {
            reply.text = "memory is off".into();
            return Ok(reply);
        }
        // Every ingress path uses the same filter, including direct CLI/RPC
        // callers. Imported canonical IDs must not be silently rewritten.
        if r.op != "import" {
            crate::privacy::scrub(&mut r.data);
        }
        match r.op.as_str() {
            "begin" | "event" | "finish" => {
                if self.boards.len() >= SESSIONS && !self.boards.contains_key(&key) {
                    if let Some(k) = self.boards.keys().next().cloned() {
                        self.boards.remove(&k);
                        self.used.remove(&k);
                        self.delivered.remove(&k);
                        self.versions.lock().unwrap().remove(&k);
                        self.prepared.lock().unwrap().remove(&k);
                    }
                }
                let board = self.boards.entry(key.clone()).or_default();
                if r.seq <= board.seq {
                    bail!("stale memory event");
                }
                board.seq = r.seq;
                if r.op == "begin" {
                    board.request = clip(r.data["text"].as_str().unwrap_or(""), 4096);
                    board.observations.clear();
                    board.complete = false;
                    board.teacher_jobs = 0;
                    self.used.remove(&key);
                    self.delivered.remove(&key);
                    self.prepared.lock().unwrap().remove(&key);
                }
                if r.op == "event" {
                    board.observations.push(Observation {
                        id: r.seq,
                        tool: clip(r.data["tool"].as_str().unwrap_or(""), 64),
                        args: clip(r.data["args"].as_str().unwrap_or(""), 1024),
                        output: clip(r.data["output"].as_str().unwrap_or(""), 2048),
                    });
                    if board.observations.len() > 8 {
                        board.observations.remove(0);
                    }
                }
                if r.op == "finish" {
                    board.complete = r.data["complete"].as_bool().unwrap_or(false);
                }
                if r.op == "finish" {
                    self.versions.lock().unwrap().remove(&key);
                    self.prepared.lock().unwrap().remove(&key);
                    // Keep only sequence/completion so stale calls still fail.
                    board.request.clear();
                    board.observations.clear();
                } else {
                    self.versions.lock().unwrap().insert(key.clone(), r.seq);
                }
                if r.mode == Mode::Deep
                    && r.op == "event"
                    && !board.complete
                    && board.teacher_jobs < 4
                    && (r.seq.is_multiple_of(4)
                        || r.data["output"].as_str().is_some_and(|s| {
                            s.starts_with("error:") || (s.starts_with("exit code:") && !s.starts_with("exit code: 0\n"))
                        }))
                    && self.jobs.try_send(Job { key, board: board.clone() }).is_ok()
                {
                    board.teacher_jobs += 1;
                }
            }
            "advice" | "prepared" => {
                let Some(b) = self.boards.get(&key) else { return Ok(reply) };
                if b.seq != r.seq || b.complete {
                    return Ok(reply);
                };
                if self.delivered.get(&key).is_some_and(|d| d.len() >= if r.mode == Mode::Deep { 4 } else { 2 }) {
                    return Ok(reply);
                }
                if let Some(p) = self.prepared.lock().unwrap().remove(&key).filter(|p| p.seq == r.seq) {
                    reply.text = p.text;
                    self.delivered.entry(key).or_default().insert(format!("deep:{}", r.seq));
                    return Ok(reply);
                }
                if r.op == "prepared" {
                    return Ok(reply);
                }
                let query = format!(
                    "{} {}",
                    b.request,
                    b.observations.last().map(|o| format!("{} {} {}", o.tool, o.args, o.output)).unwrap_or_default()
                );
                let policy = self.store.policy(&r.scope);
                let query_terms = words(&query);
                let mut best: Option<(f64, Lesson, [f64; 4])> = None;
                if !self.cache.contains_key(&r.scope) {
                    if self.cache.len() >= SESSIONS {
                        self.cache.clear();
                    }
                    let lessons = self
                        .store
                        .lessons(&r.scope)?
                        .into_iter()
                        .map(|mut l| {
                            let terms = words(&l.text);
                            l.evidence = clip(&l.evidence, 160);
                            (l, terms)
                        })
                        .collect();
                    self.cache.insert(r.scope.clone(), lessons);
                }
                for (l, terms) in &self.cache[&r.scope] {
                    if self.delivered.get(&key).is_some_and(|d| d.contains(&l.id)) {
                        continue;
                    }
                    let f = features_terms(&query_terms, l, terms);
                    if f[1] < 0.25 && l.kind != "preference" {
                        continue;
                    }
                    let p = policy.score(f);
                    if p >= 0.6 && best.as_ref().is_none_or(|(old, _, _)| p > *old) {
                        best = Some((p, l.clone(), f));
                    }
                }
                if let Some((_, l, f)) = best {
                    reply.text = format!(
                        "Apply this prior lesson if its prerequisites still hold: {} [memory:{}; {}]",
                        l.text,
                        l.id,
                        clip(&l.evidence, 160)
                    );
                    reply.id = l.id.clone();
                    self.delivered.entry(key.clone()).or_default().insert(l.id.clone());
                    self.used.insert(key, (l.id, f));
                }
            }
            "remember" => {
                let text = clip(r.data["text"].as_str().unwrap_or("").trim(), 2048);
                if text.is_empty() {
                    bail!("empty lesson");
                }
                if r.data["global"].as_bool() == Some(true) {
                    bail!("advisor memory is project scoped; global memory is available in legacy mode");
                }
                let kind = r.data["kind"].as_str().unwrap_or("fact");
                if !["preference", "fact", "decision", "gotcha", "todo"].contains(&kind) {
                    bail!("unknown memory kind");
                }
                let id = format!("{:016x}", hash(&format!("{}:{kind}:{text}", r.scope)));
                let evidence =
                    clip(r.data["evidence"].as_str().unwrap_or("explicitly saved; not independently verified"), 2048);
                self.store.save_lesson(&Lesson {
                    id: id.clone(),
                    scope: r.scope.clone(),
                    kind: kind.into(),
                    text,
                    evidence,
                    helpful: 0,
                    harmful: 0,
                })?;
                self.cache.remove(&r.scope);
                reply.text = format!("saved project memory {id}");
                reply.id = id;
            }
            "recall" | "list" => {
                let query = r.data["query"].as_str().unwrap_or("");
                let mut ls = self.store.lessons(&r.scope)?;
                if r.op == "recall" {
                    ls.retain(|l| features(query, l)[1] > 0.0);
                    ls.sort_by(|a, b| features(query, b)[1].total_cmp(&features(query, a)[1]));
                }
                reply.text = clip(
                    &ls.iter()
                        .take(10)
                        .map(|l| format!("[{}:{}] {}", l.kind, l.id, l.text))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    4096,
                );
            }
            "forget" => {
                let id = r.data["id"].as_str().unwrap_or("");
                let tx = self.store.db.transaction()?;
                let n = tx.execute("DELETE FROM lessons WHERE id=?1 AND scope=?2", params![id, r.scope])?;
                if n > 0 {
                    tx.execute("INSERT OR IGNORE INTO deleted VALUES(?1,?2)", params![id, r.scope])?;
                    reply.text = format!("forgot {id}");
                } else {
                    reply.text = "no memory in this scope".into();
                }
                tx.commit()?;
                self.store.db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
                self.prepared.lock().unwrap().clear();
                self.used.retain(|_, (last, _)| last != id);
                self.cache.remove(&r.scope);
            }
            "feedback" => {
                let Some((id, x)) = self.used.remove(&key) else {
                    bail!("no delivered L2 intervention to rate");
                };
                let helpful = r.data["helpful"].as_bool().context("helpful must be boolean")?;
                self.store.db.execute(
                    if helpful {
                        "UPDATE lessons SET helpful=helpful+1 WHERE id=?1 AND scope=?2"
                    } else {
                        "UPDATE lessons SET harmful=harmful+1 WHERE id=?1 AND scope=?2"
                    },
                    params![id, r.scope],
                )?;
                let mut p = self.store.policy(&r.scope);
                p.update(x, helpful);
                self.store.db.execute(
                    "INSERT INTO policies VALUES(?1,?2) ON CONFLICT(scope) DO UPDATE SET payload=excluded.payload",
                    params![r.scope, serde_json::to_string(&p)?],
                )?;
                reply.text = "feedback recorded; outcome attribution is explicit".into();
                self.cache.remove(&r.scope);
            }
            "export" => {
                let deleted = self
                    .store
                    .db
                    .prepare("SELECT id FROM deleted WHERE scope=?1")?
                    .query_map([&r.scope], |r| r.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                reply.data = json!({"version":1,"scope":r.scope,"lessons":self.store.lessons(&r.scope)?,"deleted":deleted,"policy":self.store.policy(&r.scope)});
            }
            "import" => {
                if r.data["version"] != 1 || r.data["scope"] != r.scope {
                    bail!("memory export version or scope mismatch");
                }
                let lessons: Vec<Lesson> = serde_json::from_value(r.data["lessons"].clone())?;
                let deleted: Vec<String> = serde_json::from_value(r.data.get("deleted").cloned().unwrap_or(json!([])))?;
                if lessons.len() > LESSONS {
                    bail!("import too large");
                }
                for l in &lessons {
                    if l.scope != r.scope
                        || l.text.is_empty()
                        || l.text.len() > 2048
                        || l.evidence.len() > 2048
                        || crate::privacy::redact(&l.text).0 != l.text
                        || crate::privacy::redact(&l.evidence).0 != l.evidence
                        || !["preference", "fact", "decision", "gotcha", "todo"].contains(&l.kind.as_str())
                        || l.id != format!("{:016x}", hash(&format!("{}:{}:{}", l.scope, l.kind, l.text)))
                    {
                        bail!("invalid imported lesson");
                    }
                }
                if deleted.len() > 65536
                    || deleted.iter().any(|s| s.len() != 16 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
                {
                    bail!("invalid imported deletion");
                }
                let tx = self.store.db.transaction()?;
                for id in deleted {
                    tx.execute("DELETE FROM lessons WHERE id=?1 AND scope=?2", params![id, r.scope])?;
                    tx.execute("INSERT OR IGNORE INTO deleted VALUES(?1,?2)", params![id, r.scope])?;
                }
                for l in lessons {
                    tx.execute("INSERT INTO lessons(id,scope,kind,text,evidence,helpful,harmful) SELECT ?1,?2,?3,?4,?5,0,0 WHERE NOT EXISTS(SELECT 1 FROM deleted WHERE id=?1) ON CONFLICT(id) DO NOTHING",params![l.id,l.scope,l.kind,pack(&l.text)?,pack(&l.evidence)?])?;
                }
                let n: i64 = tx.query_row("SELECT count(*) FROM lessons", [], |r| r.get(0))?;
                if n > LESSONS as i64 {
                    bail!("memory capacity exceeded; import rolled back");
                }
                tx.commit()?;
                self.cache.remove(&r.scope);
                self.prepared.lock().unwrap().clear();
                reply.text = "imported scoped lessons; local feedback policy preserved".into();
            }
            "shutdown" => {
                reply.text = "memory daemon stopped".into();
            }
            "status" => {
                let lessons: i64 =
                    self.store.db.query_row("SELECT count(*) FROM lessons WHERE scope=?1", [&r.scope], |r| r.get(0))?;
                reply.data = json!({"lessons":lessons,"sessions":self.boards.len(),"compressed_event_bytes":0,"l1_persistence":"ram-only","policy_updates":self.store.policy(&r.scope).updates,"background":*self.teacher_metrics.lock().unwrap(),"l1":self.boards.get(&key).map(|b| json!({"seq":b.seq,"observations":b.observations.len(),"complete":b.complete}))});
                reply.text = reply.data.to_string();
            }
            _ => bail!("unknown memory operation"),
        }
        Ok(reply)
    }
}

pub fn serve(home: &Path) -> Result<()> {
    let dir = home.join("memory");
    crate::privacy::private_dir(&dir)?;
    // A lock prevents a second daemon from unlinking an active socket.
    use std::os::fd::AsRawFd;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(dir.join("daemon.lock"))?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        bail!("memory daemon already running");
    }
    // A connectable socket must mean initialization is complete. Cold overlay
    // filesystems can take longer than the hook deadline to initialize SQLite.
    let mut engine = Engine::new(home)?;
    let path = dir.join("advisor.sock");
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else {
            continue;
        };
        if stream.set_read_timeout(Some(Duration::from_millis(50))).is_err()
            || stream.set_write_timeout(Some(Duration::from_millis(50))).is_err()
        {
            continue;
        }
        let mut s = String::new();
        if BufReader::new((&stream).take(TRANSFER as u64)).read_line(&mut s).is_err() || !s.ends_with('\n') {
            continue;
        }
        let shutdown = serde_json::from_str::<Request>(&s).is_ok_and(|r| r.op == "shutdown");
        let response = serde_json::from_str::<Request>(&s)
            .map_err(anyhow::Error::from)
            .and_then(|r| engine.handle(r))
            .unwrap_or_else(|e| Response { text: e.to_string(), error: true, ..Response::default() });
        if let Ok(mut body) = serde_json::to_vec(&response) {
            if body.len() > TRANSFER {
                body = serde_json::to_vec(&Response {
                    text: "memory response exceeds frame limit".into(),
                    error: true,
                    ..Response::default()
                })?;
            }
            body.push(b'\n');
            let _ = stream.write_all(&body);
        }
        if shutdown && !response.error {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    fn engine() -> (Engine, PathBuf) {
        let p = std::env::temp_dir().join(format!(
            "rusty-memory-unit-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        (Engine::new(&p).unwrap(), p)
    }
    fn req(op: &str, scope: &str, seq: u64, data: Value) -> Request {
        Request { op: op.into(), scope: scope.into(), session: "test".into(), seq, mode: Mode::On, data }
    }
    #[test]
    fn scope_staleness_learning_and_deletion() {
        let (mut e, p) = engine();
        let id = e
            .handle(req(
                "remember",
                "a",
                0,
                json!({"kind":"preference","text":"Always inspect the workspace manifest before editing packages"}),
            ))
            .unwrap()
            .id;
        e.handle(req("begin", "a", 1, json!({"text":"edit workspace packages"}))).unwrap();
        assert!(!e.handle(req("advice", "a", 1, Value::Null)).unwrap().text.is_empty());
        assert!(e.handle(req("advice", "b", 1, Value::Null)).unwrap().text.is_empty());
        assert!(e.handle(req("begin", "a", 1, json!({"text":"stale"}))).is_err());
        e.handle(req("feedback", "a", 1, json!({"helpful":false}))).unwrap();
        assert_eq!(e.store.policy("a").updates, 1);
        let exported = e.handle(req("export", "a", 1, Value::Null)).unwrap().data;
        e.handle(req("forget", "a", 1, json!({"id":id}))).unwrap();
        e.handle(req("import", "a", 1, exported)).unwrap();
        assert!(e.store.lessons("a").unwrap().is_empty());
        drop(e);
        std::fs::remove_dir_all(p).unwrap();
    }
    #[test]
    fn evidence_compression_unicode_and_bounded_notepad() {
        assert_eq!(clip("ééé", 3), "é");
        let text = "long repeated evidence ".repeat(100);
        let b = pack(&text).unwrap();
        assert!(b.len() < text.len() / 5);
        assert_eq!(unpack(&b).unwrap(), text);
        assert_eq!(pack("small").unwrap().len(), 6);
        assert_eq!(unpack(&pack("small").unwrap()).unwrap(), "small");
        let (mut e, p) = engine();
        e.handle(req("begin", "a", 1, json!({"text":"work"}))).unwrap();
        for n in 2..30 {
            e.handle(req("event", "a", n, json!({"tool":"bash","args":"x".repeat(5000),"output":"y".repeat(5000)})))
                .unwrap();
        }
        assert_eq!(e.boards["a:test"].observations.len(), 8);
        assert_eq!(e.boards["a:test"].observations[0].args.len(), 1024);
        drop(e);
        std::fs::remove_dir_all(p).unwrap();
    }
    #[test]
    fn fact_advice_and_background_advice_require_current_state() {
        let (mut e, p) = engine();
        e.handle(req(
            "remember",
            "a",
            0,
            json!({"kind":"fact","text":"monorepo URL is https://github.com/example/workspace"}),
        ))
        .unwrap();
        e.handle(req("begin", "a", 1, json!({"text":"search monorepo"}))).unwrap();
        assert!(!e.handle(req("advice", "a", 1, Value::Null)).unwrap().id.is_empty());
        e.prepared.lock().unwrap().insert("a:test".into(), Prepared { seq: 1, text: "old proposal".into() });
        e.handle(req("event", "a", 2, json!({"tool":"bash","output":"exit code: 0"}))).unwrap();
        assert!(e.handle(req("advice", "a", 2, Value::Null)).unwrap().text.is_empty());
        e.prepared.lock().unwrap().insert("a:test".into(), Prepared { seq: 2, text: "current proposal".into() });
        assert_eq!(e.handle(req("advice", "a", 2, Value::Null)).unwrap().text, "current proposal");
        e.handle(req("finish", "a", 3, json!({"complete":true}))).unwrap();
        assert!(e.handle(req("advice", "a", 3, Value::Null)).unwrap().text.is_empty());
        drop(e);
        std::fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn feedback_moves_probability_in_correct_direction() {
        let mut p = Policy::default();
        let x = [1.0, 0.8, 0.5, 0.0];
        let before = p.score(x);
        p.update(x, true);
        assert!(p.score(x) > before);
        p.update(x, false);
        assert!(p.score(x) < before + 0.02);
    }

    #[test]
    fn upgrade_removes_legacy_traces_and_sanitizes_saved_lessons() {
        let (e, p) = engine();
        let text = "DB_PASSWORD=legacy-secret-canary-12345";
        let id = format!("{:016x}", hash(&format!("a:fact:{text}")));
        e.store
            .db
            .execute(
                "INSERT INTO lessons VALUES(?1,'a','fact',?2,?3,0,0)",
                params![id, text, pack("Bearer legacy-evidence-canary-12345").unwrap()],
            )
            .unwrap();
        e.store.db.execute_batch("CREATE TABLE events(payload BLOB); CREATE TABLE boards(payload BLOB); INSERT INTO events VALUES('legacy-tool-output-canary');").unwrap();
        drop(e);
        let store = Store::open(&p).unwrap();
        let lessons = store.lessons("a").unwrap();
        assert_eq!(lessons.len(), 1);
        assert_ne!(lessons[0].id, id);
        assert!(!lessons[0].text.contains("legacy-secret"));
        assert!(!lessons[0].evidence.contains("legacy-evidence"));
        assert!(store
            .db
            .query_row("SELECT EXISTS(SELECT 1 FROM deleted WHERE id=?1)", [&id], |r| r.get::<_, bool>(0))
            .unwrap());
        let old: i64 = store
            .db
            .query_row("SELECT count(*) FROM sqlite_master WHERE name IN ('events','boards')", [], |r| r.get(0))
            .unwrap();
        assert_eq!(old, 0);
        drop(store);
        std::fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn compressed_input_cannot_expand_past_the_bound() {
        let bomb = pack(&"x".repeat(FRAME + 1)).unwrap();
        assert!(unpack(&bomb).is_err());
    }

    #[test]
    fn sustained_hooks_bound_ram_state_without_growing_disk() {
        let (mut e, p) = engine();
        e.handle(req("remember", "a", 0, json!({"kind":"preference","text":"Inspect the workspace manifest"})))
            .unwrap();
        let bytes =
            || std::fs::read_dir(p.join("memory")).unwrap().map(|f| f.unwrap().metadata().unwrap().len()).sum::<u64>();
        let before = bytes();
        for n in 0..120 {
            let mut r = req("begin", "a", 1, json!({"text":"workspace manifest"}));
            r.session = format!("session-{n}");
            e.handle(r.clone()).unwrap();
            r.op = "advice".into();
            e.handle(r.clone()).unwrap();
            r.op = "event".into();
            r.data = json!({"tool":"bash","args":"a".repeat(4000),"output":"b".repeat(4000)});
            for seq in 2..12 {
                r.seq = seq;
                e.handle(r.clone()).unwrap();
            }
        }
        assert_eq!(e.boards.len(), SESSIONS);
        assert!(e.used.len() <= SESSIONS && e.delivered.len() <= SESSIONS);
        assert!(e.versions.lock().unwrap().len() <= SESSIONS);
        assert!(e.boards.values().all(|b| b.observations.len() <= 8));
        assert_eq!(bytes(), before, "coding hooks wrote persistent data");
        drop(e);
        std::fs::remove_dir_all(p).unwrap();
    }
}

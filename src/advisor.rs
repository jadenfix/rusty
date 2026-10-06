//! Optional local advisor. Hooks have a deadline; learning never executes tools.
//! L1 is per session, L2 is scoped, and only explicit feedback trains the selector.
use anyhow::{bail, Context, Result};
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
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
const EVENTS: usize = 2048;
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
    let overlap = q.intersection(&t).count() as f64 / q.len().min(t.len()).max(1) as f64;
    [
        1.0,
        overlap,
        (lesson.helpful as f64 + 1.0) / (lesson.helpful + lesson.harmful + 2) as f64,
        if lesson.kind == "preference" { 1.0 } else { 0.0 },
    ]
}
fn pack(s: &str) -> Result<Vec<u8>> {
    let mut w = GzEncoder::new(Vec::new(), Compression::fast());
    w.write_all(s.as_bytes())?;
    Ok(w.finish()?)
}
fn unpack(b: &[u8]) -> Result<String> {
    let mut s = String::new();
    GzDecoder::new(b).take(FRAME as u64).read_to_string(&mut s)?;
    Ok(s)
}

struct Store {
    db: Connection,
}
impl Store {
    fn open(home: &Path) -> Result<Self> {
        let dir = home.join("memory");
        std::fs::create_dir_all(&dir)?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        let db = Connection::open(dir.join("memory.sqlite"))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=1000; PRAGMA journal_size_limit=1048576; PRAGMA wal_autocheckpoint=128; PRAGMA max_page_count=4096;
          CREATE TABLE IF NOT EXISTS lessons(id TEXT PRIMARY KEY,scope TEXT NOT NULL,kind TEXT NOT NULL,text TEXT NOT NULL,evidence BLOB NOT NULL,helpful INTEGER DEFAULT 0,harmful INTEGER DEFAULT 0);
          CREATE TABLE IF NOT EXISTS events(id INTEGER PRIMARY KEY,scope TEXT,session TEXT,payload BLOB);
          CREATE TABLE IF NOT EXISTS boards(scope TEXT,session TEXT,payload BLOB,PRIMARY KEY(scope,session));
          CREATE TABLE IF NOT EXISTS policies(scope TEXT PRIMARY KEY,payload TEXT);
          CREATE TABLE IF NOT EXISTS deleted(id TEXT PRIMARY KEY,scope TEXT NOT NULL);")?;
        Ok(Self { db })
    }
    fn lessons(&self, scope: &str) -> Result<Vec<Lesson>> {
        let mut q =
            self.db.prepare("SELECT id,scope,kind,text,evidence,helpful,harmful FROM lessons WHERE scope=?1")?;
        let rows = q.query_map([scope], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
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
        tx.execute("INSERT INTO lessons(id,scope,kind,text,evidence,helpful,harmful) SELECT ?1,?2,?3,?4,?5,?6,?7 WHERE NOT EXISTS(SELECT 1 FROM deleted WHERE id=?1) ON CONFLICT(id) DO NOTHING",params![l.id,l.scope,l.kind,l.text,pack(&l.evidence)?,l.helpful,l.harmful])?;
        tx.commit()?;
        Ok(())
    }
    fn event(&self, r: &Request, b: &Board) -> Result<()> {
        self.db.execute(
            "INSERT INTO events(scope,session,payload) VALUES(?1,?2,?3)",
            params![r.scope, r.session, pack(&serde_json::to_string(r)?)?],
        )?;
        self.db.execute(
            "DELETE FROM events WHERE id NOT IN(SELECT id FROM events ORDER BY id DESC LIMIT ?1)",
            [EVENTS as i64],
        )?;
        self.db.execute(
            "INSERT INTO boards VALUES(?1,?2,?3) ON CONFLICT(scope,session) DO UPDATE SET payload=excluded.payload",
            params![r.scope, r.session, pack(&serde_json::to_string(b)?)?],
        )?;
        self.db.execute(
            "DELETE FROM boards WHERE rowid NOT IN(SELECT rowid FROM boards ORDER BY rowid DESC LIMIT ?1)",
            [SESSIONS as i64],
        )?;
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

fn teacher(board: &Board, metrics: &Arc<Mutex<TeacherMetrics>>) -> Option<String> {
    let key = std::env::var("RUSTY_API_KEY").or_else(|_| std::env::var("NVIDIA_API_KEY")).ok()?;
    let base = std::env::var("RUSTY_BASE_URL").unwrap_or_else(|_| "https://integrate.api.nvidia.com/v1".into());
    let model = std::env::var("RUSTY_MEMORY_MODEL")
        .or_else(|_| std::env::var("RUSTY_MODEL"))
        .unwrap_or_else(|_| "nvidia/nemotron-3-super-120b-a12b".into());
    let client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(20)).build().ok()?;
    let prompt="You advise a coding agent using only the supplied request and observed evidence. Choose one specific useful NEXT step or remain silent. Do not invent preferences, repositories, credentials or permissions. Return JSON only: {\"next_step\":\"...\",\"evidence_ids\":[1]}. Each ID must reference an observation that supports the step. If no grounded intervention is needed return {\"next_step\":\"\",\"evidence_ids\":[]}. Do not repeat a successful action or prescribe external mutations.";
    let mut body = json!({"model":model,"stream":false,"temperature":0.0,"max_tokens":2048,"messages":[{"role":"system","content":prompt},{"role":"user","content":serde_json::to_string(board).ok()?}]});
    if base.trim_end_matches('/') == "https://integrate.api.nvidia.com/v1"
        && model == "nvidia/nemotron-3-super-120b-a12b"
    {
        body["reasoning_effort"] = json!("low");
        body["reasoning_budget"] = json!(512);
    }
    metrics.lock().unwrap().requests += 1;
    let v: Value = client
        .post(format!("{}/chat/completions", base.trim_end_matches('/')))
        .bearer_auth(key)
        .json(&body)
        .send()
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .ok()?;
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
        clip(text, 1200),
        ids.iter().filter_map(Value::as_u64).map(|n| n.to_string()).collect::<Vec<_>>().join(",")
    ))
}

struct Engine {
    store: Store,
    boards: HashMap<String, Board>,
    used: HashMap<String, (String, [f64; 4])>,
    delivered: HashMap<String, HashSet<String>>,
    cache: HashMap<String, Vec<Lesson>>,
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
            for j in rx {
                if current.lock().unwrap().get(&j.key) != Some(&j.board.seq) {
                    continue;
                }
                let start = Instant::now();
                let answer = teacher(&j.board, &measurements);
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
        let boards = {
            let mut q = store.db.prepare("SELECT scope,session,payload FROM boards")?;
            let rows =
                q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Vec<u8>>(2)?)))?;
            let mut boards = HashMap::new();
            for row in rows {
                let (scope, session, b) = row?;
                if let Ok(b) = serde_json::from_str::<Board>(&unpack(&b)?) {
                    boards.insert(format!("{scope}:{session}"), b);
                }
            }
            boards
        };
        Ok(Self {
            store,
            boards,
            used: HashMap::new(),
            delivered: HashMap::new(),
            cache: HashMap::new(),
            prepared,
            jobs: tx,
            teacher_metrics,
            versions,
        })
    }
    fn handle(&mut self, r: Request) -> Result<Response> {
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
        match r.op.as_str() {
            "begin" | "event" | "finish" => {
                if self.boards.len() >= SESSIONS && !self.boards.contains_key(&key) {
                    if let Some(k) = self.boards.keys().next().cloned() {
                        self.boards.remove(&k);
                        self.used.remove(&k);
                        self.delivered.remove(&k);
                        self.versions.lock().unwrap().remove(&k);
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
                self.store.event(&r, board)?;
                if r.op == "finish" {
                    self.versions.lock().unwrap().remove(&key);
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
                let mut best: Option<(f64, Lesson, [f64; 4])> = None;
                if !self.cache.contains_key(&r.scope) {
                    if self.cache.len() >= SESSIONS {
                        self.cache.clear();
                    }
                    self.cache.insert(r.scope.clone(), self.store.lessons(&r.scope)?);
                }
                for l in &self.cache[&r.scope] {
                    if self.delivered.get(&key).is_some_and(|d| d.contains(&l.id)) {
                        continue;
                    }
                    let f = features(&query, l);
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
                    tx.execute("INSERT INTO lessons(id,scope,kind,text,evidence,helpful,harmful) SELECT ?1,?2,?3,?4,?5,0,0 WHERE NOT EXISTS(SELECT 1 FROM deleted WHERE id=?1) ON CONFLICT(id) DO NOTHING",params![l.id,l.scope,l.kind,l.text,pack(&l.evidence)?])?;
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
            "status" => {
                let lessons = self.store.lessons(&r.scope)?.len();
                let bytes: i64 =
                    self.store.db.query_row("SELECT COALESCE(sum(length(payload)),0) FROM events", [], |r| r.get(0))?;
                reply.data = json!({"lessons":lessons,"sessions":self.boards.len(),"compressed_event_bytes":bytes,"policy_updates":self.store.policy(&r.scope).updates,"background":*self.teacher_metrics.lock().unwrap(),"l1":self.boards.get(&key).map(|b| json!({"seq":b.seq,"observations":b.observations.len(),"complete":b.complete}))});
                reply.text = reply.data.to_string();
            }
            _ => bail!("unknown memory operation"),
        }
        Ok(reply)
    }
}

pub fn serve(home: &Path) -> Result<()> {
    let dir = home.join("memory");
    std::fs::create_dir_all(&dir)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    // A lock prevents a second daemon from unlinking an active socket.
    use std::os::fd::AsRawFd;
    let lock = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join("daemon.lock"))?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        bail!("memory daemon already running");
    }
    let path = dir.join("advisor.sock");
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    let mut engine = Engine::new(home)?;
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
}

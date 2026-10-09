//! A minimal MCP client: stdio servers from `.mcp.json` (or `RUSTY_MCP_CONFIG`),
//! their tools offered to the model as `mcp__<server>__<tool>`.
//!
//! Newline-delimited JSON-RPC 2.0 over the server's stdin/stdout, per the MCP
//! stdio transport. Only tools are supported; resources and prompts are not.
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};

const PROTOCOL: &str = "2025-06-18";
const PREFIX: &str = "mcp__";

#[derive(Deserialize)]
struct Config {
    #[serde(rename = "mcpServers", default)]
    servers: BTreeMap<String, ServerConfig>,
}

#[derive(Deserialize)]
struct ServerConfig {
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

/// One tool, under the name the model sees.
pub struct Tool {
    pub exposed: String,
    name: String,
    description: String,
    schema: Value,
}

struct Server {
    name: String,
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    next_id: u64,
    tools: Vec<Tool>,
}

/// The connected servers. Dropping it stops them.
#[derive(Default)]
pub struct Mcp {
    servers: Vec<Server>,
}

impl Mcp {
    /// Connects every configured server. A server that fails to start is
    /// skipped with a warning, so one broken entry never blocks the session.
    pub fn load(cwd: &Path) -> Result<Self> {
        let path = match std::env::var_os("RUSTY_MCP_CONFIG") {
            Some(p) => p.into(),
            None => cwd.join(".mcp.json"),
        };
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let config: Config = serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        let mut servers = Vec::new();
        for (name, cfg) in config.servers {
            match Server::start(&name, &cfg, cwd) {
                Ok(s) => servers.push(s),
                Err(e) => eprintln!("mcp server `{name}` unavailable: {e:#}"),
            }
        }
        Ok(Self { servers })
    }

    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }

    /// `name (N tools)` for each server, for the banner.
    pub fn summary(&self) -> String {
        self.servers.iter().map(|s| format!("{} ({} tools)", s.name, s.tools.len())).collect::<Vec<_>>().join(", ")
    }

    pub fn is_tool(name: &str) -> bool {
        name.get(..PREFIX.len()).is_some_and(|p| p.eq_ignore_ascii_case(PREFIX))
    }

    /// The exposed tool a name means: itself, or the one tool it matches once
    /// separators are ignored, so `mcp__simcloud#get_resource` still works.
    pub fn resolve(&self, name: &str) -> Option<String> {
        let tools = || self.servers.iter().flat_map(|s| &s.tools);
        if tools().any(|t| t.exposed == name) {
            return Some(name.to_string());
        }
        let key = loose(name);
        let mut matches = tools().filter(|t| loose(&t.exposed) == key);
        let found = matches.next()?;
        matches.next().is_none().then(|| found.exposed.clone())
    }

    /// OpenAI-style function definitions for every tool.
    pub fn definitions(&self) -> Vec<Value> {
        self.servers
            .iter()
            .flat_map(|s| &s.tools)
            .map(|t| {
                json!({"type": "function", "function": {
                    "name": t.exposed, "description": t.description, "parameters": t.schema
                }})
            })
            .collect()
    }

    /// Calls a tool by its exposed name and returns its text content. A tool
    /// error comes back as text starting with `error:`, like other tools.
    pub fn call(&mut self, exposed: &str, args: &Value) -> Result<String> {
        let server = self
            .servers
            .iter_mut()
            .find(|s| s.tools.iter().any(|t| t.exposed == exposed))
            .ok_or_else(|| anyhow!("unknown MCP tool `{exposed}`"))?;
        let name = server.tools.iter().find(|t| t.exposed == exposed).map(|t| t.name.clone()).unwrap_or_default();
        let args = if args.is_object() { args.clone() } else { json!({}) };
        let result = server.request("tools/call", json!({"name": name, "arguments": args}), call_timeout())?;
        let text: Vec<&str> = result["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| if c["type"] == "text" { c["text"].as_str() } else { None })
            .collect();
        let text = if text.is_empty() { result["structuredContent"].to_string() } else { text.join("\n") };
        let text = crate::tools::cap(text);
        Ok(if result["isError"].as_bool().unwrap_or(false) { format!("error: {text}") } else { text })
    }
}

impl Server {
    fn start(name: &str, cfg: &ServerConfig, cwd: &Path) -> Result<Self> {
        let mut child = Command::new(&cfg.command)
            .args(&cfg.args)
            .envs(&cfg.env)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("starting `{}`", cfg.command))?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        // A reader thread, so a silent server can never block past a deadline.
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut server = Self { name: name.to_string(), child, stdin, lines, next_id: 1, tools: Vec::new() };
        let handshake = Duration::from_secs(30);
        server.request(
            "initialize",
            json!({"protocolVersion": PROTOCOL, "capabilities": {},
                   "clientInfo": {"name": "rusty", "version": env!("CARGO_PKG_VERSION")}}),
            handshake,
        )?;
        server.notify("notifications/initialized")?;
        let listed = server.request("tools/list", json!({}), handshake)?;
        server.tools = listed["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| {
                let tool = t["name"].as_str()?;
                let mut schema = t["inputSchema"].clone();
                if !schema.is_object() {
                    schema = json!({"type": "object", "properties": {}});
                }
                Some(Tool {
                    exposed: exposed_name(name, tool),
                    name: tool.to_string(),
                    description: format!("[{name} MCP] {}", t["description"].as_str().unwrap_or("")),
                    schema,
                })
            })
            .collect();
        Ok(server)
    }

    fn send(&mut self, message: &Value) -> Result<()> {
        writeln!(self.stdin, "{message}").and_then(|()| self.stdin.flush()).context("the MCP server closed its input")
    }

    fn notify(&mut self, method: &str) -> Result<()> {
        self.send(&json!({"jsonrpc": "2.0", "method": method}))
    }

    fn request(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = match self.lines.recv_timeout(left) {
                Ok(line) => line,
                Err(RecvTimeoutError::Timeout) => bail!("{method} timed out after {}s", timeout.as_secs()),
                Err(RecvTimeoutError::Disconnected) => bail!("the MCP server `{}` exited", self.name),
            };
            // Skip notifications, server requests and stray output.
            let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
            if msg["id"].as_u64() != Some(id) || msg.get("method").is_some() {
                continue;
            }
            if let Some(err) = msg.get("error") {
                bail!("{method}: {}", err["message"].as_str().unwrap_or("error"));
            }
            return Ok(msg["result"].clone());
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Lowercase letters and digits only.
fn loose(name: &str) -> String {
    name.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_lowercase()).collect()
}

fn call_timeout() -> Duration {
    let secs = std::env::var("RUSTY_MCP_TIMEOUT").ok().and_then(|v| v.parse().ok()).unwrap_or(300);
    Duration::from_secs(secs)
}

/// `mcp__server__tool`, limited to the characters and length tool names allow.
fn exposed_name(server: &str, tool: &str) -> String {
    let clean =
        |s: &str| s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect::<String>();
    let mut name = format!("{PREFIX}{}__{}", clean(server), clean(tool));
    name.truncate(64);
    name
}

/// The verb a tool name starts or ends with decides its permission class:
/// reads run freely, deletes need a person, everything else counts as a change.
pub fn classify(exposed: &str) -> crate::permissions::Class {
    use crate::permissions::Class;
    let tool = exposed.rsplit("__").next().unwrap_or(exposed).to_ascii_lowercase();
    let words: Vec<&str> = tool.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let has = |set: &[&str]| words.iter().any(|w| set.contains(w));
    if has(&["delete", "destroy", "purge", "drop", "remove", "revoke", "wipe", "truncate"]) {
        return Class::Destructive(format!("MCP tool {tool} deletes"));
    }
    const READS: &[&str] = &[
        "get",
        "list",
        "describe",
        "read",
        "search",
        "find",
        "status",
        "logs",
        "log",
        "metrics",
        "show",
        "whoami",
        "simulate",
        "trace",
        "audit",
        "incidents",
        "diff",
        "plan",
        "drift",
        "compare",
        "explain",
        "query",
        "inspect",
        "pending",
        "timeline",
        "history",
        "fetch",
        "view",
        "check",
        "health",
        "kinds",
        "access",
    ];
    if words.first().is_some_and(|w| READS.contains(w)) || words.last().is_some_and(|w| READS.contains(w)) {
        return Class::ReadOnly;
    }
    Class::Risky(format!("MCP tool {tool} may change something"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::Class;

    #[test]
    fn names_are_prefixed_and_safe() {
        assert_eq!(exposed_name("simcloud", "secret.access"), "mcp__simcloud__secret_access");
        assert!(exposed_name(&"s".repeat(40), &"t".repeat(40)).len() <= 64);
        assert!(Mcp::is_tool("mcp__a__b") && !Mcp::is_tool("bash"));
    }

    #[test]
    fn verbs_decide_the_permission_class() {
        assert_eq!(classify("mcp__simcloud__get"), Class::ReadOnly);
        assert_eq!(classify("mcp__simcloud__simulate_access"), Class::ReadOnly);
        assert_eq!(classify("mcp__simcloud__pending_iam_changes"), Class::ReadOnly);
        assert!(matches!(classify("mcp__simcloud__delete"), Class::Destructive(_)));
        assert!(matches!(classify("mcp__simcloud__kv_put"), Class::Risky(_)));
        assert!(matches!(classify("mcp__simcloud__deploy"), Class::Risky(_)));
    }
}

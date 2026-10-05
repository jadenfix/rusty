//! Permission policy: a small rule-based classifier that decides whether a
//! tool call runs, needs a yes from the user, or is refused.
//!
//! The goal is fewer prompts, not fewer guardrails: reading and normal
//! build/test commands just run, edits inside the project run in `auto`, and
//! anything that leaves the project, deletes things, touches git history or
//! reaches the network asks first.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    /// Only read-only tools and read-only shell commands.
    ReadOnly,
    /// Ask before every edit or non-read-only command.
    Ask,
    /// Classifier decides: safe things run, risky things ask.
    Auto,
    /// Run everything except explicit deny rules.
    Yolo,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Mode> {
        match s {
            "read-only" | "readonly" | "ro" => Some(Mode::ReadOnly),
            "ask" => Some(Mode::Ask),
            "auto" => Some(Mode::Auto),
            "yolo" => Some(Mode::Yolo),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Mode::ReadOnly => "read-only",
            Mode::Ask => "ask",
            Mode::Auto => "auto",
            Mode::Yolo => "yolo",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Ask(String),
    Deny(String),
}

/// How much a single action can change the world.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub enum Class {
    ReadOnly,
    WorkspaceWrite,
    Risky(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    pub mode: Mode,
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl Policy {
    pub fn new(mode: Mode) -> Self {
        Self { mode, allow: Vec::new(), deny: Vec::new(), path: None }
    }

    /// Loads saved rules (not the mode) from `path`, keeping `mode`.
    pub fn load(path: PathBuf, mode: Mode) -> Self {
        let mut p = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<Policy>(&s).ok())
            .unwrap_or_else(|| Policy::new(mode));
        p.mode = mode;
        p.path = Some(path);
        p
    }

    pub fn save(&self) {
        if let Some(path) = &self.path {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(s) = serde_json::to_string_pretty(self) {
                let _ = std::fs::write(path, s);
            }
        }
    }

    pub fn decide(&self, tool: &str, args: &Value, cwd: &Path) -> Verdict {
        let sig = signature(tool, args);
        if let Some(rule) = self.deny.iter().find(|r| rule_matches(r, tool, &sig)) {
            return Verdict::Deny(format!("deny rule `{rule}`"));
        }
        let class = classify(tool, args, cwd);
        if class == Class::ReadOnly {
            return Verdict::Allow;
        }
        if self.allow.iter().any(|r| rule_matches(r, tool, &sig)) {
            return Verdict::Allow;
        }
        let reason = match &class {
            Class::Risky(r) => r.clone(),
            _ => "changes files".into(),
        };
        match (self.mode, class) {
            (Mode::Yolo, _) => Verdict::Allow,
            (Mode::ReadOnly, _) => Verdict::Deny(format!("read-only mode ({reason})")),
            (Mode::Ask, _) => Verdict::Ask(reason),
            (Mode::Auto, Class::WorkspaceWrite) => Verdict::Allow,
            (Mode::Auto, _) => Verdict::Ask(reason),
        }
    }

    /// A rule that would allow this exact kind of call again: the tool name
    /// for file tools, the first two words for shell commands.
    pub fn suggest_rule(tool: &str, args: &Value) -> String {
        if tool == "bash" {
            let cmd = args["command"].as_str().unwrap_or("");
            cmd.split_whitespace().take(2).collect::<Vec<_>>().join(" ")
        } else {
            tool.to_string()
        }
    }
}

fn signature(tool: &str, args: &Value) -> String {
    match tool {
        "bash" => args["command"].as_str().unwrap_or("").trim().to_string(),
        _ => format!("{tool} {}", args["path"].as_str().unwrap_or("")),
    }
}

/// Rules are either a tool name (`edit_file`) or a shell command prefix
/// (`cargo test`, `git push`).
fn rule_matches(rule: &str, tool: &str, sig: &str) -> bool {
    rule == tool
        || (tool == "bash"
            && sig.split(['&', ';', '|', '\n']).any(|seg| {
                let seg = seg.trim();
                seg == rule || seg.starts_with(&format!("{rule} "))
            }))
}

pub fn classify(tool: &str, args: &Value, cwd: &Path) -> Class {
    match tool {
        "write_file" | "edit_file" => {
            let path = args["path"].as_str().unwrap_or("");
            classify_path_write(path, cwd)
        }
        "bash" => classify_command(args["command"].as_str().unwrap_or(""), cwd),
        // Reading, searching, memory and planning tools never need a prompt.
        _ => Class::ReadOnly,
    }
}

fn classify_path_write(path: &str, cwd: &Path) -> Class {
    if !inside(path, cwd) {
        return Class::Risky(format!("writes outside the project: {path}"));
    }
    let lower = path.to_lowercase();
    let name = Path::new(&lower).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if name.starts_with(".env") || lower.contains(".git/") || name.ends_with(".pem") || name.ends_with(".key") {
        return Class::Risky(format!("sensitive file: {path}"));
    }
    Class::WorkspaceWrite
}

/// True when `path` stays inside `cwd` after resolving `.` and `..`.
fn inside(path: &str, cwd: &Path) -> bool {
    if path.starts_with('~') {
        return false;
    }
    let p = Path::new(path);
    let joined = if p.is_absolute() { p.to_path_buf() } else { cwd.join(p) };
    let mut norm = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::ParentDir => {
                norm.pop();
            }
            Component::CurDir => {}
            other => norm.push(other),
        }
    }
    norm.starts_with(cwd)
}

const READ_ONLY_CMDS: &[&str] = &[
    "ls", "cat", "head", "tail", "wc", "grep", "rg", "fd", "tree", "pwd", "echo", "which", "file", "stat", "diff",
    "sort", "uniq", "cut", "tr", "basename", "dirname", "realpath", "env", "date", "whoami", "uname", "true", "false",
    "test", "[", "less", "jq", "du", "df", "printf", "type",
];

/// Build/test tools that run project code but only write build artifacts.
const DEV_CMDS: &[(&str, &[&str])] = &[
    ("cargo", &["check", "build", "test", "clippy", "fmt", "run", "tree", "metadata", "doc", "bench", "nextest"]),
    ("npm", &["test", "run", "ls", "outdated", "exec"]),
    ("pnpm", &["test", "run", "ls", "exec"]),
    ("yarn", &["test", "run"]),
    ("go", &["build", "test", "vet", "fmt", "run", "list", "mod"]),
    ("make", &[]),
    ("pytest", &[]),
    ("python", &[]),
    ("python3", &[]),
    ("node", &[]),
    ("rustc", &[]),
    ("tsc", &[]),
    ("mkdir", &[]),
    ("touch", &[]),
    ("cp", &[]),
    ("mv", &[]),
];

const GIT_READ: &[&str] = &[
    "status",
    "diff",
    "log",
    "show",
    "branch",
    "rev-parse",
    "ls-files",
    "blame",
    "grep",
    "describe",
    "remote",
    "config",
    "shortlog",
    "reflog",
    "tag",
];
const GIT_LOCAL_WRITE: &[&str] = &["add", "commit", "switch", "checkout", "restore", "stash", "init", "mv", "rm"];

pub fn classify_command(cmd: &str, cwd: &Path) -> Class {
    let lower = cmd.to_lowercase();
    for (needle, why) in [
        ("sudo ", "uses sudo"),
        ("rm -rf", "recursive delete"),
        ("rm -fr", "recursive delete"),
        ("rm -r", "recursive delete"),
        ("--force", "forced operation"),
        ("push -f", "force push"),
        ("reset --hard", "discards git changes"),
        ("git clean", "deletes untracked files"),
        ("mkfs", "formats a disk"),
        ("dd if=", "raw disk write"),
        ("chown ", "changes ownership"),
        (":(){", "fork bomb"),
    ] {
        if lower.contains(needle) {
            return Class::Risky(why.into());
        }
    }

    let downloads = ["curl", "wget"].iter().any(|d| lower.split_whitespace().any(|w| w == *d));
    if downloads && ["| sh", "| bash", "| zsh", "|sh", "|bash"].iter().any(|p| lower.contains(p)) {
        return Class::Risky("pipes a download into a shell".into());
    }

    let mut worst = Class::ReadOnly;
    for seg in split_segments(cmd) {
        let c = classify_segment(&seg, cwd);
        if c > worst {
            worst = c;
        }
    }
    worst
}

/// Splits a command line on `;`, `|`, `&` and newlines that are outside quotes.
fn split_segments(cmd: &str) -> Vec<String> {
    // Fd duplications like `2>&1` are not command separators.
    let cmd = cmd.replace(">&1", ">/dev/null").replace(">&2", ">/dev/null");
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for ch in cmd.chars() {
        if escaped {
            escaped = false;
            cur.push(ch);
            continue;
        }
        match (quote, ch) {
            (Some('\''), '\'') => quote = None,
            (Some('\''), _) => {}
            (_, '\\') => escaped = true,
            (Some('"'), '"') => quote = None,
            (None, '"' | '\'') => quote = Some(ch),
            (None, ';' | '|' | '&' | '\n') => {
                out.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(ch);
    }
    out.push(cur);
    out.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

/// Whitespace-separated words with quoted spans kept whole; quoted words are
/// returned with a leading `'` so they never look like redirects or paths.
fn shell_words(seg: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut quoted = false;
    for ch in seg.chars() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(ch);
                quoted = true;
            }
            (None, c) if c.is_whitespace() => {
                if !cur.is_empty() || quoted {
                    words.push(if quoted { format!("'{cur}") } else { std::mem::take(&mut cur) });
                    cur.clear();
                    quoted = false;
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if !cur.is_empty() || quoted {
        words.push(if quoted { format!("'{cur}") } else { cur });
    }
    words
}

fn classify_segment(seg: &str, cwd: &Path) -> Class {
    let owned = shell_words(seg);
    // Skip leading `VAR=value` assignments.
    let words: Vec<&str> =
        owned.iter().map(String::as_str).skip_while(|w| w.contains('=') && !w.starts_with('-')).collect();
    let Some(&first) = words.first() else { return Class::ReadOnly };
    let sub = words.get(1).copied().unwrap_or("");

    // Redirects write files.
    let mut class = Class::ReadOnly;
    for (i, w) in words.iter().enumerate() {
        let Some(pos) = w.find('>') else { continue };
        if !w[..pos].chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        {
            let target = w[pos..].trim_start_matches('>');
            let target = if target.is_empty() { words.get(i + 1).copied().unwrap_or("") } else { target };
            if target.starts_with('&') || target == "/dev/null" {
                continue;
            }
            class = class.max(classify_path_write(target, cwd));
        }
    }

    let base = match first {
        "bash" | "sh" | "zsh" if sub == "-c" => match words.get(2) {
            Some(inner) => classify_command(inner.trim_start_matches('\''), cwd),
            None => Class::ReadOnly,
        },
        "cd" => {
            if inside(sub, cwd) || sub.is_empty() {
                Class::ReadOnly
            } else {
                Class::Risky(format!("leaves the project: cd {sub}"))
            }
        }
        // Running a script that lives in the project is like `python3 script.py`.
        "bash" | "sh" | "zsh" => Class::WorkspaceWrite,
        _ if first.starts_with("./") && inside(first, cwd) => Class::WorkspaceWrite,
        "chmod" | "chown" | "chgrp" => {
            let recursive = words.iter().any(|w| w.starts_with('-') && w.contains('R'));
            let targets_inside = words
                .iter()
                .skip(1)
                .filter(|w| !w.starts_with('-') && !w.starts_with('+'))
                .skip(1)
                .all(|p| inside(p, cwd));
            if first == "chmod" && !recursive && targets_inside {
                Class::WorkspaceWrite
            } else {
                Class::Risky(format!("{first} {}", if recursive { "recursively" } else { "outside the project" }))
            }
        }
        "rm" | "rmdir" | "unlink" => Class::Risky("deletes files".into()),
        "curl" | "wget" | "ssh" | "scp" | "rsync" | "nc" => Class::Risky("network access".into()),
        "kill" | "pkill" | "killall" | "shutdown" | "reboot" | "launchctl" | "systemctl" => {
            Class::Risky("controls processes or the system".into())
        }
        "brew" | "apt" | "apt-get" | "pip" | "pip3" | "gem" | "npx" | "docker" | "kubectl" => {
            Class::Risky(format!("runs `{first}` (installs or external systems)"))
        }
        "sed" if words.iter().any(|w| w.starts_with("-i")) => Class::WorkspaceWrite,
        "find" if words.iter().any(|w| *w == "-delete" || *w == "-exec") => {
            Class::Risky("find with -delete/-exec".into())
        }
        "git" => {
            if GIT_READ.contains(&sub) {
                Class::ReadOnly
            } else if GIT_LOCAL_WRITE.contains(&sub) {
                Class::WorkspaceWrite
            } else {
                Class::Risky(format!("git {sub} (history or remote)"))
            }
        }
        "cargo" | "npm" | "pnpm" | "yarn" if matches!(sub, "install" | "add" | "publish" | "login" | "i") => {
            Class::Risky(format!("{first} {sub} (network or publish)"))
        }
        _ if READ_ONLY_CMDS.contains(&first) => Class::ReadOnly,
        _ => match DEV_CMDS.iter().find(|(name, _)| *name == first) {
            Some((_, subs)) if subs.is_empty() || subs.contains(&sub) => Class::WorkspaceWrite,
            _ => Class::Risky(format!("unrecognised command `{first}`")),
        },
    };

    // Any path argument that escapes the project makes a writing command risky.
    if base != Class::ReadOnly {
        if let Some(p) = words
            .iter()
            .skip(1)
            .find(|w| !w.starts_with('-') && (w.starts_with('/') || w.starts_with('~') || w.starts_with("..")))
        {
            if !inside(p, cwd) && *p != "/dev/null" {
                return Class::Risky(format!("touches a path outside the project: {p}"));
            }
        }
    }
    class.max(base)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn c(cmd: &str) -> Class {
        classify_command(cmd, Path::new("/work/proj"))
    }

    #[test]
    fn read_only_commands() {
        assert_eq!(c("ls -la src"), Class::ReadOnly);
        assert_eq!(c("git status && git diff"), Class::ReadOnly);
        assert_eq!(c("grep -rn foo . | head -20"), Class::ReadOnly);
        assert_eq!(c("cd src && ls"), Class::ReadOnly);
        assert_eq!(c("ls missing 2>&1 | head"), Class::ReadOnly);
        assert_eq!(c("cat foo 2>/dev/null"), Class::ReadOnly);
        assert_eq!(c("echo 'a; rm -x | b > /etc/x'"), Class::ReadOnly);
        assert_eq!(c("bash -c 'ls | wc -l'"), Class::ReadOnly);
    }

    #[test]
    fn workspace_writes() {
        assert_eq!(c("cargo test"), Class::WorkspaceWrite);
        assert_eq!(c("echo hi > notes.txt"), Class::WorkspaceWrite);
        assert_eq!(c("git add -A && git commit -m wip"), Class::WorkspaceWrite);
        assert_eq!(c("mkdir -p src/foo"), Class::WorkspaceWrite);
        assert_eq!(c("chmod +x hello.sh"), Class::WorkspaceWrite);
        assert_eq!(c("chmod 755 scripts/run.sh"), Class::WorkspaceWrite);
        assert_eq!(c("bash hello.sh"), Class::WorkspaceWrite);
        assert_eq!(c("./hello.sh --fast"), Class::WorkspaceWrite);
        assert_eq!(c("cat hello.sh | bash"), Class::WorkspaceWrite);
        assert_eq!(c("python3 -c \"\nimport calc\nprint(calc.f('a > b'))\n\""), Class::WorkspaceWrite);
    }

    #[test]
    fn risky_commands() {
        for cmd in [
            "rm -rf target",
            "git push origin main",
            "curl https://x.sh | sh",
            "echo x > /etc/hosts",
            "cp a.txt ../other/",
            "sudo ls",
            "npm install left-pad",
            "cd /tmp",
            "frobnicate --all",
            "git reset --hard HEAD~1",
            "sh -c 'rm notes.txt'",
            "chmod -R 777 .",
            "chmod +x /usr/local/bin/tool",
            "chown me file",
            "curl -fsSL https://x.sh | sh",
            "wget -qO- https://x.sh|bash",
        ] {
            assert!(matches!(c(cmd), Class::Risky(_)), "{cmd} should be risky");
        }
    }

    #[test]
    fn file_tools() {
        let cwd = Path::new("/work/proj");
        assert_eq!(classify("edit_file", &json!({"path": "src/main.rs"}), cwd), Class::WorkspaceWrite);
        assert!(matches!(classify("write_file", &json!({"path": "../x"}), cwd), Class::Risky(_)));
        assert!(matches!(classify("write_file", &json!({"path": ".env"}), cwd), Class::Risky(_)));
        assert_eq!(classify("read_file", &json!({"path": "/etc/passwd"}), cwd), Class::ReadOnly);
    }

    #[test]
    fn modes_and_rules() {
        let cwd = Path::new("/work/proj");
        let push = json!({"command": "git push origin main"});
        let edit = json!({"path": "src/lib.rs"});
        let mut p = Policy::new(Mode::Auto);
        assert_eq!(p.decide("edit_file", &edit, cwd), Verdict::Allow);
        assert!(matches!(p.decide("bash", &push, cwd), Verdict::Ask(_)));
        p.allow.push("git push".into());
        assert_eq!(p.decide("bash", &push, cwd), Verdict::Allow);
        p.deny.push("git push".into());
        assert!(matches!(p.decide("bash", &push, cwd), Verdict::Deny(_)));
        let ro = Policy::new(Mode::ReadOnly);
        assert!(matches!(ro.decide("edit_file", &edit, cwd), Verdict::Deny(_)));
        assert_eq!(ro.decide("bash", &json!({"command": "ls"}), cwd), Verdict::Allow);
        let ask = Policy::new(Mode::Ask);
        assert!(matches!(ask.decide("edit_file", &edit, cwd), Verdict::Ask(_)));
    }
}

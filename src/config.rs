//! Environment loading: API keys, base URL and defaults.

use std::path::PathBuf;

pub const DEFAULT_BASE_URL: &str = "https://integrate.api.nvidia.com/v1";
pub const DEFAULT_MODEL: &str = "nvidia/nemotron-3-super-120b-a12b";

/// Loads `.env` files without overriding variables already set in the shell.
/// Order: `~/.config/rusty/.env`, then the `.env` next to this crate's sources
/// (so `cargo install --path .` works with the repo's own `.env`).
pub fn load_env() {
    if std::env::var_os("RUSTY_NO_DOTENV").is_some() {
        return;
    }
    if let Some(dir) = config_dir() {
        let _ = dotenvy::from_path(dir.join(".env"));
    }
    let _ = dotenvy::from_path(concat!(env!("CARGO_MANIFEST_DIR"), "/.env"));
}

/// `RUSTY_HOME` overrides the default `~/.config/rusty` (handy for tests).
pub fn config_dir() -> Option<PathBuf> {
    if let Some(h) = std::env::var_os("RUSTY_HOME") {
        return Some(PathBuf::from(h));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config").join("rusty"))
}

/// All configured API keys, in rotation order.
pub fn api_keys() -> Vec<String> {
    let mut names = vec!["RUSTY_API_KEY".to_string(), "NVIDIA_API_KEY".to_string()];
    names.extend((2..=9).map(|i| format!("NVIDIA_API_KEY_{i}")));
    let mut keys: Vec<String> = Vec::new();
    for name in names {
        if let Ok(k) = std::env::var(&name) {
            let k = k.trim().to_string();
            if !k.is_empty() && !keys.contains(&k) {
                keys.push(k);
            }
        }
    }
    keys
}

pub fn base_url() -> String {
    std::env::var("RUSTY_BASE_URL")
        .or_else(|_| std::env::var("NVIDIA_API_BASE"))
        .unwrap_or_else(|_| DEFAULT_BASE_URL.to_string())
        .trim_end_matches('/')
        .to_string()
}

/// How the agent may delegate work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentsMode {
    /// No delegation.
    Off,
    /// One read-only subagent at a time (`task` tool).
    Sub,
    /// Many read-only workers in parallel (`swarm` tool).
    Swarm,
    /// Both available; the model picks the lightest option that fits.
    Auto,
}

impl AgentsMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "off" => Some(Self::Off),
            "sub" | "subagents" | "on" => Some(Self::Sub),
            "swarm" => Some(Self::Swarm),
            "auto" => Some(Self::Auto),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Sub => "sub",
            Self::Swarm => "swarm",
            Self::Auto => "auto",
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AgentsConfig {
    pub mode: AgentsMode,
    /// Model for subagents; defaults to the main model.
    pub sub_model: Option<String>,
    /// Models handed out round-robin to swarm workers; empty = sub_model.
    pub swarm_models: Vec<String>,
    /// Most workers one swarm call may start.
    pub swarm_max: usize,
    /// Temperature spread across workers (0 = all alike, 1 = very varied).
    pub spread: f32,
}

impl Default for AgentsConfig {
    fn default() -> Self {
        Self { mode: AgentsMode::Off, sub_model: None, swarm_models: Vec::new(), swarm_max: 8, spread: 0.4 }
    }
}

/// Preferences that persist across sessions in ~/.config/rusty/settings.json.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Tool location for future sessions; a bridge is required for daytona.
    pub tools: String,
    pub theme: String,
    pub font: String,
    pub view: String,
    pub tips: bool,
    pub agents: AgentsConfig,
    /// None means rusty picks per request.
    pub execution_mode: Option<crate::execution::ExecutionMode>,
    pub delegation_override: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            tools: "local".into(),
            theme: "rust".into(),
            font: "rust".into(),
            view: "default".into(),
            tips: true,
            agents: AgentsConfig::default(),
            execution_mode: None,
            delegation_override: false,
        }
    }
}

impl Settings {
    fn path() -> Option<PathBuf> {
        config_dir().map(|d| d.join("settings.json"))
    }

    pub fn load() -> Self {
        Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Some(p) = Self::path() {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(value) = serde_json::to_value(self) {
                let _ = rusty::privacy::write_json(&p, value);
            }
        }
    }
}

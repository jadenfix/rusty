//! Chat client with SSE streaming, tool calls and key rotation, for
//! OpenAI-compatible endpoints (NVIDIA, OpenAI, local servers) and Anthropic's
//! Messages API. Each request goes to the provider its model belongs to.

use anyhow::{anyhow, bail, Context, Result};
use reqwest::blocking::Response;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::Duration;

use crate::anthropic;
use crate::config::Provider;
use crate::execution::ExecutionMode;
use crate::ui::truncate;

/// Rounds of retries across every key before a request gives up.
const ROUNDS: usize = 6;

/// What a background request sends back to the UI thread.
pub enum Event {
    Reasoning(String),
    Content(String),
    Done(Result<Reply>),
}

pub enum Delta<'a> {
    Reasoning(&'a str),
    Content(&'a str),
}

#[derive(Default, Clone, Debug)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Default, Clone, Copy, Debug)]
pub struct Usage {
    pub prompt: u64,
    pub completion: u64,
}

#[derive(Default, Debug)]
pub struct Reply {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Option<Usage>,
    pub finish_reason: Option<String>,
    /// Anthropic's content blocks for this turn, kept so thinking goes back
    /// to the API unchanged. None for other providers.
    pub raw: Option<Value>,
}

/// One provider's endpoint and its keys.
pub struct Endpoint {
    pub provider: Provider,
    pub base_url: String,
    keys: Vec<String>,
    key_idx: AtomicUsize,
}

impl Endpoint {
    pub fn new(provider: Provider, base_url: String, keys: Vec<String>) -> Self {
        Self { provider, base_url: base_url.trim_end_matches('/').to_string(), keys, key_idx: AtomicUsize::new(0) }
    }
}

pub struct Client {
    http: reqwest::blocking::Client,
    endpoints: Vec<Endpoint>,
}

impl Client {
    /// Every provider with at least one key in the environment.
    pub fn from_env() -> Result<Self> {
        let endpoints = Provider::ALL
            .into_iter()
            .map(|p| Endpoint::new(p, p.base_url(), p.keys()))
            .filter(|e| !e.keys.is_empty())
            .collect();
        Self::new(endpoints)
    }

    pub fn new(endpoints: Vec<Endpoint>) -> Result<Self> {
        if endpoints.iter().all(|e| e.keys.is_empty()) {
            bail!(
                "no API key found. Set NVIDIA_API_KEY, ANTHROPIC_API_KEY or OPENAI_API_KEY in your shell, \
                 in ~/.config/rusty/.env, or in the .env next to rusty's Cargo.toml"
            );
        }
        let http = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(600))
            .build()?;
        Ok(Self { http, endpoints })
    }

    pub fn key_count(&self) -> usize {
        self.endpoints.iter().map(|e| e.keys.len()).sum()
    }

    pub fn endpoints(&self) -> &[Endpoint] {
        &self.endpoints
    }

    /// The endpoint that serves `model`. A model whose provider has no key
    /// goes to a custom compatible endpoint (`RUSTY_BASE_URL`, say a local
    /// router) when there is one, since that is where it was meant to go.
    pub fn endpoint_for(&self, model: &str) -> Result<&Endpoint> {
        let want = Provider::for_model(model);
        if let Some(e) = self.endpoints.iter().find(|e| e.provider == want) {
            return Ok(e);
        }
        if let Some(e) = self
            .endpoints
            .iter()
            .find(|e| e.provider == Provider::Compatible && e.base_url != crate::config::DEFAULT_BASE_URL)
        {
            return Ok(e);
        }
        let vars = want.key_vars();
        bail!(
            "`{model}` is served by {}, but {} is not set. Add it to your shell or ~/.config/rusty/.env, \
             or pick another model with /model",
            want.name(),
            vars.iter().find(|v| !v.starts_with("RUSTY_")).unwrap_or(&vars[0])
        )
    }

    /// Runs a chat request on a background thread and streams events back, so
    /// the caller can animate and react to Ctrl-C while waiting. Dropping the
    /// receiver stops the stream.
    pub fn start_chat(
        self: &Arc<Self>,
        model: String,
        messages: Vec<Value>,
        tools: Value,
        temperature: f32,
        execution_mode: ExecutionMode,
    ) -> Receiver<Event> {
        let (tx, rx) = channel();
        let me = self.clone();
        std::thread::spawn(move || {
            let result = me.chat(&model, &messages, &tools, temperature, execution_mode, &mut |d| {
                let ev = match d {
                    Delta::Reasoning(s) => Event::Reasoning(s.to_string()),
                    Delta::Content(s) => Event::Content(s.to_string()),
                };
                tx.send(ev).is_ok()
            });
            let _ = tx.send(Event::Done(result));
        });
        rx
    }

    /// Streams one chat completion, calling `on` for every text/reasoning
    /// delta; `on` returns false to stop early. An empty `tools` array means a
    /// plain completion.
    pub fn chat(
        &self,
        model: &str,
        messages: &[Value],
        tools: &Value,
        temperature: f32,
        execution_mode: ExecutionMode,
        on: &mut dyn FnMut(Delta) -> bool,
    ) -> Result<Reply> {
        let ep = self.endpoint_for(model)?;
        if ep.provider == Provider::Anthropic {
            let first_party = ep.base_url == crate::config::ANTHROPIC_BASE_URL;
            let (body, beta) = anthropic::request(model, messages, tools, execution_mode, first_party);
            let resp = self.send(ep, &format!("{}/v1/messages", ep.base_url), Some(&body), beta.as_deref())?;
            return anthropic::parse_stream(resp, on);
        }
        let mut body = json!({
            "model": model,
            "messages": anthropic::strip_private(messages),
            "stream": true,
            "stream_options": { "include_usage": true },
            "max_tokens": 16384,
            "temperature": temperature,
        });
        if ep.provider == Provider::OpenAi {
            openai_inference(&mut body, model, execution_mode);
        } else {
            execution_mode.apply_inference(&mut body, &ep.base_url, model);
        }
        if tools.as_array().is_some_and(|t| !t.is_empty()) {
            body["tools"] = tools.clone();
            body["tool_choice"] = json!("auto");
        }
        let resp = self.send(ep, &format!("{}/chat/completions", ep.base_url), Some(&body), None)?;
        parse_stream(resp, on)
    }

    /// Model ids from every configured provider. A provider that fails to
    /// answer is skipped unless none answer.
    pub fn list_models(&self) -> Result<Vec<String>> {
        let mut ids = Vec::new();
        let mut last_err = None;
        for ep in &self.endpoints {
            match self.models_of(ep, ROUNDS) {
                Ok(found) => ids.extend(found),
                Err(e) => last_err = Some(e),
            }
        }
        if ids.is_empty() {
            if let Some(e) = last_err {
                return Err(e);
            }
        }
        ids.sort();
        ids.dedup();
        Ok(ids)
    }

    /// One provider's model ids. `rounds` bounds the retries: the doctor
    /// asks once, so a bad key answers in a second instead of a minute.
    pub fn models_of(&self, ep: &Endpoint, rounds: usize) -> Result<Vec<String>> {
        let url = match ep.provider {
            Provider::Anthropic => format!("{}/v1/models?limit=1000", ep.base_url),
            _ => format!("{}/models", ep.base_url),
        };
        let v: Value = self.send_rounds(ep, &url, None, None, rounds)?.json().context("bad /models response")?;
        let data = v["data"].as_array().ok_or_else(|| anyhow!("unexpected /models response"))?;
        let mut ids: Vec<String> = data.iter().filter_map(|m| m["id"].as_str().map(String::from)).collect();
        ids.sort();
        Ok(ids)
    }

    /// Sends a request, rotating keys and backing off on 429 / 5xx / auth
    /// errors. Rounds wait 2, 4, 8, 16, then 30 seconds (or what the server
    /// asks for in Retry-After), which rides out about a minute of rate limits.
    fn send(&self, ep: &Endpoint, url: &str, body: Option<&Value>, beta: Option<&str>) -> Result<Response> {
        self.send_rounds(ep, url, body, beta, ROUNDS)
    }

    fn send_rounds(
        &self,
        ep: &Endpoint,
        url: &str,
        body: Option<&Value>,
        beta: Option<&str>,
        rounds: usize,
    ) -> Result<Response> {
        let keys = &ep.keys;
        let attempts = keys.len() * rounds.max(1);
        let mut last_err = String::new();
        let mut retry_after: Option<u64> = None;
        for attempt in 0..attempts {
            let idx = ep.key_idx.load(Ordering::Relaxed) % keys.len();
            let key = &keys[idx];
            let mut req = match body {
                Some(b) => self.http.post(url).json(b),
                None => self.http.get(url),
            };
            req = if ep.provider == Provider::Anthropic {
                req.header("x-api-key", key).header("anthropic-version", anthropic::VERSION)
            } else {
                req.bearer_auth(key)
            };
            if let Some(b) = beta {
                req = req.header("anthropic-beta", b);
            }
            match req.send() {
                Ok(r) if r.status().is_success() => return Ok(r),
                Ok(r) => {
                    let status = r.status();
                    if let Some(secs) = r
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.trim().parse::<u64>().ok())
                    {
                        retry_after = Some(retry_after.unwrap_or(0).max(secs.min(60)));
                    }
                    let text = r.text().unwrap_or_default();
                    last_err = format!("HTTP {status}: {}", truncate(text.trim(), 400));
                    let retryable = status.as_u16() == 429
                        || status.is_server_error()
                        || status.as_u16() == 401
                        || status.as_u16() == 403;
                    if !retryable {
                        bail!(last_err);
                    }
                }
                Err(e) => last_err = format!("request failed: {e}"),
            }
            ep.key_idx.store((idx + 1) % keys.len(), Ordering::Relaxed);
            // Back off once every key has been tried in this round.
            if (attempt + 1) % keys.len() == 0 && attempt + 1 < attempts {
                let round = (attempt + 1) / keys.len();
                let wait = retry_after.take().unwrap_or_else(|| 2u64.pow(round as u32).min(30));
                std::thread::sleep(Duration::from_secs(wait));
            }
        }
        bail!("giving up after {attempts} attempts: {last_err}")
    }
}

/// OpenAI's own API: reasoning models (gpt-5, o-series) take
/// `max_completion_tokens` and `reasoning_effort` and reject a temperature.
fn openai_inference(body: &mut Value, model: &str, mode: ExecutionMode) {
    let m = model.to_ascii_lowercase();
    let reasoning = m.starts_with("gpt-5")
        || m.starts_with("codex-")
        || (m.starts_with('o') && m[1..].starts_with(|c: char| c.is_ascii_digit()));
    if let Some(o) = body.as_object_mut() {
        let cap = o.remove("max_tokens").unwrap_or(json!(16384));
        o.insert(
            "max_completion_tokens".into(),
            json!(u64::from(mode.max_tokens()).max(cap.as_u64().unwrap_or(16384))),
        );
        if reasoning {
            o.remove("temperature");
            let effort = match mode {
                ExecutionMode::Careful => "high",
                ExecutionMode::Standard => "medium",
                ExecutionMode::Vibe => "low",
            };
            o.insert("reasoning_effort".into(), json!(effort));
        }
    }
}

fn parse_stream(resp: Response, on: &mut dyn FnMut(Delta) -> bool) -> Result<Reply> {
    let mut reply = Reply::default();
    let mut calls: Vec<ToolCall> = Vec::new();
    let mut saw_data = false;
    let mut raw = String::new();

    for line in BufReader::new(resp).lines() {
        let line = line.context("stream interrupted")?;
        let Some(data) = line.strip_prefix("data:") else {
            if !saw_data {
                raw.push_str(&line);
            }
            continue;
        };
        saw_data = true;
        let data = data.trim();
        if data == "[DONE]" {
            break;
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else { continue };
        if let Some(err) = v.get("error") {
            bail!("API error: {err}");
        }
        if let Some(u) = v.get("usage").filter(|u| !u.is_null()) {
            reply.usage = Some(usage_of(u));
        }
        let Some(choice) = v["choices"].get(0) else { continue };
        if let Some(fr) = choice["finish_reason"].as_str() {
            reply.finish_reason = Some(fr.to_string());
        }
        let delta = &choice["delta"];
        // Providers name this field differently; take the first one present.
        if let Some(s) = ["reasoning_content", "reasoning"].iter().find_map(|k| delta[*k].as_str()) {
            if !s.is_empty() && !on(Delta::Reasoning(s)) {
                reply.finish_reason = Some("interrupted".into());
                break;
            }
        }
        if let Some(s) = delta["content"].as_str() {
            if !s.is_empty() {
                reply.content.push_str(s);
                if !on(Delta::Content(s)) {
                    reply.finish_reason = Some("interrupted".into());
                    break;
                }
            }
        }
        if let Some(tcs) = delta["tool_calls"].as_array() {
            for tc in tcs {
                merge_tool_call(&mut calls, tc);
            }
        }
    }

    // Some servers ignore `stream: true` and answer with one JSON body.
    if !saw_data && !raw.trim().is_empty() {
        let v: Value =
            serde_json::from_str(&raw).with_context(|| format!("unexpected response: {}", truncate(&raw, 300)))?;
        let msg = &v["choices"][0]["message"];
        if let Some(s) = msg["content"].as_str() {
            reply.content = s.to_string();
            let _ = on(Delta::Content(s));
        }
        for tc in msg["tool_calls"].as_array().into_iter().flatten() {
            merge_tool_call(&mut calls, tc);
        }
        reply.finish_reason = v["choices"][0]["finish_reason"].as_str().map(String::from);
        reply.usage = v.get("usage").map(usage_of);
    }

    calls.retain(|c| !c.name.is_empty());
    for (i, c) in calls.iter_mut().enumerate() {
        if c.id.is_empty() {
            c.id = format!("call_{i}");
        }
    }
    reply.tool_calls = calls;
    Ok(reply)
}

fn usage_of(u: &Value) -> Usage {
    Usage { prompt: u["prompt_tokens"].as_u64().unwrap_or(0), completion: u["completion_tokens"].as_u64().unwrap_or(0) }
}

/// Folds one streamed tool-call fragment into the accumulated list.
fn merge_tool_call(calls: &mut Vec<ToolCall>, tc: &Value) {
    let idx = match tc["index"].as_u64() {
        Some(i) => i as usize,
        None => match tc["id"].as_str() {
            Some(id) => calls.iter().position(|c| c.id == id).unwrap_or(calls.len()),
            None => calls.len().saturating_sub(1),
        },
    };
    while calls.len() <= idx {
        calls.push(ToolCall::default());
    }
    let c = &mut calls[idx];
    if let Some(id) = tc["id"].as_str().filter(|s| !s.is_empty()) {
        c.id = id.to_string();
    }
    if let Some(name) = tc["function"]["name"].as_str().filter(|s| !s.is_empty()) {
        if c.name.is_empty() {
            c.name = name.to_string();
        }
    }
    match &tc["function"]["arguments"] {
        Value::String(s) => c.arguments.push_str(s),
        Value::Null => {}
        other => c.arguments.push_str(&other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn openai_reasoning_models_get_their_own_parameters() {
        let mut body = json!({"max_tokens": 16384, "temperature": 0.3});
        openai_inference(&mut body, "gpt-5", ExecutionMode::Careful);
        assert!(body.get("temperature").is_none() && body.get("max_tokens").is_none());
        assert_eq!(body["reasoning_effort"], "high");
        assert!(body["max_completion_tokens"].as_u64().unwrap() >= 16384);
        let mut body = json!({"max_tokens": 16384, "temperature": 0.3});
        openai_inference(&mut body, "gpt-4.1", ExecutionMode::Vibe);
        assert_eq!(body["temperature"], 0.3);
        assert!(body.get("reasoning_effort").is_none());
        let mut body = json!({"max_tokens": 16384});
        openai_inference(&mut body, "o4-mini", ExecutionMode::Vibe);
        assert_eq!(body["reasoning_effort"], "low");
    }

    #[test]
    fn routes_each_model_to_its_provider() {
        let ep = |p: Provider, url: &str| Endpoint::new(p, url.into(), vec!["k".into()]);
        let c = Client::new(vec![
            ep(Provider::Compatible, crate::config::DEFAULT_BASE_URL),
            ep(Provider::Anthropic, crate::config::ANTHROPIC_BASE_URL),
        ])
        .unwrap();
        assert_eq!(c.endpoint_for("claude-opus-5-5").unwrap().provider, Provider::Anthropic);
        assert_eq!(c.endpoint_for("openai/gpt-oss-20b").unwrap().provider, Provider::Compatible);
        let err = c.endpoint_for("gpt-5").err().unwrap().to_string();
        assert!(err.contains("OPENAI_API_KEY"), "{err}");
        // A custom compatible endpoint takes models whose own provider has no key.
        let c = Client::new(vec![ep(Provider::Compatible, "http://localhost:4000/v1")]).unwrap();
        assert_eq!(c.endpoint_for("claude-opus-5-5").unwrap().provider, Provider::Compatible);
        assert!(Client::new(vec![]).is_err());
    }

    #[test]
    fn merges_streamed_tool_call_fragments() {
        let mut calls = Vec::new();
        merge_tool_call(
            &mut calls,
            &json!({"index": 0, "id": "a", "function": {"name": "read_file", "arguments": "{\"pa"}}),
        );
        merge_tool_call(&mut calls, &json!({"index": 0, "function": {"arguments": "th\": \"x\"}"}}));
        merge_tool_call(
            &mut calls,
            &json!({"index": 1, "id": "b", "function": {"name": "bash", "arguments": {"command": "ls"}}}),
        );
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].arguments, "{\"path\": \"x\"}");
        assert_eq!(calls[1].name, "bash");
        assert!(calls[1].arguments.contains("ls"));
    }
}

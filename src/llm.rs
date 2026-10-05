//! OpenAI-compatible chat client with SSE streaming, tool calls and key rotation.

use anyhow::{anyhow, bail, Context, Result};
use reqwest::blocking::Response;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::Duration;

use crate::ui::truncate;

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
}

pub struct Client {
    http: reqwest::blocking::Client,
    base_url: String,
    keys: Vec<String>,
    key_idx: AtomicUsize,
}

impl Client {
    pub fn new(base_url: String, keys: Vec<String>) -> Result<Self> {
        if keys.is_empty() {
            bail!(
                "no API key found. Set NVIDIA_API_KEY in your shell, in ~/.config/rusty/.env, \
                 or in the .env next to rusty's Cargo.toml"
            );
        }
        let http = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(600))
            .build()?;
        Ok(Self { http, base_url, keys, key_idx: AtomicUsize::new(0) })
    }

    pub fn key_count(&self) -> usize {
        self.keys.len()
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
    ) -> Receiver<Event> {
        let (tx, rx) = channel();
        let me = self.clone();
        std::thread::spawn(move || {
            let result = me.chat(&model, &messages, &tools, temperature, &mut |d| {
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
        on: &mut dyn FnMut(Delta) -> bool,
    ) -> Result<Reply> {
        let mut body = json!({
            "model": model,
            "messages": messages,
            "stream": true,
            "stream_options": { "include_usage": true },
            "max_tokens": 16384,
            "temperature": temperature,
        });
        if tools.as_array().is_some_and(|t| !t.is_empty()) {
            body["tools"] = tools.clone();
            body["tool_choice"] = json!("auto");
        }
        let resp = self.send(&format!("{}/chat/completions", self.base_url), Some(&body))?;
        parse_stream(resp, on)
    }

    pub fn list_models(&self) -> Result<Vec<String>> {
        let resp = self.send(&format!("{}/models", self.base_url), None)?;
        let v: Value = resp.json().context("bad /models response")?;
        let mut ids: Vec<String> = v["data"]
            .as_array()
            .ok_or_else(|| anyhow!("unexpected /models response"))?
            .iter()
            .filter_map(|m| m["id"].as_str().map(String::from))
            .collect();
        ids.sort();
        Ok(ids)
    }

    /// Sends a request, rotating keys and backing off on 429 / 5xx / auth
    /// errors. Rounds wait 2, 4, 8, 16, then 30 seconds (or what the server
    /// asks for in Retry-After), which rides out about a minute of rate limits.
    fn send(&self, url: &str, body: Option<&Value>) -> Result<Response> {
        const ROUNDS: usize = 6;
        let attempts = self.keys.len() * ROUNDS;
        let mut last_err = String::new();
        let mut retry_after: Option<u64> = None;
        for attempt in 0..attempts {
            let idx = self.key_idx.load(Ordering::Relaxed) % self.keys.len();
            let key = &self.keys[idx];
            let req = match body {
                Some(b) => self.http.post(url).json(b),
                None => self.http.get(url),
            };
            match req.bearer_auth(key).send() {
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
            self.key_idx.store((idx + 1) % self.keys.len(), Ordering::Relaxed);
            // Back off once every key has been tried in this round.
            if (attempt + 1) % self.keys.len() == 0 && attempt + 1 < attempts {
                let round = (attempt + 1) / self.keys.len();
                let wait = retry_after.take().unwrap_or_else(|| 2u64.pow(round as u32).min(30));
                std::thread::sleep(Duration::from_secs(wait));
            }
        }
        bail!("giving up after {attempts} attempts: {last_err}")
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

//! Process-owned model admission ledger, shared by every call on the client.
//! Not a tokenizer, billing cap, whole-task deadline or durable restart journal.
use anyhow::{bail, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Limits {
    pub requests: u64,
    pub tokens: u64,
    pub seconds: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self { requests: 256, tokens: 4_000_000, seconds: 3600 }
    }
}
#[derive(Default, Serialize)]
struct State {
    requests: u64,
    charged_tokens: u64,
    known_tokens: u64,
    settled_requests: u64,
    active_requests: u64,
    denied_requests: u64,
    rate_limited: u64,
    overrun: bool,
    halted: bool,
    by_role: BTreeMap<String, u64>,
}
#[derive(Serialize)]
pub struct Snapshot {
    limits: Option<Limits>,
    enabled: bool,
    elapsed_seconds: f64,
    deadline_reached: bool,
    unknown_usage_requests: u64,
    #[serde(flatten)]
    state: State,
}
pub struct Budget {
    limits: Limits,
    enabled: bool,
    started: Instant,
    state: Mutex<State>,
}
impl Budget {
    pub fn new(limits: Limits) -> Arc<Self> {
        Arc::new(Self { limits, enabled: true, started: Instant::now(), state: Mutex::new(State::default()) })
    }
    /// Legacy sessions still collect diagnostic accounting without enforcement.
    /// A bounded run must opt in; independent deep-memory jobs remain separate.
    pub fn unbounded() -> Arc<Self> {
        Arc::new(Self {
            limits: Limits { requests: u64::MAX, tokens: u64::MAX, seconds: u64::MAX },
            enabled: false,
            started: Instant::now(),
            state: Mutex::new(State::default()),
        })
    }
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Provider admission/response time; shell tools retain their own deadlines.
    pub fn remaining(&self) -> Result<Duration> {
        let s = self.state.lock().unwrap();
        if s.halted {
            bail!("model budget exhausted: required review could not finish; this root is halted");
        }
        if s.overrun {
            bail!("model budget exhausted: reported usage exceeded the admission cap");
        }
        if !self.enabled {
            return Ok(Duration::from_secs(600));
        }
        let remaining = Duration::from_secs(self.limits.seconds).saturating_sub(self.started.elapsed());
        if remaining.is_zero() {
            bail!("model budget exhausted: provider deadline reached");
        }
        Ok(remaining)
    }
    /// Atomically reserve before each POST, including key rotations/retries.
    /// UTF-8 request bytes plus maximum output is conservative admission
    /// accounting; providers may tokenize differently or exceed their declared cap.
    pub fn reserve(self: &Arc<Self>, role: &str, tokens: u64) -> Result<Reservation> {
        let mut s = self.state.lock().unwrap();
        let deadline = self.started.elapsed() >= Duration::from_secs(self.limits.seconds);
        if s.halted
            || s.overrun
            || deadline
            || s.requests >= self.limits.requests
            || tokens > self.limits.tokens.saturating_sub(s.charged_tokens)
        {
            s.denied_requests += 1;
            let reason = if s.halted {
                "this root is halted after a required review budget failure".to_string()
            } else if s.overrun {
                "reported usage exceeded the token admission cap".to_string()
            } else if deadline {
                "provider deadline reached".to_string()
            } else if s.requests >= self.limits.requests {
                format!("{} HTTP attempts admitted (limit {})", s.requests, self.limits.requests)
            } else {
                format!(
                    "request needs {tokens} admission tokens, {} remain",
                    self.limits.tokens.saturating_sub(s.charged_tokens)
                )
            };
            bail!("model budget exhausted: {role}: {reason}");
        }
        s.requests += 1;
        s.charged_tokens += tokens;
        s.active_requests += 1;
        *s.by_role.entry(role.into()).or_default() += 1;
        Ok(Reservation { budget: self.clone(), tokens, role: role.into() })
    }
    /// Admissions refused so far; a rise across a call means it ran out.
    pub fn denied(&self) -> u64 {
        self.state.lock().unwrap().denied_requests
    }
    /// Required-review budget failure cannot be bypassed by resuming a goal.
    pub fn halt(&self) {
        self.state.lock().unwrap().halted = true;
    }

    pub fn snapshot(&self) -> Snapshot {
        let s = self.state.lock().unwrap();
        Snapshot {
            limits: self.enabled.then_some(self.limits),
            enabled: self.enabled,
            elapsed_seconds: self.started.elapsed().as_secs_f64(),
            deadline_reached: self.started.elapsed() >= Duration::from_secs(self.limits.seconds),
            // In-flight attempts have no complete usage yet either.
            unknown_usage_requests: s.requests - s.settled_requests,
            state: State {
                requests: s.requests,
                charged_tokens: s.charged_tokens,
                known_tokens: s.known_tokens,
                settled_requests: s.settled_requests,
                active_requests: s.active_requests,
                denied_requests: s.denied_requests,
                rate_limited: s.rate_limited,
                overrun: s.overrun,
                halted: s.halted,
                by_role: s.by_role.clone(),
            },
        }
    }
}
pub struct Reservation {
    budget: Arc<Budget>,
    tokens: u64,
    role: String,
}
impl Reservation {
    /// Consume once, only for a complete response with both usage counters.
    /// Partial/error/interrupted streams keep the reservation even when partial
    /// usage appeared. Unexpected excess is charged and stops future admission.
    pub fn settle(self, prompt: u64, completion: u64) {
        let mut s = self.budget.state.lock().unwrap();
        let actual = prompt.saturating_add(completion);
        s.charged_tokens = s.charged_tokens.saturating_sub(self.tokens).saturating_add(actual);
        s.known_tokens = s.known_tokens.saturating_add(actual);
        s.settled_requests += 1;
        s.overrun |= s.charged_tokens > self.budget.limits.tokens;
    }
}
impl Reservation {
    /// A complete HTTP 429 means the provider refused before doing any work,
    /// so the attempt is returned rather than spending the run's allowance on
    /// someone else's rate limit. The retry window and deadline still bound it.
    pub fn refund_rate_limited(self) {
        let mut s = self.budget.state.lock().unwrap();
        s.requests -= 1;
        s.charged_tokens = s.charged_tokens.saturating_sub(self.tokens);
        if let Some(n) = s.by_role.get_mut(&self.role) {
            *n -= 1;
        }
        s.rate_limited += 1;
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        self.budget.state.lock().unwrap().active_requests -= 1;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    #[test]
    fn workers_race_for_one_root_without_overspending() {
        let budget = Budget::new(Limits { requests: 5, tokens: 500, seconds: 30 });
        let barrier = Arc::new(Barrier::new(16));
        let joins: Vec<_> = (0..16)
            .map(|_| {
                let (b, gate) = (budget.clone(), barrier.clone());
                std::thread::spawn(move || {
                    gate.wait();
                    b.reserve("worker", 100).is_ok()
                })
            })
            .collect();
        assert_eq!(joins.into_iter().map(|j| j.join().unwrap() as u64).sum::<u64>(), 5);
        let s = budget.snapshot();
        assert_eq!(
            (s.state.requests, s.state.charged_tokens, s.unknown_usage_requests, s.state.active_requests),
            (5, 500, 5, 0)
        );
        assert_eq!(s.state.denied_requests, 11);
    }
    #[test]
    fn settlement_refunds_only_known_usage_but_never_requests() {
        let b = Budget::new(Limits { requests: 2, tokens: 100, seconds: 30 });
        let r = b.reserve("lead", 100).unwrap();
        assert!(b.reserve("review", 1).is_err());
        r.settle(10, 5);
        drop(b.reserve("review", 85).unwrap());
        assert!(b.reserve("compaction", 1).is_err());
        let s = b.snapshot();
        assert_eq!((s.state.charged_tokens, s.state.known_tokens, s.unknown_usage_requests), (100, 15, 1));
        assert_eq!(s.state.by_role["review"], 1);
    }
    #[test]
    fn rate_limited_attempts_are_returned() {
        let b = Budget::new(Limits { requests: 1, tokens: 100, seconds: 30 });
        b.reserve("lead", 100).unwrap().refund_rate_limited();
        let r = b.reserve("lead", 100).unwrap();
        assert!(b.reserve("lead", 1).is_err());
        drop(r);
        let s = b.snapshot();
        assert_eq!((s.state.requests, s.state.charged_tokens, s.state.rate_limited), (1, 100, 1));
        assert_eq!((s.state.by_role["lead"], s.state.active_requests, s.unknown_usage_requests), (1, 0, 1));
    }
    #[test]
    fn overrun_overflow_and_expired_deadlines_fail_closed() {
        let b = Budget::new(Limits { requests: 3, tokens: 100, seconds: 30 });
        b.reserve("lead", 10).unwrap().settle(u64::MAX, 1);
        assert!(b.snapshot().state.overrun);
        assert!(b.reserve("worker", 1).is_err());
        assert!(b.remaining().is_err());
        let expired = Budget::new(Limits { seconds: 0, ..Limits::default() });
        assert!(expired.reserve("lead", 1).is_err());
        assert!(expired.remaining().is_err());
    }
}

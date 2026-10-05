//! Execution profiles control inference and review, independently of permissions.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExecutionMode {
    Careful,
    #[default]
    Standard,
    Vibe,
}

impl ExecutionMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "careful" => Some(Self::Careful),
            "standard" => Some(Self::Standard),
            "vibe" => Some(Self::Vibe),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Careful => "careful",
            Self::Standard => "standard",
            Self::Vibe => "vibe",
        }
    }

    pub fn model(self) -> Option<String> {
        std::env::var(format!("RUSTY_{}_MODEL", self.name().to_uppercase())).ok().filter(|s| !s.trim().is_empty())
    }

    pub fn max_tokens(self) -> u32 {
        match self {
            Self::Careful => 32768,
            Self::Standard => 16384,
            Self::Vibe => 8192,
        }
    }

    pub fn review_passes(self) -> usize {
        if self == Self::Careful {
            2
        } else {
            0
        }
    }

    pub fn instructions(self) -> &'static str {
        match self {
            Self::Careful => "Execution: careful. Spend reasoning on assumptions, failure modes and side effects. Before critical changes inspect the target state and describe a bounded plan with recovery steps. Make small changes, verify the result, then recheck the diff and task constraints. Use actual tool evidence; a review is not proof of production safety. Reconcile uncertain external writes before retrying. Never invent checks or rerun side-effecting actions as verification.",
            Self::Standard => "Execution: standard. Use a focused plan and normal reasoning. Verify the changed behavior with relevant tests or a direct run, and report remaining uncertainty.",
            Self::Vibe => "Execution: vibe. Optimize for fast, small, reversible iterations. Keep planning brief, use focused smoke checks, and avoid redundant reviews. Delegate independent research to a small number of read-only workers when useful; you own integration and verification. Do not skip the relevant check or claim untested behavior works. For critical production, auth, payment or data changes recommend careful mode before acting; execution mode never grants permission.",
        }
    }

    /// The documented NVIDIA Super controls are opt-in for a known endpoint/model.
    /// Unknown compatible endpoints keep their usual request schema.
    pub fn apply_inference(self, body: &mut Value, base_url: &str, model: &str) {
        body["max_tokens"] = json!(self.max_tokens());
        if base_url == crate::config::DEFAULT_BASE_URL && model == crate::config::DEFAULT_MODEL {
            match self {
                Self::Careful => {
                    body["reasoning_effort"] = json!("high");
                    body["reasoning_budget"] = json!(24576);
                }
                Self::Standard => {}
                Self::Vibe => {
                    body["reasoning_effort"] = json!("low");
                    body["reasoning_budget"] = json!(2048);
                }
            }
        }
    }
}

/// A separate inference pass is required after each completion proposal in careful mode.
/// This does not independently grade the correctness of the model's checks.
pub struct ReviewGate {
    remaining: usize,
}

impl ReviewGate {
    pub fn new(mode: ExecutionMode) -> Self {
        Self { remaining: mode.review_passes() }
    }

    pub fn next(&mut self) -> Option<&'static str> {
        let prompt = match self.remaining {
            2 => "Careful recheck 1/2: treat the proposed completion as unverified. Inspect the actual changed code or system state and test evidence. Check edge cases and regressions with relevant tools; fix anything found. Never repeat external writes to check them. If no execution is appropriate, state why. Do not close the goal yet.",
            1 => "Careful recheck 2/2: independently reconsider your assumptions using fresh reads or appropriate checks. Compare the final result against every user constraint; inspect security, data integrity and recovery where relevant. Resolve failures and distinguish executed checks from unknowns. Only then provide the final answer or call goal_done. Do not repeat consequential writes.",
            _ => return None,
        };
        self.remaining -= 1;
        Some(prompt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inference_controls_are_scoped_to_the_supported_provider() {
        let mut body = json!({});
        ExecutionMode::Careful.apply_inference(
            &mut body,
            crate::config::DEFAULT_BASE_URL,
            crate::config::DEFAULT_MODEL,
        );
        assert_eq!(body["reasoning_effort"], "high");
        assert_eq!(body["reasoning_budget"], 24576);
        let mut body = json!({});
        ExecutionMode::Vibe.apply_inference(&mut body, "http://localhost:11434/v1", "other-model");
        assert_eq!(body["max_tokens"], 8192);
        assert!(body.get("reasoning_effort").is_none());
        assert!(body.get("reasoning_budget").is_none());
        ExecutionMode::Vibe.apply_inference(&mut body, crate::config::DEFAULT_BASE_URL, crate::config::DEFAULT_MODEL);
        assert_eq!(body["reasoning_effort"], "low");
    }

    #[test]
    fn careful_requires_two_separate_rechecks() {
        let mut gate = ReviewGate::new(ExecutionMode::Careful);
        assert!(gate.next().unwrap().contains("1/2"));
        assert!(gate.next().unwrap().contains("2/2"));
        assert!(gate.next().is_none());
        assert!(ReviewGate::new(ExecutionMode::Standard).next().is_none());
        assert!(ReviewGate::new(ExecutionMode::Vibe).next().is_none());
    }
}

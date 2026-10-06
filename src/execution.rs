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

    /// Careful mode has one read-only checker review the finished work.
    pub fn checker(self) -> &'static str {
        if self == Self::Careful {
            "one read-only check"
        } else {
            "off"
        }
    }

    /// Vibe keeps delegation small unless the user picked a swarm size.
    pub const VIBE_WORKERS: usize = 3;

    pub fn instructions(self) -> &'static str {
        match self {
            Self::Careful => "Execution: careful. Spend reasoning on assumptions, failure modes and side effects. Before critical changes inspect the target state and describe a bounded plan with recovery steps. Make small changes, verify the result, then recheck the diff and task constraints. Use actual tool evidence; a review is not proof of production safety. Reconcile uncertain external writes before retrying. Never invent checks or rerun side-effecting actions as verification.",
            Self::Standard => "Execution: standard. Use a focused plan and normal reasoning. Verify the changed behavior with relevant tests or a direct run, and report remaining uncertainty.",
            Self::Vibe => "Execution: vibe. Optimize for fast, small, reversible iterations. Keep planning brief, use focused smoke checks, and avoid redundant reviews. Delegate independent research to a small number of read-only workers when useful; you own integration and verification. Do not skip the relevant check or claim untested behavior works. For critical production, auth, payment or data changes recommend careful mode before acting; execution mode never grants permission.",
        }
    }

    /// The documented NVIDIA Super controls are opt-in for a known endpoint/model.
    /// Unknown compatible endpoints keep their usual request schema, and never
    /// get an output cap above the 16k every model has handled so far.
    pub fn apply_inference(self, body: &mut Value, base_url: &str, model: &str) {
        let known = base_url == crate::config::DEFAULT_BASE_URL && model == crate::config::DEFAULT_MODEL;
        body["max_tokens"] = json!(if known { self.max_tokens() } else { self.max_tokens().min(16384) });
        if known {
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

/// Picks a starting profile for a request when the user hasn't chosen one,
/// with a short reason. Only clear signals move it off standard.
pub fn pick(request: &str) -> (ExecutionMode, String) {
    let lower = request.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let word = |w: &[&str]| words.iter().any(|x| w.contains(x));
    let stem = |s: &[&str]| words.iter().any(|x| s.iter().any(|p| x.starts_with(p)));
    let phrase = |p: &[&str]| p.iter().any(|p| lower.contains(p));
    let change = change_intent(request);
    let careful: [(&str, bool); 9] = [
        ("production", word(&["prod", "production", "live"]) && !phrase(&["live reload", "live preview"])),
        ("a migration", stem(&["migrat", "schema", "backfill"])),
        ("a deploy", stem(&["deploy", "rollout"]) || word(&["release"])),
        (
            "data",
            word(&["database", "db", "sql", "postgres", "mysql", "redis", "mongo"])
                || phrase(&["data loss", "user data"]),
        ),
        ("money", stem(&["payment", "billing", "invoice", "refund", "charge", "stripe", "ledger"])),
        (
            "security",
            word(&["auth", "iam"])
                || (stem(&["token"]) && word(&["rotate", "revoke"]))
                || stem(&[
                    "authent",
                    "authoriz",
                    "security",
                    "vulnerab",
                    "secret",
                    "credential",
                    "encrypt",
                    "permission",
                    "password",
                ]),
        ),
        ("infrastructure", stem(&["terraform", "kubernetes", "kubectl", "helm", "infra", "k8s", "dns", "iam"])),
        ("an incident", stem(&["incident", "outage", "rollback", "hotfix", "postmortem"])),
        (
            "you asked for care",
            stem(&["careful", "critical", "irreversib"]) || phrase(&["don't break", "do not break"]),
        ),
    ];
    let hits: Vec<&str> = careful.iter().filter(|(_, hit)| *hit).map(|(why, _)| *why).collect();
    if change && !hits.is_empty() {
        let shown = hits.iter().take(2).copied().collect::<Vec<_>>().join(" and ");
        return (ExecutionMode::Careful, format!("mentions {shown}"));
    }
    if change
        && (stem(&["prototyp", "sketch", "spike", "throwaway", "mockup", "playground"])
            || word(&["poc"])
            || phrase(&["mock up", "proof of concept"]))
    {
        return (ExecutionMode::Vibe, "sounds like a quick prototype".into());
    }
    (ExecutionMode::Standard, "nothing unusual".into())
}

/// Topic names and requests for explanation do not justify extra inference.
/// This is only the starting profile: actual risky tools still escalate.
pub fn change_intent(request: &str) -> bool {
    let lower = request.to_lowercase();
    let mut intent = lower.trim();
    for _ in 0..3 {
        if let Some(prefix) =
            ["please ", "can you ", "could you ", "would you "].iter().find(|p| intent.starts_with(**p))
        {
            intent = intent[prefix.len()..].trim_start();
        }
    }
    let words: Vec<_> = lower.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let action = words.iter().enumerate().any(|(i, w)| {
        let before = &words[i.saturating_sub(2)..i];
        let negated = before.last().is_some_and(|w| ["not", "never", "avoid"].contains(w)) || before == ["don", "t"];
        !negated
            && [
                "add",
                "fix",
                "implement",
                "build",
                "create",
                "change",
                "update",
                "remove",
                "delete",
                "rotate",
                "revoke",
                "deploy",
                "apply",
                "migrate",
                "backfill",
                "restore",
                "rollback",
                "release",
                "repair",
                "resolve",
                "investigate",
                "triage",
                "plan",
                "sketch",
                "prototype",
                "hotfix",
                "enable",
                "disable",
                "configure",
            ]
            .contains(w)
    });
    let informational = [
        "what is",
        "what does",
        "what did",
        "how does",
        "how do",
        "how would",
        "how can",
        "why does",
        "why is",
        "explain",
        "describe",
        "compare",
        "tell me",
        "show me",
        "should we",
        "could we",
    ]
    .iter()
    .any(|p| intent.starts_with(p))
        && !["then ", "and fix ", "and deploy ", "and implement "].iter().any(|p| lower.contains(p));
    let routine = ["typo", "typos", "documentation", "readme", "help text", "button label"]
        .iter()
        .any(|p| lower.contains(p))
        && !words.iter().any(|w| ["deploy", "apply", "migrate", "delete", "rotate", "revoke", "restore"].contains(w));
    action && !informational && !routine
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
        let mut body = json!({});
        ExecutionMode::Careful.apply_inference(&mut body, "http://localhost:11434/v1", "other-model");
        assert_eq!(body["max_tokens"], 16384, "unknown models keep the old cap");
        ExecutionMode::Vibe.apply_inference(&mut body, crate::config::DEFAULT_BASE_URL, crate::config::DEFAULT_MODEL);
        assert_eq!(body["reasoning_effort"], "low");
    }

    #[test]
    fn picks_a_profile_from_clear_signals_only() {
        let p = |s: &str| pick(s).0;
        assert_eq!(p("add a migration for the orders table"), ExecutionMode::Careful);
        assert_eq!(p("deploy the api to prod"), ExecutionMode::Careful);
        assert_eq!(p("fix the stripe refund webhook"), ExecutionMode::Careful);
        assert_eq!(p("rotate the terraform state bucket"), ExecutionMode::Careful);
        assert_eq!(p("quick prototype of a settings page"), ExecutionMode::Vibe);
        assert_eq!(p("sketch a landing page"), ExecutionMode::Vibe);
        assert_eq!(p("fix the failing parser test"), ExecutionMode::Standard);
        assert_eq!(p("why is this slow?"), ExecutionMode::Standard);
        assert_eq!(p("add live reload to the dev server"), ExecutionMode::Standard);
        // Consequential beats quick: a quick prod fix is still prod.
        assert_eq!(p("quick hotfix in production"), ExecutionMode::Careful);
        assert_eq!(pick("deploy the db migration").1, "mentions a migration and a deploy");
    }

    #[test]
    fn labeled_routing_corpus_keeps_questions_and_routine_work_standard() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("../tests/fixtures/mode-cases.json")).unwrap();
        for case in cases {
            let request = case["request"].as_str().unwrap();
            assert_eq!(pick(request).0.name(), case["mode"].as_str().unwrap(), "{request}");
        }
    }

    #[test]
    fn negated_actions_and_mixed_requests_do_not_hide_mutations() {
        assert_eq!(pick("do not deploy to production").0, ExecutionMode::Standard);
        assert_eq!(pick("don't rotate credentials").0, ExecutionMode::Standard);
        assert_eq!(pick("update README then deploy to production").0, ExecutionMode::Careful);
        assert_eq!(pick("do not deploy, fix the auth bypass").0, ExecutionMode::Careful);
        assert_eq!(pick("can you please explain how to deploy to production?").0, ExecutionMode::Standard);
        assert_eq!(pick("can you please deploy to production?").0, ExecutionMode::Careful);
    }
}

//! The infrastructure harness. A strong model can write a kubectl command;
//! it cannot know on its own which cluster that command will hit. This module
//! works out the live target (kube context, cloud account, terraform
//! workspace, git branch) so the user sees it, the model is told it, and
//! anything that looks like production starts in careful mode.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long one probe may take. Probes run in parallel, so this bounds startup.
const PROBE_SECS: u64 = 4;

/// Where the session's commands will land, as far as rusty can tell.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Target {
    pub kube_context: Option<String>,
    pub kube_namespace: Option<String>,
    pub aws_profile: Option<String>,
    pub aws_account: Option<String>,
    pub aws_region: Option<String>,
    pub gcloud_project: Option<String>,
    pub tf_workspace: Option<String>,
    pub tf_backend: Option<String>,
    pub git_branch: Option<String>,
    /// Some name above looks like production.
    pub production: bool,
}

impl Target {
    /// Probes every tool in parallel. `RUSTY_INFRA=off` skips all of it.
    pub fn detect(cwd: &Path) -> Target {
        if std::env::var("RUSTY_INFRA").is_ok_and(|v| v == "off") {
            return Target::default();
        }
        let kube = std::thread::spawn(probe_kube);
        let aws = std::thread::spawn(probe_aws);
        let gcloud = std::thread::spawn(probe_gcloud);
        let mut t = Target { git_branch: probe_git(cwd), ..Target::default() };
        (t.tf_workspace, t.tf_backend) = probe_terraform(cwd);
        if let Ok(Some((ctx, ns))) = kube.join() {
            t.kube_context = Some(ctx);
            t.kube_namespace = Some(ns);
        }
        if let Ok(Some((profile, account, region))) = aws.join() {
            t.aws_profile = Some(profile);
            t.aws_account = account;
            t.aws_region = region;
        }
        t.gcloud_project = gcloud.join().ok().flatten();
        t.production = [&t.kube_context, &t.kube_namespace, &t.aws_profile, &t.gcloud_project, &t.tf_workspace]
            .iter()
            .any(|v| v.as_deref().is_some_and(looks_production));
        t
    }

    pub fn is_empty(&self) -> bool {
        *self == Target::default()
    }

    /// One line: `kube prod-eu/payments · aws ops (123456789012, eu-west-1) · git main`.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(ctx) = &self.kube_context {
            parts.push(format!("kube {ctx}/{}", self.kube_namespace.as_deref().unwrap_or("default")));
        }
        if let Some(p) = &self.aws_profile {
            let mut extra: Vec<&str> = Vec::new();
            extra.extend(self.aws_account.as_deref());
            extra.extend(self.aws_region.as_deref());
            let extra = if extra.is_empty() { String::new() } else { format!(" ({})", extra.join(", ")) };
            parts.push(format!("aws {p}{extra}"));
        }
        if let Some(p) = &self.gcloud_project {
            parts.push(format!("gcloud {p}"));
        }
        if let Some(w) = &self.tf_workspace {
            let backend = self.tf_backend.as_ref().map(|b| format!(" ({b})")).unwrap_or_default();
            parts.push(format!("terraform {w}{backend}"));
        }
        if let Some(b) = &self.git_branch {
            parts.push(format!("git {b}"));
        }
        parts.join(" · ")
    }

    /// The block the model sees. Short: it is paid for on every step.
    pub fn prompt_block(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        let mut s = format!("\nLive infrastructure targets: {}.", self.summary());
        if self.production {
            s.push_str(" One of these looks like PRODUCTION.");
        }
        s.push_str(
            " Commands run against exactly these unless they pass --context, --profile or similar; name the \
             target before any change, and never change one the user has not confirmed. Diff or plan before \
             you apply (kubectl diff or --dry-run=server, helm diff, terraform plan -out=FILE then apply FILE). \
             Never add --force, --prune or -auto-approve unless the user asked for exactly that.\n",
        );
        s
    }
}

/// `prod`, `prd` or `live` as a whole word or a `-`/`_`/`/`/`.`/`:` delimited part.
pub fn looks_production(name: &str) -> bool {
    let lower = name.to_lowercase();
    let is_sep = |c: char| !c.is_ascii_alphanumeric();
    ["prod", "prd", "live"].iter().any(|w| {
        lower.match_indices(w).any(|(i, _)| {
            let before = lower[..i].chars().next_back().is_none_or(is_sep);
            let after = lower[i + w.len()..].chars().next().is_none_or(is_sep);
            before && after
        })
    })
}

fn probe_kube() -> Option<(String, String)> {
    let out = run(
        "kubectl",
        &["config", "view", "--minify", "-o", "jsonpath={.current-context}|{.contexts[0].context.namespace}"],
    )?;
    let (ctx, ns) = out.trim().split_once('|')?;
    if ctx.is_empty() {
        return None;
    }
    Some((ctx.to_string(), if ns.is_empty() { "default".into() } else { ns.to_string() }))
}

/// (profile, account, region). Identity is resolved with one STS call, only
/// when something says credentials are configured at all.
fn probe_aws() -> Option<(String, Option<String>, Option<String>)> {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_default();
    let has_files = home.join(".aws/credentials").exists() || home.join(".aws/config").exists();
    let profile = match env("AWS_PROFILE").or_else(|| env("AWS_DEFAULT_PROFILE")) {
        Some(p) => p,
        None if env("AWS_ACCESS_KEY_ID").is_some() => "env".into(),
        None if has_files => "default".into(),
        None => return None,
    };
    let region = env("AWS_REGION").or_else(|| env("AWS_DEFAULT_REGION")).or_else(|| {
        run("aws", &["configure", "get", "region"]).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    });
    let account = run("aws", &["sts", "get-caller-identity", "--output", "text", "--query", "Account"])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    Some((profile, account, region))
}

fn probe_gcloud() -> Option<String> {
    if let Ok(p) = std::env::var("CLOUDSDK_CORE_PROJECT") {
        if !p.trim().is_empty() {
            return Some(p.trim().to_string());
        }
    }
    let out = run("gcloud", &["config", "get-value", "project"])?;
    let p = out.trim();
    (!p.is_empty() && p != "(unset)").then(|| p.to_string())
}

/// (workspace, backend) from the files terraform leaves behind; no CLI needed.
fn probe_terraform(cwd: &Path) -> (Option<String>, Option<String>) {
    let has_tf = std::fs::read_dir(cwd)
        .map(|rd| rd.filter_map(|e| e.ok()).any(|e| e.path().extension().is_some_and(|x| x == "tf")))
        .unwrap_or(false);
    let dot = cwd.join(".terraform");
    if !has_tf && !dot.exists() {
        return (None, None);
    }
    let workspace = std::env::var("TF_WORKSPACE")
        .ok()
        .or_else(|| std::fs::read_to_string(dot.join("environment")).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "default".into());
    let backend = std::fs::read_to_string(dot.join("terraform.tfstate"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| {
            let b = &v["backend"];
            let kind = b["type"].as_str()?.to_string();
            let name = b["config"]["bucket"].as_str().or_else(|| b["config"]["path"].as_str());
            Some(match name {
                Some(n) => format!("{kind} {n}"),
                None => kind,
            })
        });
    (Some(workspace), backend)
}

fn probe_git(cwd: &Path) -> Option<String> {
    let out = Command::new("git").args(["rev-parse", "--abbrev-ref", "HEAD"]).current_dir(cwd).output().ok()?;
    let b = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !b.is_empty()).then_some(b)
}

/// Runs a probe with a time limit and returns its stdout on success.
fn run(bin: &str, args: &[&str]) -> Option<String> {
    let mut child =
        Command::new(bin).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = std::io::Read::read_to_string(&mut stdout, &mut buf);
        buf
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return reader.join().ok(),
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if start.elapsed() > Duration::from_secs(PROBE_SECS) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_names() {
        for name in ["prod", "prod-eu", "payments-prod", "arn:aws:eks:eu-west-1:1:cluster/prod-eu", "PRD_2", "live"] {
            assert!(looks_production(name), "{name}");
        }
        for name in ["product-api", "staging", "dev", "oliver", "preprod-x", "reproduce"] {
            assert!(!looks_production(name), "{name}");
        }
    }

    #[test]
    fn summary_and_prompt() {
        let t = Target {
            kube_context: Some("prod-eu".into()),
            kube_namespace: Some("payments".into()),
            aws_profile: Some("ops".into()),
            aws_account: Some("123456789012".into()),
            tf_workspace: Some("prod".into()),
            tf_backend: Some("s3 tf-state".into()),
            git_branch: Some("main".into()),
            production: true,
            ..Target::default()
        };
        assert_eq!(
            t.summary(),
            "kube prod-eu/payments · aws ops (123456789012) · terraform prod (s3 tf-state) · git main"
        );
        assert!(t.prompt_block().contains("PRODUCTION"));
        assert!(Target::default().prompt_block().is_empty());
    }

    #[test]
    fn terraform_files_give_workspace_and_backend() {
        let dir = std::env::temp_dir().join(format!("rusty-infra-tf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".terraform")).unwrap();
        assert_eq!(probe_terraform(&dir), (Some("default".into()), None));
        std::fs::write(dir.join(".terraform/environment"), "prod\n").unwrap();
        std::fs::write(
            dir.join(".terraform/terraform.tfstate"),
            r#"{"backend":{"type":"s3","config":{"bucket":"tf-state","key":"app"}}}"#,
        )
        .unwrap();
        assert_eq!(probe_terraform(&dir), (Some("prod".into()), Some("s3 tf-state".into())));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

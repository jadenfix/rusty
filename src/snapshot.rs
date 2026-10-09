//! A fixed picture of the workspace for the system prompt: what is here and
//! what can run, gathered once at startup without a model call. It saves the
//! first few exploratory calls, which matters most for smaller models.
use std::path::Path;
use std::process::Command;

const MAX_ENTRIES: usize = 40;
const TOOLS: &[&str] = &[
    "git",
    "python3",
    "uv",
    "pip",
    "node",
    "npm",
    "cargo",
    "go",
    "java",
    "make",
    "docker",
    "kubectl",
    "helm",
    "terraform",
    "tofu",
    "psql",
    "redis-cli",
    "curl",
    "jq",
    "rg",
    "aws",
    "gcloud",
    "az",
    "sc",
];

pub fn take(cwd: &Path) -> String {
    let mut lines = vec!["Workspace snapshot (taken once at startup; re-check anything you rely on):".to_string()];
    if let Some(files) = files(cwd) {
        lines.push(format!("- files here: {files}"));
    }
    if let Some(git) = git(cwd) {
        lines.push(format!("- git: {git}"));
    }
    let tools = on_path(TOOLS);
    if !tools.is_empty() {
        lines.push(format!("- tools on PATH: {}", tools.join(", ")));
    }
    if lines.len() == 1 {
        return String::new();
    }
    format!("\n{}\n", lines.join("\n"))
}

/// Top-level entries, directories first and marked with `/`.
fn files(cwd: &Path) -> Option<String> {
    let mut entries: Vec<(bool, String)> = std::fs::read_dir(cwd)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| (e.file_type().is_ok_and(|t| t.is_dir()), e.file_name().to_string_lossy().into_owned()))
        .filter(|(_, name)| !matches!(name.as_str(), ".git" | "target" | "node_modules" | "__pycache__" | ".venv"))
        .collect();
    if entries.is_empty() {
        return None;
    }
    entries.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let more = entries.len().saturating_sub(MAX_ENTRIES);
    let mut shown: Vec<String> =
        entries.into_iter().take(MAX_ENTRIES).map(|(dir, n)| if dir { format!("{n}/") } else { n }).collect();
    if more > 0 {
        shown.push(format!("+{more} more"));
    }
    Some(shown.join(" "))
}

/// Branch, a dirty mark and the last three subjects.
fn git(cwd: &Path) -> Option<String> {
    let run = |args: &[&str]| {
        let out = Command::new("git").arg("-C").arg(cwd).args(args).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let branch = run(&["rev-parse", "--abbrev-ref", "HEAD"])?;
    let dirty = run(&["status", "--porcelain"]).is_some_and(|s| !s.is_empty());
    let log = run(&["log", "--oneline", "-3", "--no-decorate"]).unwrap_or_default();
    let recent: Vec<&str> = log.lines().collect();
    Some(format!(
        "branch {branch}{}; recent: {}",
        if dirty { " (uncommitted changes)" } else { "" },
        if recent.is_empty() { "no commits".into() } else { recent.join(" | ") }
    ))
}

fn on_path(names: &[&str]) -> Vec<String> {
    let dirs: Vec<std::path::PathBuf> =
        std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    names.iter().filter(|n| dirs.iter().any(|d| executable(&d.join(n)))).map(|n| n.to_string()).collect()
}

fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_files_git_and_tools_within_bounds() {
        let dir = std::env::temp_dir().join(format!("rusty-snapshot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("node_modules")).unwrap();
        for i in 0..50 {
            std::fs::write(dir.join(format!("f{i:02}.txt")), "").unwrap();
        }
        let ok = |args: &[&str]| Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap().status.success();
        let has_git = ok(&["init", "-q"])
            && ok(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "first"]);
        let s = take(&dir);
        assert!(s.contains("files here: src/ f00.txt"), "{s}");
        assert!(s.contains("+11 more") && !s.contains("node_modules"), "{s}");
        if has_git {
            assert!(s.contains("recent: ") && s.contains("first") && s.contains("(uncommitted changes)"), "{s}");
        }
        assert!(s.len() < 2_000, "{}", s.len());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_directory_adds_nothing_but_tools() {
        let dir = std::env::temp_dir().join(format!("rusty-snapshot-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!take(&dir).contains("files here"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

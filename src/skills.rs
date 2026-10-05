//! Skills: reusable prompts invoked as slash commands (`/review`, `/explore`).
//!
//! Built-ins ship in the binary. Your own go in `~/.config/rusty/skills/` or
//! `<project>/.rusty/skills/` as `name.md`, with an optional front matter
//! `description:` line. `{{args}}` is replaced by whatever follows the command.
//! A project skill overrides a personal one, which overrides a built-in.

use std::path::Path;

pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,
    pub source: &'static str,
}

const BUILTIN: &[(&str, &str)] = &[
    ("review", include_str!("../skills/review.md")),
    ("explore", include_str!("../skills/explore.md")),
    ("feature", include_str!("../skills/feature.md")),
    ("debug", include_str!("../skills/debug.md")),
    ("test", include_str!("../skills/test.md")),
    ("commit", include_str!("../skills/commit.md")),
    ("triage", include_str!("../skills/triage.md")),
    ("change", include_str!("../skills/change.md")),
    ("postmortem", include_str!("../skills/postmortem.md")),
];

fn parse(name: &str, text: &str, source: &'static str) -> Skill {
    let mut description = String::new();
    let mut body = text;
    if let Some(rest) = text.strip_prefix("---\n") {
        if let Some((front, after)) = rest.split_once("\n---\n") {
            for line in front.lines() {
                if let Some(d) = line.strip_prefix("description:") {
                    description = d.trim().to_string();
                }
            }
            body = after;
        }
    }
    Skill { name: name.to_string(), description, body: body.trim().to_string(), source }
}

pub fn all(cwd: &Path) -> Vec<Skill> {
    let mut skills: Vec<Skill> = BUILTIN.iter().map(|(n, t)| parse(n, t, "built-in")).collect();
    let dirs = [
        (crate::config::config_dir().map(|d| d.join("skills")), "personal"),
        (Some(cwd.join(".rusty").join("skills")), "project"),
    ];
    for (dir, source) in dirs {
        let Some(dir) = dir else { continue };
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.filter_map(|e| e.ok()) {
            let path = e.path();
            if path.extension().is_none_or(|x| x != "md") {
                continue;
            }
            let Some(name) = path.file_stem().map(|s| s.to_string_lossy().to_string()) else { continue };
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            skills.retain(|s| s.name != name);
            skills.push(parse(&name, &text, source));
        }
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

pub fn find(cwd: &Path, name: &str) -> Option<Skill> {
    all(cwd).into_iter().find(|s| s.name == name)
}

pub fn render(skill: &Skill, args: &str) -> String {
    skill.body.replace("{{args}}", args.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_parse_with_descriptions() {
        let skills = all(Path::new("/nonexistent"));
        for name in ["review", "explore", "feature", "debug", "test", "commit", "triage", "change", "postmortem"] {
            let s = skills.iter().find(|s| s.name == name).unwrap();
            assert!(!s.description.is_empty(), "{name} has no description");
            assert!(s.body.contains("{{args}}"), "{name} ignores args");
        }
        let r = render(skills.iter().find(|s| s.name == "debug").unwrap(), " login fails ");
        assert!(r.starts_with("Debug this: login fails"));
    }
}

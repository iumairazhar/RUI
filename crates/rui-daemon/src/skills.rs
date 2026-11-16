//! Skills: SKILL.md files that tell a team member how to work with one stack.
//!
//! Starter skills are embedded in the binary and copied to `<data_dir>/skills`
//! on startup (never overwriting edits). Frontmatter is parsed by hand.

use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// One parsed skill.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// Bash patterns the member may run, e.g. `uv *`.
    pub bash: Vec<String>,
    pub run_hint: Option<String>,
    pub body: String,
}

const EMBEDDED: &[(&str, &str)] = &[
    ("fastapi", include_str!("../skills/fastapi/SKILL.md")),
    ("node-express", include_str!("../skills/node-express/SKILL.md")),
    ("react-vite", include_str!("../skills/react-vite/SKILL.md")),
    ("tester", include_str!("../skills/tester/SKILL.md")),
    ("fixer", include_str!("../skills/fixer/SKILL.md")),
    ("reviewer", include_str!("../skills/reviewer/SKILL.md")),
];

/// Splits `---` frontmatter from the body.
fn split_frontmatter(text: &str) -> Result<(&str, &str)> {
    let rest = text.strip_prefix("---\n").context("missing frontmatter")?;
    let end = rest.find("\n---").context("unterminated frontmatter")?;
    let body = rest[end + 4..].trim_start_matches('\n');
    Ok((&rest[..end], body))
}

fn field<'a>(front: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}:");
    front.lines().find_map(|l| l.strip_prefix(&prefix)).map(str::trim)
}

/// Parses SKILL.md text.
pub fn parse(text: &str) -> Result<Skill> {
    let (front, body) = split_frontmatter(text)?;
    let name = field(front, "name").filter(|v| !v.is_empty()).context("missing name")?;
    let description = field(front, "description").context("missing description")?;
    let bash = match field(front, "bash") {
        Some(v) => serde_json::from_str(v).context("bash must be a JSON array of strings")?,
        None => Vec::new(),
    };
    if body.trim().is_empty() {
        bail!("empty skill body");
    }
    Ok(Skill {
        name: name.into(),
        description: description.into(),
        bash,
        run_hint: field(front, "run_hint").filter(|v| !v.is_empty()).map(String::from),
        body: body.trim_end().into(),
    })
}

fn skills_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("skills")
}

/// Writes each embedded skill to disk, only if its file is missing.
pub fn seed(data_dir: &Path) {
    for (name, text) in EMBEDDED {
        let dir = skills_dir(data_dir).join(name);
        let file = dir.join("SKILL.md");
        if file.exists() {
            continue;
        }
        let res = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&file, text));
        if let Err(e) = res {
            eprintln!("cannot seed skill {name}: {e}");
        }
    }
}

/// All parsable skills on disk, sorted by name.
pub fn load_all(data_dir: &Path) -> Vec<Skill> {
    let Ok(entries) = std::fs::read_dir(skills_dir(data_dir)) else { return Vec::new() };
    let mut skills: Vec<Skill> = entries
        .flatten()
        .filter_map(|e| std::fs::read_to_string(e.path().join("SKILL.md")).ok())
        .filter_map(|text| parse(&text).ok())
        .collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// One skill by name.
pub fn load(data_dir: &Path, name: &str) -> Option<Skill> {
    load_all(data_dir).into_iter().find(|s| s.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WITH_HINT: &str = "---\nname: x\ndescription: d\nbash: [\"a *\", \"b *\"]\nrun_hint: run $PORT\n---\nBody line\n";

    #[test]
    fn parses_with_run_hint() {
        let s = parse(WITH_HINT).unwrap();
        assert_eq!(s.name, "x");
        assert_eq!(s.bash, ["a *", "b *"]);
        assert_eq!(s.run_hint.as_deref(), Some("run $PORT"));
        assert_eq!(s.body, "Body line");
    }

    #[test]
    fn parses_without_run_hint() {
        let s = parse("---\nname: y\ndescription: d\nbash: []\n---\nHi\n").unwrap();
        assert_eq!(s.run_hint, None);
        assert!(s.bash.is_empty());
    }

    #[test]
    fn bad_frontmatter_errors() {
        assert!(parse("no frontmatter").is_err());
        assert!(parse("---\nname: x\n---\nbody").is_err());
        assert!(parse("---\nname: x\ndescription: d\nbash: nope\n---\nbody").is_err());
    }

    #[test]
    fn embedded_skills_all_parse() {
        for (name, text) in EMBEDDED {
            assert_eq!(&parse(text).unwrap().name, name);
        }
    }

    #[test]
    fn seed_does_not_overwrite_edits() {
        let d = tempfile::tempdir().unwrap();
        seed(d.path());
        assert_eq!(load_all(d.path()).len(), 6);
        let file = d.path().join("skills/tester/SKILL.md");
        std::fs::write(&file, WITH_HINT.replace("name: x", "name: tester")).unwrap();
        seed(d.path());
        assert_eq!(load(d.path(), "tester").unwrap().body, "Body line");
        assert!(load(d.path(), "nope").is_none());
    }
}

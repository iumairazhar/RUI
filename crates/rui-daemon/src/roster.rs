//! Roster: agent roles. Each role is `<data_dir>/roles/<role>/AGENT.md`.
//!
//! Roles are seeded from the source tree (`roles/`) like skills. Frontmatter is
//! parsed by hand: `name`, `code`, `title`, `description`, `tools: [..]`,
//! `skills: [..]`, `max_loops`, and optional run defaults
//! `model` and `effort` (see `run_settings`). Unknown keys and nested lines are ignored.

use crate::skills::{field, seed_from, split_frontmatter};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::path::{Path, PathBuf};

const DEFAULT_MAX_LOOPS: i64 = 5;

/// Tool group names a role may be given.
pub const ROLE_TOOLS: [&str; 6] = ["Read", "Search", "Edit", "Shell", "Run tests", "Fetch URL"];

/// One parsed role.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Role {
    pub name: String,
    pub code: String,
    pub title: String,
    pub description: String,
    /// Tool groups: Read, Search, Edit, Shell, "Run tests", "Fetch URL".
    pub tools: Vec<String>,
    /// Skill folder names installed for this role.
    pub skills: Vec<String>,
    pub max_loops: i64,
    /// Default model for this role's runs; None leaves the CLI's default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Default effort for this role's runs; None leaves the CLI's default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(skip_serializing)]
    pub body: String,
}

fn roles_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("roles")
}

fn string_list(front: &str, key: &str) -> Result<Vec<String>> {
    match field(front, key) {
        Some(v) => serde_json::from_str(v).with_context(|| format!("{key} must be a JSON array of strings")),
        None => Ok(Vec::new()),
    }
}

fn text(front: &str, key: &str) -> Option<String> {
    field(front, key).filter(|v| !v.is_empty()).map(String::from)
}

/// Parses AGENT.md text.
pub fn parse(src: &str) -> Result<Role> {
    let (front, body) = split_frontmatter(src)?;
    let name = text(front, "name").context("missing name")?;
    let max_loops = match field(front, "max_loops") {
        Some(v) => v.parse().context("max_loops must be a number")?,
        None => DEFAULT_MAX_LOOPS,
    };
    if body.trim().is_empty() {
        bail!("empty role body");
    }
    let run = crate::run_settings::check(field(front, "model"), field(front, "effort"))?;
    Ok(Role {
        code: text(front, "code").unwrap_or_else(|| name.clone()),
        title: text(front, "title").unwrap_or_else(|| name.clone()),
        description: text(front, "description").context("missing description")?,
        tools: string_list(front, "tools")?,
        skills: string_list(front, "skills")?,
        max_loops,
        model: run.model,
        effort: run.effort,
        body: body.trim_end().into(),
        name,
    })
}

/// Seeds `<data_dir>/roles` from the source tree's `roles/` folder.
pub fn seed(data_dir: &Path) {
    let src = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/roles"));
    seed_from(src, &roles_dir(data_dir));
}

/// All parsable roles on disk, sorted by name.
pub fn load_all(data_dir: &Path) -> Vec<Role> {
    let Ok(entries) = std::fs::read_dir(roles_dir(data_dir)) else { return Vec::new() };
    let mut roles: Vec<Role> = entries
        .flatten()
        .filter_map(|e| std::fs::read_to_string(e.path().join("AGENT.md")).ok())
        .filter_map(|text| parse(&text).ok())
        .collect();
    roles.sort_by(|a, b| a.name.cmp(&b.name));
    roles
}

/// One role by name.
pub fn load(data_dir: &Path, name: &str) -> Option<Role> {
    load_all(data_dir).into_iter().find(|r| r.name == name)
}

/// Display title of a role, falling back to the plain name.
pub fn title_of(data_dir: &Path, name: &str) -> String {
    load(data_dir, name).map(|r| r.title).unwrap_or_else(|| name.to_string())
}

/// The Claude CLI tools a role may use. Bash is allowed broadly only through `Shell`.
pub fn cli_tools(role: &Role) -> Vec<String> {
    let mut out: Vec<&str> = Vec::new();
    for t in &role.tools {
        match t.as_str() {
            "Read" => out.push("Read"),
            "Search" => out.extend(["Glob", "Grep"]),
            "Edit" => out.extend(["Edit", "Write"]),
            "Shell" => out.push("Bash"),
            "Fetch URL" => out.extend(["WebFetch", "WebSearch"]),
            _ => {}
        }
    }
    out.into_iter().map(String::from).collect()
}

/// One catalog line per role that is switched on, for Rui's prompt.
pub fn catalog(data_dir: &Path) -> String {
    let lines: Vec<String> = load_all(data_dir)
        .iter()
        .filter(|r| crate::library::is_on(data_dir, crate::library::Kind::Role, &r.name))
        .map(|r| format!("- {} {}: {} (skills: {})", r.code, r.name, r.description, r.skills.join(", ")))
        .collect();
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROLE: &str = "---\nname: backend\ncode: BE\ntitle: Backend Engineer\ndescription: Builds APIs\ntools: [\"Read\", \"Search\", \"Edit\", \"Run tests\", \"Fetch URL\"]\nskills: [\"fastapi\"]\nmax_loops: 6\nmetadata:\n  x: y\n---\nYou build APIs.\n";

    #[test]
    fn parses_a_role() {
        let r = parse(ROLE).unwrap();
        assert_eq!((r.name.as_str(), r.code.as_str(), r.title.as_str()), ("backend", "BE", "Backend Engineer"));
        assert_eq!(r.skills, ["fastapi"]);
        assert_eq!((r.max_loops, r.body.as_str()), (6, "You build APIs."));
    }

    #[test]
    fn defaults_and_errors() {
        let r = parse("---\nname: x\ndescription: d\n---\nb\n").unwrap();
        assert_eq!((r.code.as_str(), r.max_loops, r.tools.len()), ("x", 5, 0));
        assert!(parse("---\nname: x\n---\nb").is_err());
        assert!(parse("---\nname: x\ndescription: d\ntools: nope\n---\nb").is_err());
        assert!(parse("---\nname: x\ndescription: d\n---\n").is_err());
        assert_eq!((r.model, r.effort), (None, None));
    }

    #[test]
    fn parses_run_defaults_and_rejects_bad_ones() {
        let r = parse("---\nname: x\ndescription: d\nmodel: Opus\neffort: high\n---\nb\n").unwrap();
        assert_eq!((r.model.as_deref(), r.effort.as_deref()), (Some("opus"), Some("high")));
        assert!(parse("---\nname: x\ndescription: d\nmodel: gpt\n---\nb\n").is_err());
        assert!(parse("---\nname: x\ndescription: d\neffort: huge\n---\nb\n").is_err());
    }

    #[test]
    fn cli_tools_maps_groups() {
        let r = parse(ROLE).unwrap();
        assert_eq!(cli_tools(&r), ["Read", "Glob", "Grep", "Edit", "Write", "WebFetch", "WebSearch"]);
        let shell = Role { tools: vec!["Shell".into(), "Run tests".into()], ..r };
        assert_eq!(cli_tools(&shell), ["Bash"]);
    }

    #[test]
    fn loads_roles_and_builds_catalog() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("roles/backend");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("AGENT.md"), ROLE).unwrap();
        assert_eq!(load_all(d.path()).len(), 1);
        assert!(load(d.path(), "nope").is_none());
        assert_eq!(title_of(d.path(), "backend"), "Backend Engineer");
        assert_eq!(title_of(d.path(), "ghost"), "ghost");
        assert_eq!(catalog(d.path()), "- BE backend: Builds APIs (skills: fastapi)");
    }

    #[test]
    fn seed_copies_roles_folder_when_missing() {
        let (src, dst) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::create_dir_all(src.path().join("lead")).unwrap();
        std::fs::write(src.path().join("lead/AGENT.md"), ROLE).unwrap();
        seed_from(src.path(), &dst.path().join("roles"));
        assert!(dst.path().join("roles/lead/AGENT.md").exists());
    }
}

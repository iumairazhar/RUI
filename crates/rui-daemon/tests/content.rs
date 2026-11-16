//! Checks that every shipped role and skill file parses, and that roles only reference real skills.

use rui_daemon::{roster, skills};
use std::path::Path;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn md_files(dir: &str, file: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(Path::new(ROOT).join(dir)).unwrap() {
        let path = entry.unwrap().path().join(file);
        if path.exists() {
            out.push((path.display().to_string(), std::fs::read_to_string(&path).unwrap()));
        }
    }
    out
}

#[test]
fn every_skill_parses() {
    let files = md_files("skills", "SKILL.md");
    assert!(!files.is_empty());
    for (path, text) in files {
        skills::parse(&text).unwrap_or_else(|e| panic!("{path}: {e:#}"));
    }
}

#[test]
fn every_role_parses_and_uses_real_skills() {
    let skill_names: Vec<String> =
        md_files("skills", "SKILL.md").iter().map(|(_, t)| skills::parse(t).unwrap().name).collect();
    for (path, text) in md_files("roles", "AGENT.md") {
        let role = roster::parse(&text).unwrap_or_else(|e| panic!("{path}: {e:#}"));
        for s in &role.skills {
            assert!(skill_names.contains(s), "{path}: unknown skill {s}");
        }
    }
}

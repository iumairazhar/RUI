//! Role management: listing with sidecar data, create, loop limit, tools and delete.

use crate::library::{self as lib, Kind, LibError, LibResult, Meta, bad};
use crate::roster::{self, Role, ROLE_TOOLS};
use serde_json::{Value, json};
use std::path::Path;

const DEFAULT_SKILLS: [&str; 4] = ["executing-plans", "verification-before-completion", "reporting-status", "handoff-contract"];

const TEMPLATE: &str = "---\nname: {name}\ncode: {code}\ntitle: {title}\ndescription: {description}\ntools: [\"Read\",\"Search\"]\nskills: {skills}\nmax_loops: 6\n---\n# {title}\n\n## Job\n\n## Owns\n\n## Never\n\n## Handoffs\n\n## Reporting\n\n## When to ask_lead\n";

fn info(data: &Path, role: &Role) -> Value {
    let meta = lib::read_meta(&data.join("roles").join(&role.name));
    let mut v = serde_json::to_value(role).unwrap_or(Value::Null);
    v["status"] = json!(meta.status);
    v["author"] = json!(meta.author);
    v["hist"] = json!(meta.hist);
    v["owns_checks"] = json!(["tester", "evaluator"].contains(&role.name.as_str()));
    v
}

pub fn list(data: &Path) -> Vec<Value> {
    roster::load_all(data).iter().map(|r| info(data, r)).collect()
}

pub fn get(data: &Path, name: &str) -> LibResult<Value> {
    let dir = lib::item_dir(data, Kind::Role, name)?;
    let text = std::fs::read_to_string(dir.join("AGENT.md"))?;
    let role = roster::parse(&text).map_err(|e| LibError::Bad(format!("{e:#}")))?;
    let mut v = info(data, &role);
    v["text"] = json!(text);
    v["versions"] = json!(lib::versions(&dir).iter().map(|ts| json!({"ts": ts})).collect::<Vec<_>>());
    Ok(v)
}

/// One-line frontmatter value: not empty, no line breaks.
fn one_line(value: &str, field: &str) -> LibResult<String> {
    let v = value.trim();
    if v.is_empty() || v.contains(['\n', '\r']) {
        return bad(format!("{field} must be a single non-empty line"));
    }
    Ok(v.to_string())
}

/// Creates an off role from the template.
pub fn create(data: &Path, name: &str, code: &str, title: &str, description: &str) -> LibResult<()> {
    lib::valid_name(name)?;
    let dir = data.join("roles").join(name);
    if dir.exists() {
        return Err(LibError::Conflict(format!("role '{name}' already exists")));
    }
    let text = TEMPLATE
        .replace("{name}", name)
        .replace("{code}", &one_line(code, "code")?)
        .replace("{title}", &one_line(title, "title")?)
        .replace("{description}", &one_line(description, "description")?)
        .replace("{skills}", &serde_json::to_string(&DEFAULT_SKILLS)?);
    lib::validate(Kind::Role, name, &text)?;
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("AGENT.md"), text)?;
    let mut meta = Meta::new("off", "you");
    meta.log("created it", None, None);
    lib::write_meta(&dir, &meta)
}

/// Sets `max_loops` (3 to 20).
pub fn set_limit(data: &Path, name: &str, max_loops: i64) -> LibResult<()> {
    if !(3..=20).contains(&max_loops) {
        return bad("max_loops must be between 3 and 20");
    }
    lib::rewrite_key(data, Kind::Role, name, "max_loops", &max_loops.to_string(), &format!("set the loop limit to {max_loops}"))
}

/// Sets the role's tool groups.
pub fn set_tools(data: &Path, name: &str, tools: &[String]) -> LibResult<()> {
    if let Some(t) = tools.iter().find(|t| !ROLE_TOOLS.contains(&t.as_str())) {
        return bad(format!("unknown tool '{t}'"));
    }
    let list = serde_json::to_string(tools)?;
    lib::rewrite_key(data, Kind::Role, name, "tools", &list, &format!("set tools to {}", tools.join(", ")))
}

/// Moves a role to trash. The lead cannot be deleted.
pub fn delete(data: &Path, name: &str) -> LibResult<()> {
    if name == "lead" {
        return bad("the lead role cannot be deleted");
    }
    lib::trash(data, Kind::Role, name)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROLE: &str = "---\nname: backend\ncode: BE\ntitle: Backend\ndescription: d\ntools: [\"Read\"]\nskills: [\"alpha\"]\nmax_loops: 6\n---\nBody\n";

    fn setup() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for (name, text) in [("backend", ROLE.to_string()), ("lead", ROLE.replace("backend", "lead")), ("tester", ROLE.replace("backend", "tester"))] {
            let dir = d.path().join("roles").join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("AGENT.md"), text).unwrap();
        }
        d
    }

    #[test]
    fn list_adds_sidecar_fields_and_hides_body() {
        let d = setup();
        let all = list(d.path());
        assert_eq!(all.len(), 3);
        assert!(all[0].get("body").is_none());
        assert_eq!((all[0]["status"].as_str(), all[0]["author"].as_str()), (Some("on"), Some("shipped")));
        let owns: Vec<bool> = all.iter().map(|r| r["owns_checks"].as_bool().unwrap()).collect();
        assert_eq!(owns, [false, false, true]);
        let one = get(d.path(), "backend").unwrap();
        assert_eq!(one["text"], ROLE);
        assert_eq!(one["versions"], json!([]));
    }

    #[test]
    fn create_uses_the_template() {
        let d = setup();
        create(d.path(), "data-eng", "DE", "Data Engineer", "Builds pipelines").unwrap();
        let r = roster::load(d.path(), "data-eng").unwrap();
        assert_eq!((r.code.as_str(), r.max_loops, r.tools.as_slice()), ("DE", 6, ["Read".to_string(), "Search".to_string()].as_slice()));
        assert_eq!(r.skills, DEFAULT_SKILLS);
        for section in ["## Job", "## Owns", "## Never", "## Handoffs", "## Reporting", "## When to ask_lead"] {
            assert!(r.body.contains(section), "{section}");
        }
        let meta = lib::read_meta(&d.path().join("roles/data-eng"));
        assert_eq!((meta.status.as_str(), meta.author.as_str()), ("off", "you"));
        assert!(matches!(create(d.path(), "data-eng", "D", "T", "x"), Err(LibError::Conflict(_))));
        assert!(matches!(create(d.path(), "../x", "D", "T", "x"), Err(LibError::Bad(_))));
        assert!(matches!(create(d.path(), "ok", "D", "T", "two\nlines"), Err(LibError::Bad(_))));
    }

    #[test]
    fn limit_and_tools_rewrite_frontmatter() {
        let d = setup();
        set_limit(d.path(), "backend", 9).unwrap();
        set_tools(d.path(), "backend", &["Read".into(), "Shell".into()]).unwrap();
        let r = roster::load(d.path(), "backend").unwrap();
        assert_eq!((r.max_loops, r.tools), (9, vec!["Read".to_string(), "Shell".to_string()]));
        let text = std::fs::read_to_string(d.path().join("roles/backend/AGENT.md")).unwrap();
        assert!(text.contains("code: BE\ntitle: Backend\n") && text.ends_with("---\nBody\n"));
        assert_eq!(lib::read_meta(&d.path().join("roles/backend")).hist.len(), 2);
        assert!(set_limit(d.path(), "backend", 2).is_err() && set_limit(d.path(), "backend", 21).is_err());
        assert!(matches!(set_tools(d.path(), "backend", &["Deploy".into()]), Err(LibError::Bad(_))));
    }

    #[test]
    fn lead_cannot_be_deleted_or_turned_off() {
        let d = setup();
        assert!(matches!(delete(d.path(), "lead"), Err(LibError::Bad(_))));
        assert!(matches!(lib::set_status(d.path(), Kind::Role, "lead", "off"), Err(LibError::Bad(_))));
        assert!(d.path().join("roles/lead").exists());
        delete(d.path(), "backend").unwrap();
        assert!(!d.path().join("roles/backend").exists());
        assert!(d.path().join("trash").read_dir().unwrap().next().is_some());
    }

    #[test]
    fn off_roles_are_hidden_from_the_catalog() {
        let d = setup();
        lib::set_status(d.path(), Kind::Role, "tester", "off").unwrap();
        let cat = roster::catalog(d.path());
        assert!(cat.contains("backend") && !cat.contains("tester"));
    }
}

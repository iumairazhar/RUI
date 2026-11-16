//! Skill management: listing with sidecar data, create, import, upload, attach, tools and delete.

use crate::library::{self as lib, Kind, LibError, LibResult, Meta, bad};
use crate::roster::{self, Role};
use crate::skills::{self, Skill};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Tool names a skill may declare.
pub const SKILL_TOOLS: [&str; 7] = ["Read", "Search", "Edit", "Run tests", "Shell", "Fetch a URL", "Browser"];

const TEMPLATE: &str = "---\nname: {name}\ndescription: Describe when to use this skill.\n---\n# {name}\n\n## When to use\n\n## Steps\n\n## Pitfalls\n";

fn dir_of(data: &Path, name: &str) -> LibResult<PathBuf> {
    lib::item_dir(data, Kind::Skill, name)
}

/// One skill as JSON with sidecar data and usage.
fn info(data: &Path, skill: &Skill, roles: &[Role], used: &HashMap<String, i64>) -> Value {
    let meta = lib::read_meta(&data.join("skills").join(&skill.name));
    let attached: Vec<&str> = roles.iter().filter(|r| r.skills.contains(&skill.name)).map(|r| r.name.as_str()).collect();
    let mut v = serde_json::to_value(skill).unwrap_or(Value::Null);
    v["status"] = json!(meta.status);
    v["author"] = json!(meta.author);
    v["attach"] = json!({"roles": attached, "clients": meta.attach.clients});
    v["tools"] = json!(meta.tools);
    v["hist"] = json!(meta.hist);
    v["used_7d"] = json!(used.get(&skill.name).copied().unwrap_or(0));
    v
}

pub fn list(data: &Path, used: &HashMap<String, i64>) -> Vec<Value> {
    let roles = roster::load_all(data);
    skills::load_all(data).iter().map(|s| info(data, s, &roles, used)).collect()
}

pub fn get(data: &Path, name: &str, used: &HashMap<String, i64>) -> LibResult<Value> {
    let dir = dir_of(data, name)?;
    let skill = skills::parse(&std::fs::read_to_string(dir.join("SKILL.md"))?)?;
    let mut v = info(data, &skill, &roster::load_all(data), used);
    v["text"] = json!(std::fs::read_to_string(dir.join("SKILL.md"))?);
    v["versions"] = json!(lib::versions(&dir).iter().map(|ts| json!({"ts": ts})).collect::<Vec<_>>());
    Ok(v)
}

/// Creates an off skill from the template.
pub fn create(data: &Path, name: &str) -> LibResult<()> {
    lib::valid_name(name)?;
    let dir = data.join("skills").join(name);
    if dir.exists() {
        return Err(LibError::Conflict(format!("skill '{name}' already exists")));
    }
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("SKILL.md"), TEMPLATE.replace("{name}", name))?;
    let mut meta = Meta::new("off", "you");
    meta.log("created it", None, None);
    lib::write_meta(&dir, &meta)
}

/// Copies a skill folder in under its frontmatter name. Status review.
fn adopt(data: &Path, src: &Path, author: &str, what: &str, from: Option<&str>) -> LibResult<String> {
    let Ok(text) = std::fs::read_to_string(src.join("SKILL.md")) else { return bad("no SKILL.md in that folder") };
    let name = skills::parse(&text).map_err(|e| LibError::Bad(format!("{e:#}")))?.name;
    lib::valid_name(&name)?;
    let dest = data.join("skills").join(&name);
    if dest.exists() {
        return Err(LibError::Conflict(format!("skill '{name}' already exists")));
    }
    lib::copy_tree(src, &dest)?;
    let mut meta = Meta::new("review", author);
    meta.log(what, None, from);
    lib::write_meta(&dest, &meta)?;
    Ok(name)
}

/// Adds a skill from a local folder that holds SKILL.md.
pub fn upload(data: &Path, path: &str) -> LibResult<String> {
    let Ok(src) = Path::new(path).canonicalize() else { return bad("path not found") };
    if !src.is_dir() {
        return bad("path must be a folder");
    }
    adopt(data, &src, "you", "uploaded it", None)
}

/// Where to clone from and which folder of the clone holds the skill.
#[derive(Debug, PartialEq)]
pub struct ImportSpec {
    pub clone_url: String,
    pub branch: Option<String>,
    pub subpath: String,
}

fn safe_segment(s: &str) -> bool {
    !s.is_empty() && s != ".." && s != "." && s.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

/// Accepts https://github.com/<owner>/<repo>[/tree|blob/<ref>/<path>]. Tests may also use file:// URLs.
pub fn parse_import_url(url: &str) -> LibResult<ImportSpec> {
    #[cfg(test)]
    if let Some(rest) = url.strip_prefix("file://") {
        let (path, sub) = rest.split_once('#').unwrap_or((rest, ""));
        return Ok(ImportSpec { clone_url: format!("file://{path}"), branch: None, subpath: sub.into() });
    }
    let Some(rest) = url.strip_prefix("https://github.com/") else { return bad("only https://github.com URLs are allowed") };
    let parts: Vec<&str> = rest.trim_end_matches('/').split('/').collect();
    if parts.len() < 2 || parts.iter().any(|p| !safe_segment(p)) {
        return bad("not a valid GitHub URL");
    }
    let repo = parts[1].trim_end_matches(".git");
    let clone_url = format!("https://github.com/{}/{repo}.git", parts[0]);
    match &parts[2..] {
        [] => Ok(ImportSpec { clone_url, branch: None, subpath: String::new() }),
        [kind @ ("tree" | "blob"), branch, path @ ..] => {
            let mut path = path.to_vec();
            if *kind == "blob" && path.pop() != Some("SKILL.md") {
                return bad("a blob link must point at SKILL.md");
            }
            Ok(ImportSpec { clone_url, branch: Some(branch.to_string()), subpath: path.join("/") })
        }
        _ => bad("use a repo, tree or SKILL.md blob link"),
    }
}

/// Removes its folder when dropped.
struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn clone_repo(spec: &ImportSpec, dest: &Path) -> LibResult<()> {
    let mut cmd = std::process::Command::new("git");
    cmd.args(["clone", "--depth", "1"]);
    if let Some(b) = &spec.branch {
        cmd.args(["--branch", b]);
    }
    let out = cmd.arg("--").arg(&spec.clone_url).arg(dest).env("GIT_TERMINAL_PROMPT", "0").output()?;
    if out.status.success() {
        return Ok(());
    }
    bad(format!("git clone failed: {}", String::from_utf8_lossy(&out.stderr).trim()))
}

/// Imports a skill from a git URL (see `parse_import_url`). Blocking: run it off the async runtime.
pub fn import(data: &Path, url: &str) -> LibResult<String> {
    let spec = parse_import_url(url.trim())?;
    let tmp = TempDir(data.join("tmp").join(format!("import-{}", lib::now_ms())));
    std::fs::create_dir_all(&tmp.0)?;
    let repo = tmp.0.join("repo");
    clone_repo(&spec, &repo)?;
    let src = repo.join(&spec.subpath);
    if !src.is_dir() {
        return bad("that folder is not in the repository");
    }
    adopt(data, &src, "imported", "imported it", Some(url.trim()))
}

fn dedup(list: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in list.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

fn with_skill(skills: &[String], name: &str, present: bool) -> Vec<String> {
    let mut out: Vec<String> = skills.iter().filter(|s| *s != name).cloned().collect();
    if present {
        out.push(name.to_string());
    }
    out
}

fn set_role_skills(data: &Path, role: &str, list: &[String], what: &str) -> LibResult<()> {
    lib::rewrite_key(data, Kind::Role, role, "skills", &serde_json::to_string(list)?, what)
}

/// Makes the set of roles listing the skill equal `roles`, and stores the client list.
pub fn attach(data: &Path, name: &str, roles: &[String], clients: &[String]) -> LibResult<()> {
    let dir = dir_of(data, name)?;
    let roles = dedup(roles);
    let all = roster::load_all(data);
    if let Some(missing) = roles.iter().find(|r| !all.iter().any(|a| &a.name == *r)) {
        return Err(LibError::NotFound(format!("role '{missing}' not found")));
    }
    let mut changes: Vec<String> = Vec::new();
    for role in &all {
        let (has, want) = (role.skills.iter().any(|s| s == name), roles.contains(&role.name));
        if has == want {
            continue;
        }
        let what = if want { format!("attached skill {name}") } else { format!("detached skill {name}") };
        set_role_skills(data, &role.name, &with_skill(&role.skills, name, want), &what)?;
        changes.push(format!("{} {}", if want { "added to" } else { "removed from" }, role.name));
    }
    let mut meta = lib::read_meta(&dir);
    let clients = dedup(clients);
    if clients != meta.attach.clients {
        changes.push("changed clients".into());
    }
    meta.attach.clients = clients;
    if !changes.is_empty() {
        meta.log(&format!("attachments: {}", changes.join(", ")), None, None);
    }
    lib::write_meta(&dir, &meta)
}

/// Stores the declared tools (not enforced yet). Deploy and production access is refused.
pub fn set_tools(data: &Path, name: &str, tools: &[String]) -> LibResult<()> {
    let dir = dir_of(data, name)?;
    for t in tools {
        let lower = t.to_lowercase();
        if lower.contains("deploy") || lower.contains("production") {
            return bad("skills cannot get deploy or production access");
        }
        if !SKILL_TOOLS.contains(&t.as_str()) {
            return bad(format!("unknown tool '{t}'"));
        }
    }
    let mut meta = lib::read_meta(&dir);
    meta.tools = dedup(tools);
    meta.log(&format!("set tools to {}", if tools.is_empty() { "none".into() } else { meta.tools.join(", ") }), None, None);
    lib::write_meta(&dir, &meta)
}

/// Moves the skill to trash and removes it from every role's list.
pub fn delete(data: &Path, name: &str) -> LibResult<()> {
    dir_of(data, name)?;
    for role in roster::load_all(data).iter().filter(|r| r.skills.iter().any(|s| s == name)) {
        set_role_skills(data, &role.name, &with_skill(&role.skills, name, false), &format!("removed deleted skill {name}"))?;
    }
    lib::trash(data, Kind::Skill, name)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    const ROLE: &str = "---\nname: backend\ndescription: d\ntools: [\"Read\"]\nskills: [\"alpha\"]\nmax_loops: 6\n---\nBody\n";

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn skill_text(name: &str) -> String {
        format!("---\nname: {name}\ndescription: d\n---\nBody\n")
    }

    fn setup() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        write(&d.path().join("skills/alpha/SKILL.md"), &skill_text("alpha"));
        write(&d.path().join("skills/beta/SKILL.md"), &skill_text("beta"));
        write(&d.path().join("roles/backend/AGENT.md"), ROLE);
        write(&d.path().join("roles/tester/AGENT.md"), &ROLE.replace("backend", "tester"));
        d
    }

    fn role_skills(d: &Path, role: &str) -> Vec<String> {
        roster::load(d, role).unwrap().skills
    }

    #[test]
    fn list_and_get_carry_sidecar_data() {
        let d = setup();
        let used = HashMap::from([("alpha".to_string(), 3)]);
        let all = list(d.path(), &used);
        assert_eq!(all[0]["name"], "alpha");
        assert_eq!((all[0]["status"].as_str(), all[0]["author"].as_str(), all[0]["used_7d"].as_i64()), (Some("on"), Some("shipped"), Some(3)));
        assert_eq!(all[0]["attach"]["roles"], json!(["backend", "tester"]));
        assert_eq!(all[1]["attach"]["roles"], json!([]));
        let one = get(d.path(), "alpha", &used).unwrap();
        assert!(one["text"].as_str().unwrap().starts_with("---\nname: alpha"));
        assert_eq!(one["versions"], json!([]));
        assert!(matches!(get(d.path(), "nope", &used), Err(LibError::NotFound(_))));
    }

    #[test]
    fn attach_rewrites_role_lists_both_ways() {
        let d = setup();
        attach(d.path(), "beta", &["backend".into()], &["acme".into()]).unwrap();
        assert_eq!(role_skills(d.path(), "backend"), ["alpha", "beta"]);
        assert_eq!(role_skills(d.path(), "tester"), ["alpha"]);
        let meta = lib::read_meta(&d.path().join("skills/beta"));
        assert_eq!(meta.attach.clients, ["acme"]);
        assert_eq!(meta.hist.len(), 1);
        let role_hist = lib::read_meta(&d.path().join("roles/backend")).hist;
        assert_eq!(role_hist[0].what, "attached skill beta");
        assert!(std::fs::read_to_string(d.path().join("roles/backend/AGENT.md")).unwrap().contains("skills: [\"alpha\",\"beta\"]\nmax_loops: 6"));
        attach(d.path(), "alpha", &["tester".into()], &[]).unwrap();
        assert_eq!(role_skills(d.path(), "backend"), ["beta"]);
        assert_eq!(role_skills(d.path(), "tester"), ["alpha"]);
        assert!(matches!(attach(d.path(), "alpha", &["ghost".into()], &[]), Err(LibError::NotFound(_))));
    }

    #[test]
    fn create_validates_and_conflicts() {
        let d = setup();
        create(d.path(), "my-skill").unwrap();
        let meta = lib::read_meta(&d.path().join("skills/my-skill"));
        assert_eq!((meta.status.as_str(), meta.author.as_str()), ("off", "you"));
        assert!(skills::parse(&std::fs::read_to_string(d.path().join("skills/my-skill/SKILL.md")).unwrap()).is_ok());
        assert!(matches!(create(d.path(), "my-skill"), Err(LibError::Conflict(_))));
        assert!(matches!(create(d.path(), "Bad Name"), Err(LibError::Bad(_))));
        assert!(matches!(create(d.path(), "../x"), Err(LibError::Bad(_))));
    }

    #[test]
    fn tools_are_checked() {
        let d = setup();
        set_tools(d.path(), "alpha", &["Read".into(), "Browser".into()]).unwrap();
        assert_eq!(lib::read_meta(&d.path().join("skills/alpha")).tools, ["Read", "Browser"]);
        assert!(matches!(set_tools(d.path(), "alpha", &["Deploy to prod".into()]), Err(LibError::Bad(_))));
        assert!(matches!(set_tools(d.path(), "alpha", &["Production DB".into()]), Err(LibError::Bad(_))));
        assert!(matches!(set_tools(d.path(), "alpha", &["Teleport".into()]), Err(LibError::Bad(_))));
    }

    #[test]
    fn delete_trashes_and_detaches() {
        let d = setup();
        delete(d.path(), "alpha").unwrap();
        assert!(!d.path().join("skills/alpha").exists());
        assert!(std::fs::read_dir(d.path().join("trash")).unwrap().next().is_some());
        assert!(role_skills(d.path(), "backend").is_empty() && role_skills(d.path(), "tester").is_empty());
        assert!(matches!(delete(d.path(), "alpha"), Err(LibError::NotFound(_))));
    }

    #[test]
    fn url_parsing() {
        let tree = parse_import_url("https://github.com/o/r/tree/main/skills/x").unwrap();
        assert_eq!((tree.clone_url.as_str(), tree.branch.as_deref(), tree.subpath.as_str()), ("https://github.com/o/r.git", Some("main"), "skills/x"));
        let blob = parse_import_url("https://github.com/o/r/blob/dev/a/b/SKILL.md").unwrap();
        assert_eq!((blob.branch.as_deref(), blob.subpath.as_str()), (Some("dev"), "a/b"));
        let root = parse_import_url("https://github.com/o/r").unwrap();
        assert_eq!((root.branch, root.subpath.as_str()), (None, ""));
        for bad_url in ["http://github.com/o/r", "https://evil.com/o/r", "https://github.com/o", "https://github.com/o/r/tree/main/../x", "https://github.com/o/r/blob/main/a.md"] {
            assert!(parse_import_url(bad_url).is_err(), "{bad_url}");
        }
    }

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git").arg("-C").arg(dir).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).output().unwrap().status.success();
        assert!(ok, "git {args:?}");
    }

    fn bare_repo(root: &Path) -> PathBuf {
        let work = root.join("work");
        write(&work.join("pack/one/SKILL.md"), &skill_text("imported-one"));
        write(&work.join("pack/one/refs/more.md"), "more");
        write(&work.join("SKILL.md"), &skill_text("root-skill"));
        git(root, &["init", "-q", "work"]);
        git(&work, &["add", "."]);
        git(&work, &["commit", "-q", "-m", "init"]);
        let bare = root.join("bare.git");
        git(root, &["clone", "-q", "--bare", "work", "bare.git"]);
        bare
    }

    #[test]
    fn imports_from_a_local_bare_repo() {
        let (d, fx) = (setup(), tempfile::tempdir().unwrap());
        let bare = bare_repo(fx.path());
        let url = format!("file://{}#pack/one", bare.display());
        assert_eq!(import(d.path(), &url).unwrap(), "imported-one");
        let dir = d.path().join("skills/imported-one");
        assert_eq!(std::fs::read_to_string(dir.join("refs/more.md")).unwrap(), "more");
        let meta = lib::read_meta(&dir);
        assert_eq!((meta.status.as_str(), meta.author.as_str()), ("review", "imported"));
        assert_eq!((meta.hist[0].what.as_str(), meta.hist[0].from.as_deref()), ("imported it", Some(url.as_str())));
        assert!(matches!(import(d.path(), &url), Err(LibError::Conflict(_))));
        let root_url = format!("file://{}", bare.display());
        assert_eq!(import(d.path(), &root_url).unwrap(), "root-skill");
        assert!(!d.path().join("skills/root-skill/.git").exists());
        assert!(!d.path().join("tmp").read_dir().unwrap().any(|_| true), "temp dir must be removed");
        assert!(matches!(import(d.path(), &format!("file://{}#nope", bare.display())), Err(LibError::Bad(_))));
    }

    #[test]
    fn upload_copies_a_local_folder() {
        let (d, src) = (setup(), tempfile::tempdir().unwrap());
        write(&src.path().join("SKILL.md"), &skill_text("mine"));
        write(&src.path().join(".rui.json"), "{\"status\":\"on\",\"author\":\"shipped\"}");
        assert_eq!(upload(d.path(), &src.path().to_string_lossy()).unwrap(), "mine");
        let meta = lib::read_meta(&d.path().join("skills/mine"));
        assert_eq!((meta.status.as_str(), meta.author.as_str()), ("review", "you"));
        assert!(matches!(upload(d.path(), &src.path().to_string_lossy()), Err(LibError::Conflict(_))));
        let empty = tempfile::tempdir().unwrap();
        assert!(matches!(upload(d.path(), &empty.path().to_string_lossy()), Err(LibError::Bad(_))));
        assert!(matches!(upload(d.path(), "/definitely/not/here"), Err(LibError::Bad(_))));
    }
}

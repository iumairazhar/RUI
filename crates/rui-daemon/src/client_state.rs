//! Per-client state kept as plain files under `<data_dir>/clients/<slug>/`.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientFile {
    pub name: String,
    pub repo_path: String,
    #[serde(default)]
    pub internal: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// Email domains that belong to this client, like "acme.com". Used to match emails and meetings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub domains: Vec<String>,
    /// How to deploy this client's work. Only the owner sets it; every run needs the owner's yes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deploy: Option<DeployConfig>,
    /// What RUI may do with git branches for this client. Only the owner changes it.
    #[serde(default)]
    pub branches: BranchRules,
}

/// The owner's branch rules: may RUI merge into the base, push branches, and which branches are off limits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BranchRules {
    #[serde(default = "yes")]
    pub allow_land: bool,
    #[serde(default = "yes")]
    pub allow_push: bool,
    #[serde(default = "default_protected")]
    pub protected: Vec<String>,
}

fn yes() -> bool {
    true
}

fn default_protected() -> Vec<String> {
    ["main", "master", "production", "prod", "release"].map(String::from).to_vec()
}

impl Default for BranchRules {
    fn default() -> Self {
        BranchRules { allow_land: true, allow_push: true, protected: default_protected() }
    }
}

/// The client's branch rules; defaults when client.json or the field is missing.
pub fn branch_rules(data_dir: &Path, name: &str) -> Result<BranchRules> {
    Ok(read_client(data_dir, name)?.map(|c| c.branches).unwrap_or_default())
}

/// The owner's deploy command and the environment it deploys to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeployConfig {
    pub command: String,
    pub environment: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PodRole {
    pub role: String,
    #[serde(default)]
    pub skills: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Pod {
    #[serde(default)]
    pub roles: Vec<PodRole>,
    #[serde(default)]
    pub protected: Vec<String>,
    /// Agent CLI for this client's members: "claude" (default) or "codex". A role's own `cli` wins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli: Option<String>,
}

/// Folder holding one client's state files.
pub fn dir(data_dir: &Path, name: &str) -> PathBuf {
    data_dir.join("clients").join(crate::projects::slug(name))
}

fn now() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn create_if_missing(path: &Path, content: &str) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    Ok(fs::write(path, content)?)
}

/// Creates the folder and any missing files; existing history is left as it is.
pub fn ensure(data_dir: &Path, name: &str, repo_path: &str, internal: Option<bool>) -> Result<ClientFile> {
    if crate::projects::slug(name).is_empty() {
        bail!("client name '{name}' has no usable letters or digits");
    }
    let d = dir(data_dir, name);
    fs::create_dir_all(&d)?;
    let old = read_client(data_dir, name)?;
    let kept = old.as_ref().is_some_and(|c| c.internal);
    let domains = old.as_ref().map(|c| c.domains.clone()).unwrap_or_default();
    let deploy = old.as_ref().and_then(|c| c.deploy.clone());
    let branches = old.as_ref().map(|c| c.branches.clone()).unwrap_or_default();
    let base = old.and_then(|c| c.base);
    let file = ClientFile { name: name.to_string(), repo_path: repo_path.to_string(), internal: internal.unwrap_or(kept), base, domains, deploy, branches };
    fs::write(d.join("client.json"), serde_json::to_string_pretty(&file)?)?;
    create_if_missing(&d.join("pod.json"), &serde_json::to_string_pretty(&Pod::default())?)?;
    for f in ["briefs.md", "decisions.log", "playbook.md", "knowledge.md"] {
        create_if_missing(&d.join(f), "")?;
    }
    Ok(file)
}

pub fn read_client(data_dir: &Path, name: &str) -> Result<Option<ClientFile>> {
    let path = dir(data_dir, name).join("client.json");
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(&fs::read_to_string(path)?)?))
}

/// The branch work lands on: the client's `base` setting, else the repo's detected base branch.
pub fn base_for(data_dir: &Path, client_name: &str, repo: &Path) -> Result<String> {
    let set = read_client(data_dir, client_name)?.and_then(|c| c.base).filter(|b| !b.trim().is_empty());
    match set {
        Some(base) => Ok(base),
        None => crate::worktree::base_branch(repo),
    }
}

pub fn read_pod(data_dir: &Path, name: &str) -> Result<Pod> {
    let path = dir(data_dir, name).join("pod.json");
    if !path.exists() {
        return Ok(Pod::default());
    }
    Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
}

pub fn write_pod(data_dir: &Path, name: &str, pod: &Pod) -> Result<()> {
    let d = dir(data_dir, name);
    fs::create_dir_all(&d)?;
    Ok(fs::write(d.join("pod.json"), serde_json::to_string_pretty(pod)?)?)
}

fn append(data_dir: &Path, name: &str, file: &str, text: &str) -> Result<()> {
    let d = dir(data_dir, name);
    fs::create_dir_all(&d)?;
    let mut f = OpenOptions::new().create(true).append(true).open(d.join(file))?;
    Ok(f.write_all(text.as_bytes())?)
}

pub fn append_brief(data_dir: &Path, name: &str, task_id: i64, title: &str, brief: &str) -> Result<()> {
    append(data_dir, name, "briefs.md", &format!("## {} task {task_id}: {title}\n\n{brief}\n\n", now()))
}

pub fn append_decision(data_dir: &Path, name: &str, line: &str) -> Result<()> {
    let flat = line.replace(['\n', '\r'], " ");
    append(data_dir, name, "decisions.log", &format!("{} {}\n", now(), flat.trim()))
}

fn read_or_empty(data_dir: &Path, name: &str, file: &str) -> Result<String> {
    let path = dir(data_dir, name).join(file);
    if !path.exists() {
        return Ok(String::new());
    }
    Ok(fs::read_to_string(path)?)
}

/// Last `max_lines` lines of the decision log.
pub fn read_log(data_dir: &Path, name: &str, max_lines: usize) -> Result<String> {
    let text = read_or_empty(data_dir, name, "decisions.log")?;
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(max_lines);
    Ok(lines[start..].join("\n"))
}

pub fn read_briefs(data_dir: &Path, name: &str) -> Result<String> {
    read_or_empty(data_dir, name, "briefs.md")
}

pub fn read_playbook(data_dir: &Path, name: &str) -> Result<String> {
    read_or_empty(data_dir, name, "playbook.md")
}

/// Appends one bullet to the client's playbook. Existing lines are never touched.
pub fn append_playbook(data_dir: &Path, name: &str, text: &str) -> Result<()> {
    append(data_dir, name, "playbook.md", &bullet(text))
}

/// What the owner told RUI about this client. It always wins over anything inferred.
pub fn read_knowledge(data_dir: &Path, name: &str) -> Result<String> {
    read_or_empty(data_dir, name, "knowledge.md")
}

/// Appends one dated bullet to the client's owner knowledge. Existing lines are never touched.
pub fn append_knowledge(data_dir: &Path, name: &str, text: &str) -> Result<()> {
    if text.trim().is_empty() {
        bail!("text must not be empty");
    }
    let day = chrono::Utc::now().format("%Y-%m-%d");
    append(data_dir, name, "knowledge.md", &bullet(&format!("{day}: {}", text.trim())))
}

/// The Global Playbook every client shares: `<data_dir>/playbook.md`.
pub fn global_playbook_path(data_dir: &Path) -> PathBuf {
    data_dir.join("playbook.md")
}

pub fn read_global_playbook(data_dir: &Path) -> Result<String> {
    let path = global_playbook_path(data_dir);
    if !path.exists() {
        return Ok(String::new());
    }
    Ok(fs::read_to_string(path)?)
}

/// Appends one bullet to the Global Playbook, creating the file when missing.
pub fn append_global_playbook(data_dir: &Path, text: &str) -> Result<()> {
    fs::create_dir_all(data_dir)?;
    let mut f = OpenOptions::new().create(true).append(true).open(global_playbook_path(data_dir))?;
    Ok(f.write_all(bullet(text).as_bytes())?)
}

/// `text` as one markdown bullet line.
pub fn bullet(text: &str) -> String {
    format!("- {}\n", text.replace(['\n', '\r'], " ").trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_creates_files_and_keeps_state() {
        let t = tempfile::tempdir().unwrap();
        ensure(t.path(), "Acme Co", "/r", Some(true)).unwrap();
        let d = dir(t.path(), "Acme Co");
        for f in ["client.json", "pod.json", "briefs.md", "decisions.log", "playbook.md", "knowledge.md"] {
            assert!(d.join(f).exists(), "{f}");
        }
        append_decision(t.path(), "Acme Co", "kept").unwrap();
        let c = ensure(t.path(), "Acme Co", "/r2", None).unwrap();
        assert!(c.internal);
        assert_eq!(c.repo_path, "/r2");
        let path = d.join("client.json");
        let with_base = fs::read_to_string(&path).unwrap().replace("\"internal\": true", "\"internal\": true, \"base\": \"release\"");
        fs::write(&path, with_base).unwrap();
        assert_eq!(ensure(t.path(), "Acme Co", "/r3", None).unwrap().base.as_deref(), Some("release"));
        assert!(read_log(t.path(), "Acme Co", 10).unwrap().contains("kept"));
    }

    #[test]
    fn base_for_prefers_the_client_setting() {
        let t = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let o = std::process::Command::new("git").arg("-C").arg(repo.path()).args(args).output().unwrap();
            assert!(o.status.success());
        };
        git(&["init", "-b", "main"]);
        git(&["-c", "user.email=t@t", "-c", "user.name=t", "commit", "--allow-empty", "-m", "i"]);
        ensure(t.path(), "A", "/r", None).unwrap();
        assert_eq!(base_for(t.path(), "A", repo.path()).unwrap(), "main");
        let file = ClientFile { name: "A".into(), repo_path: "/r".into(), internal: false, base: Some("release".into()), domains: vec![], deploy: None, branches: BranchRules::default() };
        fs::write(dir(t.path(), "A").join("client.json"), serde_json::to_string(&file).unwrap()).unwrap();
        assert_eq!(base_for(t.path(), "A", repo.path()).unwrap(), "release");
    }

    #[test]
    fn branch_rules_default_when_the_field_is_missing() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(branch_rules(t.path(), "A").unwrap(), BranchRules::default());
        let d = dir(t.path(), "A");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("client.json"), r#"{"name":"A","repo_path":"/r"}"#).unwrap();
        let rules = branch_rules(t.path(), "A").unwrap();
        assert!(rules.allow_land && rules.allow_push);
        assert_eq!(rules.protected, ["main", "master", "production", "prod", "release"]);
        fs::write(d.join("client.json"), r#"{"name":"A","repo_path":"/r","branches":{"allow_push":false}}"#).unwrap();
        let rules = branch_rules(t.path(), "A").unwrap();
        assert!(rules.allow_land && !rules.allow_push && rules.protected.len() == 5);
    }

    #[test]
    fn pod_defaults_and_round_trips() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(read_pod(t.path(), "A").unwrap(), Pod::default());
        let pod = Pod { roles: vec![PodRole { role: "backend".into(), skills: vec!["rust".into()] }], protected: vec!["src/db.rs".into()], cli: None };
        write_pod(t.path(), "A", &pod).unwrap();
        assert_eq!(read_pod(t.path(), "A").unwrap(), pod);
    }

    #[test]
    fn decisions_are_flat_and_tailed() {
        let t = tempfile::tempdir().unwrap();
        append_decision(t.path(), "A", "first").unwrap();
        append_decision(t.path(), "A", " second\nline ").unwrap();
        let last = read_log(t.path(), "A", 1).unwrap();
        assert!(last.ends_with("second line"));
        assert!(!last.contains("first"));
        assert_eq!(read_log(t.path(), "A", 10).unwrap().lines().count(), 2);
    }

    #[test]
    fn brief_is_appended() {
        let t = tempfile::tempdir().unwrap();
        append_brief(t.path(), "A", 7, "Title", "Body text").unwrap();
        let b = read_briefs(t.path(), "A").unwrap();
        assert!(b.contains("task 7: Title") && b.contains("Body text"));
        assert_eq!(read_playbook(t.path(), "A").unwrap(), "");
    }

    #[test]
    fn knowledge_appends_dated_bullets_and_domains_survive_ensure() {
        let t = tempfile::tempdir().unwrap();
        ensure(t.path(), "A", "/r", None).unwrap();
        append_knowledge(t.path(), "A", "Prefers Postgres").unwrap();
        append_knowledge(t.path(), "A", "Never deploy\non Fridays").unwrap();
        assert!(append_knowledge(t.path(), "A", "  ").is_err());
        let k = read_knowledge(t.path(), "A").unwrap();
        let day = chrono::Utc::now().format("%Y-%m-%d").to_string();
        assert_eq!(k, format!("- {day}: Prefers Postgres\n- {day}: Never deploy on Fridays\n"));
        let path = dir(t.path(), "A").join("client.json");
        let with = fs::read_to_string(&path).unwrap().replace("\"internal\": false", "\"internal\": false, \"domains\": [\"a.com\"]");
        fs::write(&path, with).unwrap();
        assert_eq!(ensure(t.path(), "A", "/r", None).unwrap().domains, vec!["a.com".to_string()]);
        assert_eq!(read_knowledge(t.path(), "A").unwrap(), k);
    }

    #[test]
    fn empty_slug_errors() {
        let t = tempfile::tempdir().unwrap();
        assert!(ensure(t.path(), "!!!", "/r", None).is_err());
    }
}

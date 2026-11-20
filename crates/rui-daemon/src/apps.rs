//! Runs the apps a project declares in `.rui/apps.json`, each on its own port, inside srt.
//! The app may write only in its project, bind a local port and reach the allowed hosts.
//! With local binding on, srt opens loopback to every port, the daemon's included; the
//! daemon's routes still need the owner token or a run token, and both stay hidden.

use crate::sandbox::Sandbox;
use anyhow::{Context, Result};
use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};

const PORTS: RangeInclusive<u16> = 8100..=8199;
const LOG_LINES: usize = 200;
const HEALTH_POLLS: u32 = 120;
const HEALTH_EVERY: Duration = Duration::from_millis(500);

/// A runnable app the owner can start later.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct AppSpec {
    /// Short app name, e.g. "api" or "web".
    pub name: String,
    /// Folder inside the project to run in, relative to the project root ("." for the root).
    pub dir: String,
    /// Shell command that starts the app. May use $PORT, which is replaced with a free port.
    pub command: String,
}

/// What the API reports about one app.
#[derive(Debug, Clone, Serialize)]
pub struct AppInfo {
    pub project: String,
    pub name: String,
    pub port: u16,
    pub url: String,
    /// One of: starting, running, failed, stopped.
    pub status: String,
    pub log: Vec<String>,
}

/// An app's info plus its process, if it is alive.
pub struct AppEntry {
    pub info: AppInfo,
    child: Option<Child>,
    /// The app's srt settings and temp folder, removed when the app stops.
    run: Option<crate::srt::Prepared>,
}

/// All apps, keyed by (project, name).
pub type Apps = Arc<Mutex<HashMap<(String, String), AppEntry>>>;

type Key = (String, String);

// ---- pure helpers ----

/// Reads `.rui/apps.json`; a missing file means no apps.
pub fn read_specs(repo: &Path) -> Result<Vec<AppSpec>> {
    let path = repo.join(".rui/apps.json");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

/// First port in the range that is not taken and can be bound on 127.0.0.1.
pub fn free_port(range: RangeInclusive<u16>, taken: &HashSet<u16>) -> Option<u16> {
    range
        .filter(|p| !taken.contains(p))
        .find(|p| std::net::TcpListener::bind(("127.0.0.1", *p)).is_ok())
}

fn env_name(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' }).collect()
}

/// Environment for an app: its port plus the urls of the apps started before it.
pub fn env_for(_spec: &AppSpec, port: u16, earlier: &[(String, String)]) -> Vec<(String, String)> {
    let mut env = vec![("PORT".to_string(), port.to_string())];
    for (name, url) in earlier {
        let n = env_name(name);
        env.push((format!("{n}_URL"), url.clone()));
        env.push((format!("VITE_{n}_URL"), url.clone()));
    }
    env
}

/// Replaces `$PORT` and `${PORT}` in a command.
pub fn expand_command(cmd: &str, port: u16) -> String {
    let p = port.to_string();
    cmd.replace("${PORT}", &p).replace("$PORT", &p)
}

fn push_log(log: &mut Vec<String>, line: &str) {
    log.push(line.to_string());
    if log.len() > LOG_LINES {
        log.remove(0);
    }
}

// ---- state helpers ----

fn with_entry<R>(apps: &Apps, key: &Key, f: impl FnOnce(&mut AppEntry) -> R) -> Option<R> {
    let mut map = apps.lock().ok()?;
    map.get_mut(key).map(f)
}

fn set_status(apps: &Apps, key: &Key, status: &str, reason: Option<&str>) {
    with_entry(apps, key, |e| {
        e.info.status = status.to_string();
        if let Some(r) = reason {
            push_log(&mut e.info.log, r);
        }
    });
}

fn taken_ports(apps: &Apps) -> HashSet<u16> {
    let Ok(map) = apps.lock() else { return HashSet::new() };
    map.values().filter(|e| e.child.is_some() || e.info.status == "starting").map(|e| e.info.port).collect()
}

/// Sends SIGTERM to the whole process group, then drops the child (kill_on_drop).
fn terminate(child: Child) {
    if let Some(pid) = child.id() {
        let _ = std::process::Command::new("kill").args(["-TERM", &format!("-{pid}")]).status();
    }
    drop(child);
}

/// Lists all apps, sorted by project then name.
pub fn list(apps: &Apps) -> Vec<AppInfo> {
    let Ok(map) = apps.lock() else { return Vec::new() };
    let mut out: Vec<AppInfo> = map.values().map(|e| e.info.clone()).collect();
    out.sort_by(|a, b| (&a.project, &a.name).cmp(&(&b.project, &b.name)));
    out
}

fn stop_entry(e: &mut AppEntry) {
    if let Some(child) = e.child.take() {
        terminate(child);
    }
    e.run = None;
    e.info.status = "stopped".into();
}

/// Stops every app of a project; entries stay so the UI still shows them.
pub fn stop_project(apps: &Apps, project: &str) {
    let Ok(mut map) = apps.lock() else { return };
    map.iter_mut().filter(|((p, _), _)| p == project).for_each(|(_, e)| stop_entry(e));
}

/// Stops every app of every project (daemon shutdown).
pub fn stop_all(apps: &Apps) {
    let Ok(mut map) = apps.lock() else { return };
    map.values_mut().for_each(stop_entry);
}

/// Stops one app.
pub fn stop_one(apps: &Apps, project: &str, name: &str) {
    with_entry(apps, &(project.to_string(), name.to_string()), stop_entry);
}

// ---- starting ----

fn pump_log<R: AsyncRead + Unpin + Send + 'static>(apps: Apps, key: Key, stream: R) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stream).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            with_entry(&apps, &key, |e| push_log(&mut e.info.log, &line));
        }
    });
}

fn spawn_app(apps: &Apps, key: &Key, sb: &Sandbox, repo: &Path, spec: &AppSpec, port: u16, env: &[(String, String)]) -> Result<()> {
    let run = crate::srt::prepare(crate::srt::bin_for(sb), sb, repo, true)?;
    let mut std_cmd = run.command(&expand_command(&spec.command, port), &repo.join(&spec.dir));
    std_cmd.envs(env.iter().cloned());
    std_cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut cmd = Command::from(std_cmd);
    cmd.kill_on_drop(true);
    let mut child = cmd.spawn().context("spawning app")?;
    if let Some(out) = child.stdout.take() {
        pump_log(apps.clone(), key.clone(), out);
    }
    if let Some(err) = child.stderr.take() {
        pump_log(apps.clone(), key.clone(), err);
    }
    with_entry(apps, key, |e| {
        e.child = Some(child);
        e.run = Some(run);
    });
    Ok(())
}

async fn http_ok(port: u16) -> bool {
    let probe = async {
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.ok()?;
        s.write_all(b"GET / HTTP/1.0\r\nHost: localhost\r\n\r\n").await.ok()?;
        let mut buf = [0u8; 5];
        s.read_exact(&mut buf).await.ok()?;
        Some(buf.starts_with(b"HTTP/"))
    };
    matches!(tokio::time::timeout(Duration::from_secs(2), probe).await, Ok(Some(true)))
}

/// True once the child has exited (or is gone).
fn exited(apps: &Apps, key: &Key) -> bool {
    let gone = with_entry(apps, key, |e| match e.child.as_mut() {
        Some(c) => matches!(c.try_wait(), Ok(Some(_)) | Err(_)),
        None => true,
    });
    gone.unwrap_or(true)
}

/// Polls until the app answers HTTP; Err holds the reason it did not.
async fn wait_healthy(apps: &Apps, key: &Key, port: u16) -> Result<(), String> {
    for _ in 0..HEALTH_POLLS {
        if exited(apps, key) {
            return Err("process exited before it became healthy".into());
        }
        if http_ok(port).await {
            return Ok(());
        }
        tokio::time::sleep(HEALTH_EVERY).await;
    }
    Err("timed out after 60s waiting for HTTP response".into())
}

fn new_entry(project: &str, spec: &AppSpec, port: u16) -> AppEntry {
    let info = AppInfo {
        project: project.into(),
        name: spec.name.clone(),
        port,
        url: format!("http://localhost:{port}"),
        status: "starting".into(),
        log: Vec::new(),
    };
    AppEntry { info, child: None, run: None }
}

async fn start_one(apps: &Apps, sb: &Sandbox, project: &str, repo: &Path, spec: &AppSpec, earlier: &[(String, String)]) {
    let key: Key = (project.to_string(), spec.name.clone());
    let Some(port) = free_port(PORTS, &taken_ports(apps)) else {
        let mut e = new_entry(project, spec, 0);
        e.info.status = "failed".into();
        push_log(&mut e.info.log, "no free port in 8100-8199");
        if let Ok(mut m) = apps.lock() {
            m.insert(key, e);
        }
        return;
    };
    // Inserted as "starting" first, so no other start picks this port.
    if let Ok(mut m) = apps.lock() {
        m.insert(key.clone(), new_entry(project, spec, port));
    }
    let env = env_for(spec, port, earlier);
    if let Err(e) = spawn_app(apps, &key, sb, repo, spec, port, &env) {
        return set_status(apps, &key, "failed", Some(&format!("{e:#}")));
    }
    match wait_healthy(apps, &key, port).await {
        Ok(()) => set_status(apps, &key, "running", None),
        Err(why) => {
            set_status(apps, &key, "failed", Some(&why));
        }
    }
}

/// Starts every app of a project in order, waiting for each to answer HTTP.
pub async fn start_project(apps: &Apps, sb: &Sandbox, project: &str, repo: &Path) -> Vec<AppInfo> {
    start_project_in(apps, sb, project, repo, repo).await
}

/// Where the apps' code is: the repo when every app folder is there, else the first candidate
/// (a task worktree, newest first) that has them all, else the repo.
pub fn code_dir(repo: &Path, candidates: &[PathBuf]) -> PathBuf {
    let Ok(specs) = read_specs(repo) else { return repo.to_path_buf() };
    let has_all = |d: &Path| specs.iter().all(|s| d.join(&s.dir).is_dir());
    if has_all(repo) {
        return repo.to_path_buf();
    }
    candidates.iter().find(|c| has_all(c)).cloned().unwrap_or_else(|| repo.to_path_buf())
}

/// `start_project`, reading the app list from `repo` but running each app's code in `code`.
pub async fn start_project_in(apps: &Apps, sb: &Sandbox, project: &str, repo: &Path, code: &Path) -> Vec<AppInfo> {
    stop_project(apps, project);
    let specs = match read_specs(repo) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("apps: {e:#}");
            return Vec::new();
        }
    };
    let mut earlier: Vec<(String, String)> = Vec::new();
    for spec in &specs {
        start_one(apps, sb, project, code, spec, &earlier).await;
        let key = (project.to_string(), spec.name.clone());
        if let Some(url) = with_entry(apps, &key, |e| e.info.url.clone()) {
            earlier.push((spec.name.clone(), url));
        }
    }
    list(apps).into_iter().filter(|a| a.project == project).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str) -> AppSpec {
        AppSpec { name: name.into(), dir: ".".into(), command: "x".into() }
    }

    #[test]
    fn code_dir_uses_a_worktree_only_when_the_repo_lacks_the_app_folder() {
        let repo = tempfile::tempdir().unwrap();
        let (old, new) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::create_dir_all(repo.path().join(".rui")).unwrap();
        std::fs::write(repo.path().join(".rui/apps.json"), r#"[{"name":"web","dir":"proto","command":"run $PORT"}]"#).unwrap();
        std::fs::create_dir_all(new.path().join("proto")).unwrap();
        let wts = vec![old.path().to_path_buf(), new.path().to_path_buf()];
        assert_eq!(code_dir(repo.path(), &wts), new.path());
        std::fs::create_dir_all(repo.path().join("proto")).unwrap();
        assert_eq!(code_dir(repo.path(), &wts), repo.path(), "merged code runs from the repo");
    }

    #[test]
    fn read_specs_missing_and_present() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_specs(dir.path()).unwrap().is_empty());
        std::fs::create_dir_all(dir.path().join(".rui")).unwrap();
        std::fs::write(dir.path().join(".rui/apps.json"), r#"[{"name":"api","dir":".","command":"run $PORT"}]"#).unwrap();
        let specs = read_specs(dir.path()).unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "api");
    }

    #[test]
    fn env_for_sets_port_and_earlier_urls() {
        let earlier = vec![("api".to_string(), "http://localhost:8100".to_string()), ("my-db".to_string(), "http://localhost:8101".to_string())];
        let env = env_for(&spec("web"), 8102, &earlier);
        let get = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        assert_eq!(get("PORT"), Some("8102"));
        assert_eq!(get("API_URL"), Some("http://localhost:8100"));
        assert_eq!(get("VITE_API_URL"), Some("http://localhost:8100"));
        assert_eq!(get("MY_DB_URL"), Some("http://localhost:8101"));
        assert_eq!(get("VITE_MY_DB_URL"), Some("http://localhost:8101"));
    }

    #[test]
    fn expand_command_replaces_both_forms() {
        assert_eq!(expand_command("a --port $PORT --p ${PORT}", 8100), "a --port 8100 --p 8100");
    }

    #[test]
    fn free_port_skips_taken() {
        let taken: HashSet<u16> = [8100, 8101].into();
        let p = free_port(8100..=8199, &taken).unwrap();
        assert!(p >= 8102);
        assert_eq!(free_port(8100..=8101, &taken), None);
    }

    #[test]
    fn push_log_keeps_last_lines() {
        let mut log = Vec::new();
        for i in 0..250 {
            push_log(&mut log, &i.to_string());
        }
        assert_eq!(log.len(), 200);
        assert_eq!(log[0], "50");
    }

    #[tokio::test]
    async fn start_and_stop_project() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".rui")).unwrap();
        let json = r#"[{"name":"site","dir":".","command":"python3 -m http.server $PORT"}]"#;
        std::fs::write(dir.path().join(".rui/apps.json"), json).unwrap();
        let apps = Apps::default();
        let sb = crate::srt::tests::sandbox(dir.path(), dir.path());
        let infos = start_project(&apps, &sb, "demo", dir.path()).await;
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].status, "running", "log: {:?}", infos[0].log);
        assert!(http_ok(infos[0].port).await);
        stop_project(&apps, "demo");
        assert_eq!(list(&apps)[0].status, "stopped");
    }
}

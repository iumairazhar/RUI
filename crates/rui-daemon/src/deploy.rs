//! Deploys: when a task's PR merges (or local work lands) and the client's client.json has a
//! `deploy` config, RUI records `deploy_requested {task_id, environment, command}` and shows an
//! Inbox item. Nothing runs until the owner says "deploy"; there is no undo window, because a
//! production deploy cannot be taken back. "skip" records `deploy_skipped {event_id}`.
//!
//! A deploy runs the requested command with `sh -c` in the client's repo, only when the repo is on
//! its base branch with a clean working tree (RUI never checks out or changes the owner's tree),
//! its HEAD contains the task's merge commit (or branch tip), and no other deploy is running.
//! A deploy still running when the daemon restarts is closed as failed with an unknown outcome.
//! It runs off the async runtime with a 15 minute limit; on timeout the whole process group is
//! killed. Events: `deploy_started {event_id}`, then `deploy_finished {event_id, ok, exit_code,
//! output_tail}`. A failed deploy shows as a "Deploy failed" Inbox item that can be retried or skipped.

use crate::client_state::{self, DeployConfig};
use crate::control::ControlError;
use crate::db::Db;
use crate::inbox::InboxItem;
use anyhow::{Context, Result, bail};
use rui_core::{Event, Task};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a deploy may run before its process group is killed.
pub const LIMIT: Duration = Duration::from_secs(15 * 60);
const ENVIRONMENTS: [&str; 2] = ["production", "staging"];
const KINDS: [&str; 4] = ["deploy_requested", "deploy_started", "deploy_finished", "deploy_skipped"];
const TAIL_BYTES: usize = 4096;
/// Held while a deploy's state is checked and `deploy_started` recorded, so a double click runs it once.
static START: Mutex<()> = Mutex::new(());

/// Where one deploy request stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Open,
    Running,
    Done,
    Failed,
    Skipped,
}

/// One deploy request with its latest follow-up event.
pub struct Request {
    pub event: Event,
    pub state: State,
    pub last: Option<Event>,
}

/// Everything a started deploy needs to run.
pub struct Job {
    pub event_id: i64,
    pub task_id: Option<i64>,
    pub cwd: PathBuf,
    pub command: String,
}

/// A deploy config needs a non-empty one-line command and a known environment.
pub fn validate(cfg: &DeployConfig) -> Result<()> {
    if cfg.command.trim().is_empty() {
        bail!("command must not be empty");
    }
    if cfg.command.contains(['\n', '\r']) {
        bail!("command must be one line");
    }
    if !ENVIRONMENTS.contains(&cfg.environment.as_str()) {
        bail!("environment must be production or staging");
    }
    Ok(())
}

/// Stores the owner's deploy config in the client's client.json.
pub fn set_config(data_dir: &Path, client: &str, cfg: DeployConfig) -> Result<DeployConfig> {
    validate(&cfg)?;
    let mut file = client_state::read_client(data_dir, client)?.context("client.json not found")?;
    let cfg = DeployConfig { command: cfg.command.trim().to_string(), environment: cfg.environment };
    file.deploy = Some(cfg.clone());
    let path = client_state::dir(data_dir, client).join("client.json");
    std::fs::write(path, serde_json::to_string_pretty(&file)?)?;
    Ok(cfg)
}

/// Records `deploy_requested` for a finished task when its client has a deploy config. Runs nothing.
/// `merge_commit` is the PR's merge commit when GitHub reported one.
pub fn request(db: &Db, data_dir: &Path, task: &Task, merge_commit: Option<&str>) -> Result<Option<i64>> {
    let Some(client) = db.get_client(task.client_id)? else { return Ok(None) };
    let Some(cfg) = client_state::read_client(data_dir, &client.name)?.and_then(|c| c.deploy) else { return Ok(None) };
    if validate(&cfg).is_err() {
        return Ok(None);
    }
    let payload = json!({"task_id": task.id, "environment": cfg.environment, "command": cfg.command, "merge_commit": merge_commit});
    Ok(Some(db.add_event(Some(task.id), "scheduler", "deploy_requested", &payload)?))
}

/// Every deploy request with its state, oldest first.
pub fn requests(db: &Db) -> Result<Vec<Request>> {
    let evs = db.events_of_kinds(&KINDS)?;
    let (reqs, follow): (Vec<Event>, Vec<Event>) = evs.into_iter().partition(|e| e.kind == "deploy_requested");
    Ok(reqs.into_iter().map(|r| with_state(r, &follow)).collect())
}

fn with_state(event: Event, follow: &[Event]) -> Request {
    let last = follow.iter().filter(|e| e.payload["event_id"].as_i64() == Some(event.id)).max_by_key(|e| e.id).cloned();
    let state = match last.as_ref().map(|e| (e.kind.as_str(), e.payload["ok"].as_bool())) {
        None => State::Open,
        Some(("deploy_started", _)) => State::Running,
        Some(("deploy_skipped", _)) => State::Skipped,
        Some((_, Some(true))) => State::Done,
        Some(_) => State::Failed,
    };
    Request { event, state, last }
}

fn find(db: &Db, event_id: i64) -> Result<Request, ControlError> {
    let found = requests(db)?.into_iter().find(|r| r.event.id == event_id);
    found.ok_or_else(|| ControlError::NotFound("no deploy request with that id".into()))
}

/// Only an open or failed request can be deployed or skipped.
fn actionable(r: &Request) -> Result<(), ControlError> {
    match r.state {
        State::Open | State::Failed => Ok(()),
        State::Running => Err(ControlError::Conflict("this deploy is already running".into())),
        State::Done => Err(ControlError::Conflict("this deploy already finished".into())),
        State::Skipped => Err(ControlError::Conflict("this deploy was skipped".into())),
    }
}

/// Closes every deploy left running by a previous daemon as failed with an unknown outcome.
pub fn recover(db: &Db) -> Result<usize> {
    let _lock = START.lock().unwrap_or_else(|e| e.into_inner());
    let running: Vec<Request> = requests(db)?.into_iter().filter(|r| r.state == State::Running).collect();
    for r in &running {
        let payload = json!({"event_id": r.event.id, "ok": false, "exit_code": null, "output_tail": "daemon restarted; outcome unknown"});
        db.add_event(r.event.task_id, "scheduler", "deploy_finished", &payload)?;
    }
    Ok(running.len())
}

/// Only one deploy may run at a time, across all clients.
fn none_running(db: &Db) -> Result<(), ControlError> {
    if requests(db)?.iter().any(|r| r.state == State::Running) {
        return Err(ControlError::Conflict("another deploy is still running; wait for it to finish".into()));
    }
    Ok(())
}

/// The owner skips a deploy: it leaves the Inbox and never runs.
pub fn skip(db: &Db, event_id: i64) -> Result<(), ControlError> {
    let _lock = START.lock().unwrap_or_else(|e| e.into_inner());
    let r = find(db, event_id)?;
    actionable(&r)?;
    db.add_event(r.event.task_id, "owner", "deploy_skipped", &json!({"event_id": event_id}))?;
    Ok(())
}

/// Why the repo cannot be deployed from as it is, if anything: not on `base`, or uncommitted changes.
pub fn tree_problem(repo: &Path, base: &str) -> Result<Option<String>> {
    let head = crate::worktree::git_stdout(repo, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if head.trim() != base {
        return Ok(Some(format!("the repo is on '{}', not the base branch '{base}'; check out {base} yourself first", head.trim())));
    }
    let dirty = crate::worktree::git_stdout(repo, &["status", "--porcelain"])?;
    if !dirty.trim().is_empty() {
        return Ok(Some("the repo has uncommitted or untracked changes; commit or stash them yourself first".into()));
    }
    Ok(None)
}

/// The commit HEAD must contain: the PR's merge commit, else the task's branch tip.
fn required_commit(r: &Request, task: &Task) -> Option<String> {
    let merge = r.event.payload["merge_commit"].as_str().filter(|c| !c.is_empty()).map(String::from);
    merge.or_else(|| task.branch.clone())
}

/// Why HEAD does not contain `commit`, if it does not.
pub fn ancestry_problem(repo: &Path, commit: &str) -> Option<String> {
    let ok = Command::new("git").arg("-C").arg(repo).args(["merge-base", "--is-ancestor", commit, "HEAD"])
        .stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
    (!ok).then(|| format!("HEAD does not contain {commit}; pull or merge it into the base branch yourself first"))
}

/// HEAD's short sha in the repo, or "unknown".
pub fn head_short(repo: &Path) -> String {
    let out = crate::worktree::git_stdout(repo, &["rev-parse", "--short", "HEAD"]).unwrap_or_default();
    let sha = out.trim();
    if sha.is_empty() { "unknown".into() } else { sha.to_string() }
}

/// The client's repo, when it is on its base branch with a clean tree that contains the task's work.
fn deploy_dir(db: &Db, data_dir: &Path, r: &Request) -> Result<PathBuf, ControlError> {
    let task_id = r.event.task_id.ok_or_else(|| ControlError::NotFound("deploy has no task".into()))?;
    let task = db.get_task(task_id)?.ok_or_else(|| ControlError::NotFound("task not found".into()))?;
    let client = db.get_client(task.client_id)?.ok_or_else(|| ControlError::NotFound("client not found".into()))?;
    let repo = crate::scheduler::expand_home(&client.repo_path);
    let base = client_state::base_for(data_dir, &client.name, &repo)?;
    if let Some(why) = tree_problem(&repo, &base)? {
        return Err(ControlError::Conflict(format!("cannot deploy: {why}")));
    }
    let commit = required_commit(r, &task).ok_or_else(|| ControlError::Conflict("cannot deploy: the task has no branch or merge commit to check".into()))?;
    match ancestry_problem(&repo, &commit) {
        Some(why) => Err(ControlError::Conflict(format!("cannot deploy: {why}"))),
        None => Ok(repo),
    }
}

/// The owner says deploy: checks the request and the repo, then records `deploy_started`.
/// Run the returned job with [`finish`] off the async runtime.
pub fn start(db: &Db, data_dir: &Path, event_id: i64) -> Result<Job, ControlError> {
    let _lock = START.lock().unwrap_or_else(|e| e.into_inner());
    let r = find(db, event_id)?;
    actionable(&r)?;
    none_running(db)?;
    let cwd = deploy_dir(db, data_dir, &r)?;
    let command = r.event.payload["command"].as_str().unwrap_or_default().to_string();
    if command.trim().is_empty() {
        return Err(ControlError::Conflict("deploy request has no command".into()));
    }
    db.add_event(r.event.task_id, "owner", "deploy_started", &json!({"event_id": event_id}))?;
    Ok(Job { event_id, task_id: r.event.task_id, cwd, command })
}

/// Runs a started deploy (blocking) and records `deploy_finished`. True when it passed.
pub fn finish(db: &Db, job: &Job, limit: Duration) -> Result<bool> {
    let (ok, exit_code, output_tail) = match run_command(&job.cwd, &job.command, limit) {
        Ok(Ran::Exited(code, tail)) => (code == Some(0), code, tail),
        Ok(Ran::TimedOut(tail)) => (false, None, format!("{tail}\ntimed out after {} minutes", limit.as_secs() / 60)),
        Err(e) => (false, None, format!("could not run the deploy: {e:#}")),
    };
    let payload = json!({"event_id": job.event_id, "ok": ok, "exit_code": exit_code, "output_tail": output_tail});
    db.add_event(job.task_id, "scheduler", "deploy_finished", &payload)?;
    Ok(ok)
}

/// How a deploy command ended, with the tail of its combined output.
enum Ran {
    Exited(Option<i32>, String),
    TimedOut(String),
}

fn temp_log() -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    std::env::temp_dir().join(format!("rui-deploy-{}-{nanos}.log", std::process::id()))
}

/// Runs `sh -c cmd` in its own process group; output goes to a temp file so a stray
/// background process holding a pipe can never block us.
fn run_command(cwd: &Path, cmd: &str, limit: Duration) -> Result<Ran> {
    let log = temp_log();
    let res = run_logged_to(cwd, cmd, limit, &log);
    let tail = tail_of(&log);
    let _ = std::fs::remove_file(&log);
    Ok(match res? {
        Some(code) => Ran::Exited(code, tail),
        None => Ran::TimedOut(tail),
    })
}

/// Some(exit code) when the command ended, None when it timed out and its group was killed.
fn run_logged_to(cwd: &Path, cmd: &str, limit: Duration, log: &Path) -> Result<Option<Option<i32>>> {
    use std::os::unix::process::CommandExt;
    let out = std::fs::File::create(log).context("create deploy log")?;
    let mut child = Command::new("sh")
        .args(["-c", cmd])
        .process_group(0)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(out.try_clone()?)
        .stderr(out)
        .spawn()
        .context("start deploy command")?;
    let start = Instant::now();
    while start.elapsed() < limit {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status.code()));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // Kill the whole group so whatever the command started dies with sh.
    let _ = Command::new("kill").args(["-KILL", "--", &format!("-{}", child.id())]).status();
    let _ = child.wait();
    Ok(None)
}

fn tail_of(log: &Path) -> String {
    let bytes = std::fs::read(log).unwrap_or_default();
    let start = bytes.len().saturating_sub(TAIL_BYTES);
    String::from_utf8_lossy(&bytes[start..]).trim_end().to_string()
}

// ---- Inbox ----

/// Open requests ("deploy") and failed deploys ("deploy_failed"), both with deploy and skip.
pub fn inbox_items(db: &Db) -> Result<Vec<InboxItem>> {
    let mut items = vec![];
    for r in requests(db)? {
        let title = task_title(db, &r.event)?;
        match r.state {
            State::Open => items.push(open_item(&r, &title, &repo_head(db, &r.event)?)),
            State::Failed => items.push(failed_item(&r, &title)),
            _ => {}
        }
    }
    Ok(items)
}

fn task_title(db: &Db, e: &Event) -> Result<String> {
    let task = e.task_id.map(|id| db.get_task(id)).transpose()?.flatten();
    Ok(task.map(|t| t.title).unwrap_or_else(|| "this task".into()))
}

/// HEAD's short sha in the repo of the request's client.
fn repo_head(db: &Db, e: &Event) -> Result<String> {
    let task = e.task_id.map(|id| db.get_task(id)).transpose()?.flatten();
    let client = task.map(|t| db.get_client(t.client_id)).transpose()?.flatten();
    Ok(client.map(|c| head_short(&crate::scheduler::expand_home(&c.repo_path))).unwrap_or_else(|| "unknown".into()))
}

fn field(e: &Event, key: &str) -> String {
    e.payload[key].as_str().unwrap_or_default().to_string()
}

fn actions() -> Vec<String> {
    vec!["deploy".into(), "skip".into()]
}

fn open_item(r: &Request, title: &str, head: &str) -> InboxItem {
    let env = field(&r.event, "environment");
    let detail = (env == "production").then(|| "A production deploy cannot be undone.".to_string());
    InboxItem {
        kind: "deploy",
        id: r.event.id,
        task_id: r.event.task_id,
        title: format!("Deploy {title} to {env}?"),
        reason: format!("Runs: {} at HEAD {head}", field(&r.event, "command")),
        actions: actions(),
        created_at: r.event.created_at.clone(),
        detail,
        link: None,
    }
}

fn failed_item(r: &Request, title: &str) -> InboxItem {
    let last = r.last.as_ref().unwrap_or(&r.event);
    InboxItem {
        kind: "deploy_failed",
        id: r.event.id,
        task_id: r.event.task_id,
        title: "Deploy failed".into(),
        reason: format!("{title} to {}: {}", field(&r.event, "environment"), field(&r.event, "command")),
        actions: actions(),
        created_at: last.created_at.clone(),
        detail: Some(field(last, "output_tail")),
        link: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::NewTask;
    use rui_core::{Role, SourceKind, TaskStatus};

    fn git(dir: &Path, args: &[&str]) {
        let o = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    }

    /// A repo on `main` with one commit, a client using it, and one finished task.
    fn setup(command: Option<&str>) -> (Db, tempfile::TempDir, tempfile::TempDir, Task) {
        let (db, data, repo) = (Db::open_test(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        git(repo.path(), &["init", "-b", "main"]);
        std::fs::write(repo.path().join("a.txt"), "a").unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "init"]);
        let repo_path = repo.path().to_string_lossy().into_owned();
        let c = db.create_client("acme", &repo_path, None, &[]).unwrap();
        client_state::ensure(data.path(), "acme", &repo_path, None).unwrap();
        if let Some(cmd) = command {
            set_config(data.path(), "acme", DeployConfig { command: cmd.into(), environment: "production".into() }).unwrap();
        }
        let task = db.create_task(&NewTask {
            client_id: c.id, parent_id: None, title: "Login page".into(), body: String::new(), status: TaskStatus::Done,
            done_command: None, max_loops: 3, role: Role::Lead, source_kind: SourceKind::User, source_ref: None, agent: None, pod: vec![],
        }).unwrap();
        git(repo.path(), &["branch", "rui/task-1"]);
        db.set_task_branch(task.id, "rui/task-1").unwrap();
        let task = db.get_task(task.id).unwrap().unwrap();
        (db, data, repo, task)
    }

    fn sha(dir: &Path, rev: &str) -> String {
        let o = Command::new("git").arg("-C").arg(dir).args(["rev-parse", rev]).output().unwrap();
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    }

    fn kinds(db: &Db, kind: &str) -> Vec<Event> {
        db.events_of_kinds(&[kind]).unwrap()
    }

    #[test]
    fn config_is_validated_and_survives_ensure() {
        let (_db, data, repo, _t) = setup(None);
        let cfg = |c: &str, e: &str| DeployConfig { command: c.into(), environment: e.into() };
        assert!(set_config(data.path(), "acme", cfg("  ", "production")).is_err());
        assert!(set_config(data.path(), "acme", cfg("a\nb", "production")).is_err());
        assert!(set_config(data.path(), "acme", cfg("make deploy", "prod")).is_err());
        set_config(data.path(), "acme", cfg("make deploy", "staging")).unwrap();
        let again = client_state::ensure(data.path(), "acme", &repo.path().to_string_lossy(), None).unwrap();
        assert_eq!(again.deploy, Some(cfg("make deploy", "staging")));
    }

    #[test]
    fn pr_merged_requests_a_deploy_only_when_configured() {
        let merged = crate::pr_watch::Finding::Merged { merged_at: "2026-11-18T10:00:00Z".into(), merge_commit: None };
        let (db, data, _repo, task) = setup(None);
        crate::pr_watch::record(&db, data.path(), &task, "https://x.test/pr/1", std::slice::from_ref(&merged)).unwrap();
        assert!(kinds(&db, "deploy_requested").is_empty());
        assert!(inbox_items(&db).unwrap().is_empty());

        let (db, data, _repo, task) = setup(Some("echo ok"));
        assert_eq!(head_short(_repo.path()).len(), 7);
        crate::pr_watch::record(&db, data.path(), &task, "https://x.test/pr/1", &[merged]).unwrap();
        let reqs = kinds(&db, "deploy_requested");
        assert_eq!(reqs.len(), 1);
        assert_eq!((reqs[0].payload["task_id"].as_i64(), reqs[0].payload["command"].as_str()), (Some(task.id), Some("echo ok")));
        let items = crate::inbox::inbox(&db).unwrap();
        let item = items.iter().find(|i| i.kind == "deploy").unwrap();
        assert_eq!(item.title, "Deploy Login page to production?");
        let reason = format!("Runs: echo ok at HEAD {}", head_short(_repo.path()));
        assert_eq!((item.reason.as_str(), item.actions.clone()), (reason.as_str(), actions()));
        assert!(kinds(&db, "deploy_started").is_empty(), "nothing runs without the owner");
    }

    #[test]
    fn deploy_is_refused_on_a_dirty_or_other_branch_tree() {
        let (db, data, repo, task) = setup(Some("echo ok"));
        let id = request(&db, data.path(), &task, None).unwrap().unwrap();
        std::fs::write(repo.path().join("a.txt"), "changed").unwrap();
        let err = start(&db, data.path(), id).err().unwrap();
        assert!(matches!(&err, ControlError::Conflict(m) if m.contains("uncommitted")), "{err}");
        git(repo.path(), &["checkout", "--", "a.txt"]);
        git(repo.path(), &["checkout", "-b", "feature"]);
        let err = start(&db, data.path(), id).err().unwrap();
        assert!(matches!(&err, ControlError::Conflict(m) if m.contains("not the base branch")), "{err}");
        assert!(kinds(&db, "deploy_started").is_empty());
        assert_eq!(std::fs::read_to_string(repo.path().join("a.txt")).unwrap(), "a", "the tree is never touched");
    }

    #[test]
    fn deploy_runs_and_records_finished_ok() {
        let (db, data, repo, task) = setup(Some("echo ok && pwd"));
        let id = request(&db, data.path(), &task, None).unwrap().unwrap();
        let job = start(&db, data.path(), id).unwrap();
        assert!(matches!(start(&db, data.path(), id), Err(ControlError::Conflict(_))), "no second start while running");
        assert!(finish(&db, &job, LIMIT).unwrap());
        let done = kinds(&db, "deploy_finished");
        assert_eq!((done[0].payload["ok"].as_bool(), done[0].payload["exit_code"].as_i64()), (Some(true), Some(0)));
        let tail = done[0].payload["output_tail"].as_str().unwrap();
        let real = std::fs::canonicalize(repo.path()).unwrap();
        assert!(tail.starts_with("ok") && tail.contains(real.to_str().unwrap()), "{tail}");
        assert!(inbox_items(&db).unwrap().is_empty());
    }

    #[test]
    fn failed_deploy_becomes_an_inbox_item_and_timeouts_kill_the_group() {
        let (db, data, _repo, task) = setup(Some("echo boom; exit 3"));
        let id = request(&db, data.path(), &task, None).unwrap().unwrap();
        assert!(!finish(&db, &start(&db, data.path(), id).unwrap(), LIMIT).unwrap());
        let items = inbox_items(&db).unwrap();
        assert_eq!((items[0].kind, items[0].title.as_str()), ("deploy_failed", "Deploy failed"));
        assert_eq!(items[0].detail.as_deref(), Some("boom"));
        let job = Job { command: "sleep 30".into(), ..start(&db, data.path(), id).unwrap() };
        let t = Instant::now();
        assert!(!finish(&db, &job, Duration::from_millis(300)).unwrap());
        assert!(t.elapsed() < Duration::from_secs(10));
        let last = kinds(&db, "deploy_finished").pop().unwrap();
        assert!(last.payload["output_tail"].as_str().unwrap().contains("timed out"));
    }

    #[test]
    fn deploy_is_refused_until_head_contains_the_work() {
        let (db, data, repo, task) = setup(Some("echo ok"));
        git(repo.path(), &["checkout", "-q", "rui/task-1"]);
        std::fs::write(repo.path().join("b.txt"), "b").unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "-m", "work"]);
        git(repo.path(), &["checkout", "-q", "main"]);
        let id = request(&db, data.path(), &task, None).unwrap().unwrap();
        let err = start(&db, data.path(), id).err().unwrap();
        assert!(matches!(&err, ControlError::Conflict(m) if m.contains("HEAD does not contain rui/task-1")), "{err}");
        let tip = sha(repo.path(), "rui/task-1");
        let by_merge = request(&db, data.path(), &task, Some(&tip)).unwrap().unwrap();
        assert!(matches!(start(&db, data.path(), by_merge), Err(ControlError::Conflict(_))));
        git(repo.path(), &["merge", "-q", "--ff-only", "rui/task-1"]);
        let job = start(&db, data.path(), by_merge).unwrap();
        assert!(finish(&db, &job, LIMIT).unwrap());
    }

    #[test]
    fn only_one_deploy_runs_at_a_time_and_restart_closes_it() {
        let (db, data, _repo, task) = setup(Some("echo ok"));
        let first = request(&db, data.path(), &task, None).unwrap().unwrap();
        let second = request(&db, data.path(), &task, None).unwrap().unwrap();
        let _job = start(&db, data.path(), first).unwrap();
        let err = start(&db, data.path(), second).err().unwrap();
        assert!(matches!(&err, ControlError::Conflict(m) if m.contains("another deploy")), "{err}");
        assert_eq!(recover(&db).unwrap(), 1);
        let last = kinds(&db, "deploy_finished").pop().unwrap();
        assert_eq!(last.payload, json!({"event_id": first, "ok": false, "exit_code": null, "output_tail": "daemon restarted; outcome unknown"}));
        assert_eq!(recover(&db).unwrap(), 0);
        let failed: Vec<i64> = inbox_items(&db).unwrap().iter().filter(|i| i.kind == "deploy_failed").map(|i| i.id).collect();
        assert_eq!(failed, [first]);
        assert!(start(&db, data.path(), second).is_ok());
    }

    #[test]
    fn skip_clears_the_item_and_blocks_a_deploy() {
        let (db, data, _repo, task) = setup(Some("echo ok"));
        let id = request(&db, data.path(), &task, None).unwrap().unwrap();
        skip(&db, id).unwrap();
        assert_eq!(kinds(&db, "deploy_skipped").len(), 1);
        assert!(inbox_items(&db).unwrap().is_empty());
        assert!(matches!(start(&db, data.path(), id), Err(ControlError::Conflict(_))));
        assert!(matches!(skip(&db, id), Err(ControlError::Conflict(_))));
        assert!(matches!(skip(&db, 9999), Err(ControlError::NotFound(_))));
    }
}

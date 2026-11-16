//! The scheduler: plain rules that decide what runs next, plus the code that runs it.

use crate::apps::{self, AppInfo, Apps};
use crate::db::Db;
use crate::live::{Live, LiveGuard, LiveKey, on_line};
use crate::runner::{AgentRun, ParsedLine, RunOutcome, run_agent};
use crate::skills::{self, Skill};
use crate::worktree::{DoneCheck, LandResult, commit_all, create_worktree, land, run_done_command};
use anyhow::{Context, Result};
use rui_core::{ChatRole, Client, QuestionStatus, Role, Task, TaskStatus};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const LEAD_PROMPT: &str = include_str!("../prompts/lead.md");
const BUILDER_PROMPT: &str = include_str!("../prompts/builder.md");
const LEAD_TOOLS: &str =
    "mcp__rui__ask_question,mcp__rui__create_subtask,mcp__rui__mark_split,Read,Glob,Grep";

/// Daemon settings shared by the scheduler and chat loops.
#[derive(Clone)]
pub struct Config {
    pub port: u16,
    pub tick: Duration,
    pub data_dir: PathBuf,
}

/// One thing the scheduler decided to do.
#[derive(Debug, PartialEq)]
pub enum Action {
    StartLead { task_id: i64, answers: Vec<(String, String)> },
    StartBuilder { task_id: i64 },
    Rollup { task_id: i64, status: TaskStatus },
}

// ---- pure decision layer ----

/// Looks at the database and returns what should happen now, skipping running tasks.
pub fn plan_tick(db: &Db, running: &HashSet<i64>) -> Result<Vec<Action>> {
    let tasks = db.list_tasks()?;
    let by_id: HashMap<i64, &Task> = tasks.iter().map(|t| (t.id, t)).collect();
    let mut actions = Vec::new();
    for t in tasks.iter().filter(|t| !running.contains(&t.id)) {
        if let Some(a) = plan_task(db, t, &by_id)? {
            actions.push(a);
        }
    }
    Ok(actions)
}

fn plan_task(db: &Db, t: &Task, by_id: &HashMap<i64, &Task>) -> Result<Option<Action>> {
    match (t.parent_id, t.status) {
        (None, TaskStatus::New) => Ok(Some(Action::StartLead { task_id: t.id, answers: vec![] })),
        (None, TaskStatus::WaitingAnswer) => plan_answered(db, t),
        (Some(p), TaskStatus::Open) => Ok(builder_ready(t, by_id.get(&p).copied(), by_id)),
        (_, TaskStatus::Split) => plan_rollup(db, t),
        _ => Ok(None),
    }
}

fn plan_answered(db: &Db, t: &Task) -> Result<Option<Action>> {
    let qs = db.questions_for_task(t.id)?;
    if qs.is_empty() || qs.iter().any(|q| q.status != QuestionStatus::Answered) {
        return Ok(None);
    }
    let answers = qs
        .into_iter()
        .filter_map(|q| q.answer.map(|a| (q.text, a)))
        .collect();
    Ok(Some(Action::StartLead { task_id: t.id, answers }))
}

fn builder_ready(t: &Task, parent: Option<&Task>, by_id: &HashMap<i64, &Task>) -> Option<Action> {
    if parent?.status != TaskStatus::Split {
        return None;
    }
    let deps_done = t
        .depends_on
        .iter()
        .all(|d| by_id.get(d).is_some_and(|x| x.status == TaskStatus::Done));
    deps_done.then_some(Action::StartBuilder { task_id: t.id })
}

fn plan_rollup(db: &Db, t: &Task) -> Result<Option<Action>> {
    let subs = db.subtasks(t.id)?;
    if subs.is_empty() {
        return Ok(None);
    }
    let rollup = |status| Some(Action::Rollup { task_id: t.id, status });
    if subs.iter().all(|s| s.status == TaskStatus::Done) {
        return Ok(rollup(TaskStatus::Done));
    }
    if subs.iter().any(|s| s.status == TaskStatus::Stuck) {
        return Ok(rollup(TaskStatus::Stuck));
    }
    Ok(None)
}

/// After a lead turn: a lead that neither asked nor split gets the task marked stuck.
pub fn after_lead_turn(db: &Db, task_id: i64) -> Result<TaskStatus> {
    let status = db.get_task(task_id)?.context("task not found")?.status;
    if status != TaskStatus::Clarifying {
        return Ok(status);
    }
    db.set_task_status(task_id, TaskStatus::Stuck)?;
    db.add_event(Some(task_id), "lead", "lead_no_outcome", &json!({}))?;
    Ok(TaskStatus::Stuck)
}

/// After a done-command run: decide done, open again, or stuck.
pub fn after_done_check(db: &Db, task_id: i64, check: &DoneCheck) -> Result<TaskStatus> {
    let payload = json!({"passed": check.passed, "output_tail": check.output_tail});
    db.add_event(Some(task_id), "scheduler", "done_check", &payload)?;
    let task = db.get_task(task_id)?.context("task not found")?;
    if task.status == TaskStatus::Stuck {
        return Ok(TaskStatus::Stuck);
    }
    let next = next_status(db, &task, check.passed)?;
    db.set_task_status(task_id, next)?;
    Ok(next)
}

fn next_status(db: &Db, task: &Task, passed: bool) -> Result<TaskStatus> {
    if passed {
        return Ok(TaskStatus::Done);
    }
    let used = db.increment_loops(task.id)?;
    Ok(if used >= task.max_loops { TaskStatus::Stuck } else { TaskStatus::Open })
}

/// Output tail of the most recent done check, if any.
pub fn last_done_output(db: &Db, task_id: i64) -> Result<Option<String>> {
    let events = db.list_events(Some(task_id), 1000)?;
    let last = events.iter().rev().find(|e| e.kind == "done_check");
    Ok(last.and_then(|e| e.payload.get("output_tail")?.as_str().map(String::from)))
}

// ---- effect layer ----

/// Abort handles of the turns currently running, keyed by task id.
pub type Runs = Arc<Mutex<HashMap<i64, tokio::task::AbortHandle>>>;

/// Runs forever: every tick, plan and execute. Never exits on error.
pub async fn run(db: Arc<Db>, cfg: Config, running: Runs, live: Live, apps: Apps) {
    loop {
        tokio::time::sleep(cfg.tick).await;
        let snapshot: HashSet<i64> =
            running.lock().map(|r| r.keys().copied().collect()).unwrap_or_default();
        match plan_tick(&db, &snapshot) {
            Ok(actions) => {
                for a in actions {
                    execute(&db, &cfg, &running, &live, &apps, a);
                }
            }
            Err(e) => report_error(&db, None, &e),
        }
    }
}

fn report_error(db: &Db, task_id: Option<i64>, e: &anyhow::Error) {
    eprintln!("scheduler error: {e:#}");
    let _ = db.add_event(task_id, "scheduler", "scheduler_error", &json!({"error": format!("{e:#}")}));
}

fn execute(db: &Arc<Db>, cfg: &Config, running: &Runs, live: &Live, apps: &Apps, action: Action) {
    match action {
        Action::Rollup { task_id, status } => {
            let res = db
                .set_task_status(task_id, status)
                .and_then(|_| db.add_event(Some(task_id), "scheduler", "rollup", &json!({"status": status})));
            if let Err(e) = res {
                report_error(db, Some(task_id), &e);
            } else if status == TaskStatus::Done {
                launch_apps(db, apps, task_id);
            }
        }
        Action::StartLead { task_id, answers } => {
            spawn_turn(db, cfg, running, live, task_id, TaskStatus::Clarifying, move |db, cfg, live| async move {
                lead_turn(&db, &cfg, &live, task_id, answers).await
            });
        }
        Action::StartBuilder { task_id } => {
            spawn_turn(db, cfg, running, live, task_id, TaskStatus::InProgress, move |db, cfg, live| async move {
                builder_turn(&db, &cfg, &live, task_id).await
            });
        }
    }
}

/// After a top-level Rui task is done, starts its project's apps and tells the chat.
fn launch_apps(db: &Arc<Db>, apps: &Apps, task_id: i64) {
    let Ok(Some(task)) = db.get_task(task_id) else { return };
    if task.role != Role::Rui || task.parent_id.is_some() {
        return;
    }
    let (db, apps) = (db.clone(), apps.clone());
    tokio::spawn(async move {
        if let Err(e) = launch_and_report(&db, &apps, &task).await {
            report_error(&db, Some(task.id), &e);
        }
    });
}

async fn launch_and_report(db: &Db, apps: &Apps, task: &Task) -> Result<()> {
    let client = client_for(db, task)?;
    let repo = expand_home(&client.repo_path);
    let infos = apps::start_project(apps, &client.name, &repo).await;
    db.add_event(Some(task.id), "scheduler", "apps_started", &json!({"apps": infos}))?;
    let chat_id = task.source_ref.as_deref().and_then(|r| r.strip_prefix("chat:")).and_then(|id| id.parse::<i64>().ok());
    let Some(chat_id) = chat_id else { return Ok(()) };
    let text = done_message(&task.title, &client.repo_path, &infos);
    db.add_chat_message(chat_id, ChatRole::Rui, &text)?;
    Ok(())
}

/// The chat message announcing a finished task and its apps.
fn done_message(title: &str, repo: &str, infos: &[AppInfo]) -> String {
    if infos.is_empty() {
        return format!("✅ **{title}** is done.\n\nProject folder: {repo}");
    }
    if let Some(f) = infos.iter().find(|a| a.status != "running") {
        let skip = f.log.len().saturating_sub(5);
        let tail = f.log[skip..].join("\n");
        return format!("⚠️ **{title}** is done but {} failed to start:\n\n```\n{tail}\n```", f.name);
    }
    let lines: Vec<String> = infos.iter().map(|a| format!("- {}: {}", a.name, a.url)).collect();
    format!("✅ **{title}** is done and running:\n\n{}", lines.join("\n"))
}

/// Removes the task from the running set when dropped, even on panic.
struct RunGuard(Runs, i64);

impl Drop for RunGuard {
    fn drop(&mut self) {
        if let Ok(mut map) = self.0.lock() {
            map.remove(&self.1);
        }
    }
}

fn spawn_turn<F, Fut>(
    db: &Arc<Db>,
    cfg: &Config,
    running: &Runs,
    live: &Live,
    task_id: i64,
    start_status: TaskStatus,
    turn: F,
) where
    F: FnOnce(Arc<Db>, Config, Live) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<()>> + Send + 'static,
{
    let Ok(mut map) = running.lock() else { return };
    if map.contains_key(&task_id) {
        return;
    }
    if let Err(e) = db.set_task_status(task_id, start_status) {
        report_error(db, Some(task_id), &e);
        return;
    }
    let (db, cfg) = (db.clone(), cfg.clone());
    let guard = RunGuard(running.clone(), task_id);
    let live_guard = LiveGuard(live.clone(), LiveKey::Task(task_id));
    let live = live.clone();
    let handle = tokio::spawn(async move {
        let _guard = (guard, live_guard);
        if let Err(e) = turn(db.clone(), cfg, live).await {
            report_error(&db, Some(task_id), &e);
            let _ = db.set_task_status(task_id, TaskStatus::Stuck);
        }
    });
    // Inserted while the lock is held, so the guard cannot remove it first.
    map.insert(task_id, handle.abort_handle());
}

/// Turns a parsed line into an event kind and JSON payload; None for live-only lines.
pub fn line_event(p: &ParsedLine) -> Option<(&'static str, Value)> {
    Some(match p {
        ParsedLine::Stream(_) => return None,
        ParsedLine::Init { raw, .. } => ("init", raw.clone()),
        ParsedLine::Assistant(v) => ("assistant", v.clone()),
        ParsedLine::ToolResult(v) => ("toolresult", v.clone()),
        ParsedLine::Result { raw, .. } => ("result", raw.clone()),
        ParsedLine::Other(v) => ("other", v.clone()),
    })
}

/// Expands a leading `~/` to the home directory.
pub fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(path),
    }
}

fn client_for(db: &Db, task: &Task) -> Result<Client> {
    db.get_client(task.client_id)?.context("client not found")
}

/// Runs the agent, logging every line as an event for `agent`.
async fn run_logged(db: &Db, live: &Live, agent: &str, task_id: i64, run: &AgentRun) -> Result<RunOutcome> {
    run_agent(run, |p| {
        on_line(live, LiveKey::Task(task_id), p);
        let Some((kind, payload)) = line_event(p) else { return };
        let _ = db.add_event(Some(task_id), agent, kind, &payload);
    })
    .await
}

fn lead_prompt(task: &Task, client: &Client, answers: &[(String, String)]) -> String {
    if answers.is_empty() {
        return format!(
            "Client: {}\nTask id: {}\nTitle: {}\n\n{}\n\nClient test command: {}",
            client.name,
            task.id,
            task.title,
            task.body,
            client.test_command.as_deref().unwrap_or("(none)")
        );
    }
    let pairs: Vec<String> = answers.iter().map(|(q, a)| format!("Q: {q}\nA: {a}")).collect();
    format!("The owner answered:\n\n{}\n\nContinue: split or ask more.", pairs.join("\n\n"))
}

/// Text block listing owner instructions, or an empty string when there are none.
fn instructions_block(items: &[(i64, String)]) -> String {
    if items.is_empty() {
        return String::new();
    }
    let lines: Vec<String> = items.iter().map(|(_, t)| format!("- {t}")).collect();
    format!("The owner sent new instructions:\n{}\n\n", lines.join("\n"))
}

/// Takes the pending instructions and marks them delivered before the agent runs.
fn take_instructions(db: &Db, task_id: i64) -> Result<String> {
    let pending = db.pending_instructions(task_id)?;
    let Some((last_id, _)) = pending.last() else { return Ok(String::new()) };
    db.add_event(Some(task_id), "owner", "instruction_delivered", &json!({"upto": last_id}))?;
    Ok(instructions_block(&pending))
}

async fn lead_turn(db: &Db, cfg: &Config, live: &Live, task_id: i64, answers: Vec<(String, String)>) -> Result<()> {
    let task = db.get_task(task_id)?.context("task not found")?;
    let client = client_for(db, &task)?;
    let block = take_instructions(db, task_id)?;
    let run = AgentRun {
        role: Role::Lead,
        task_id: Some(task_id),
        cwd: expand_home(&client.repo_path),
        prompt: format!("{block}{}", lead_prompt(&task, &client, &answers)),
        system_prompt: LEAD_PROMPT.into(),
        allowed_tools: LEAD_TOOLS.split(',').map(String::from).collect(),
        builtin_tools: None,
        permission_mode: None,
        mcp_url: format!("http://127.0.0.1:{}/mcp/lead", cfg.port),
        mcp_headers: vec![("X-Rui-Task-Id".into(), task_id.to_string())],
        resume: db.get_session(Role::Lead, Some(task_id))?,
        partial: true,
    };
    match run_logged(db, live, "lead", task_id, &run).await {
        Ok(out) => {
            if let Some(id) = out.session_id {
                db.set_session(Role::Lead, Some(task_id), &id)?;
            }
        }
        Err(e) => return agent_failed(db, task_id, &e),
    }
    after_lead_turn(db, task_id)?;
    Ok(())
}

fn agent_failed(db: &Db, task_id: i64, e: &anyhow::Error) -> Result<()> {
    db.add_event(Some(task_id), "scheduler", "agent_error", &json!({"error": format!("{e:#}")}))?;
    db.set_task_status(task_id, TaskStatus::Stuck)
}

fn builder_tools(client: &Client, done_command: &str) -> Vec<String> {
    let base = "Read,Edit,Write,Glob,Grep,mcp__rui__report_progress,mcp__rui__report_stuck";
    let mut tools: Vec<String> = base.split(',').map(String::from).collect();
    let cmds = client.allowed_commands.iter().map(String::as_str).chain([done_command]);
    tools.extend(cmds.map(|c| format!("Bash({c})")));
    tools
}

/// Extra Bash permissions a skill grants.
fn skill_tools(skill: &Skill) -> Vec<String> {
    skill.bash.iter().map(|p| format!("Bash({p})")).collect()
}

/// System prompt and allowed tools for a builder, including its skill if it has one.
fn builder_setup(db: &Db, cfg: &Config, task: &Task, client: &Client, done: &str) -> Result<(String, Vec<String>)> {
    let mut tools = builder_tools(client, done);
    let Some(name) = &task.skill else { return Ok((BUILDER_PROMPT.into(), tools)) };
    let Some(skill) = skills::load(&cfg.data_dir, name) else {
        db.add_event(Some(task.id), "scheduler", "skill_missing", &json!({"skill": name}))?;
        return Ok((BUILDER_PROMPT.into(), tools));
    };
    tools.extend(skill_tools(&skill));
    Ok((format!("{BUILDER_PROMPT}\n\n# Skill: {name}\n{}", skill.body), tools))
}

fn builder_prompt(task: &Task, done_command: &str, last_output: Option<&str>) -> String {
    let mut p = format!(
        "Subtask: {}\n\n{}\n\nDone command: {}\nThis is loop {} of {}.",
        task.title,
        task.body,
        done_command,
        task.loops_used + 1,
        task.max_loops
    );
    if let Some(out) = last_output {
        p.push_str(&format!("\n\nOutput of the last done check:\n{out}"));
    }
    p
}

/// On daemon start: orphaned in_progress tasks go back to open, clarifying ones to new.
pub fn recover_on_start(db: &Db) -> Result<usize> {
    let moves = [
        (TaskStatus::InProgress, TaskStatus::Open),
        (TaskStatus::Clarifying, TaskStatus::New),
    ];
    let mut count = 0;
    for (from, to) in moves {
        for t in db.tasks_with_status(from)? {
            db.set_task_status(t.id, to)?;
            let payload = json!({"from": from.as_str(), "to": to.as_str()});
            db.add_event(Some(t.id), "scheduler", "recovered", &payload)?;
            count += 1;
        }
    }
    Ok(count)
}

async fn builder_turn(db: &Db, cfg: &Config, live: &Live, task_id: i64) -> Result<()> {
    let task = db.get_task(task_id)?.context("task not found")?;
    let done_command = task.done_command.clone().context("subtask has no done_command")?;
    let client = client_for(db, &task)?;
    let repo = expand_home(&client.repo_path);
    let (wt, branch) = create_worktree(&repo, &cfg.data_dir.join("worktrees"), task_id, None)?;
    db.set_task_branch(task_id, &branch)?;
    let last = last_done_output(db, task_id)?;
    let block = take_instructions(db, task_id)?;
    let (system_prompt, allowed_tools) = builder_setup(db, cfg, &task, &client, &done_command)?;
    let resume = if block.is_empty() { None } else { db.get_session(Role::Builder, Some(task_id))? };
    let run = AgentRun {
        role: Role::Builder,
        task_id: Some(task_id),
        cwd: wt.clone(),
        prompt: format!("{block}{}", builder_prompt(&task, &done_command, last.as_deref())),
        system_prompt,
        allowed_tools,
        builtin_tools: None,
        permission_mode: Some("acceptEdits".into()),
        mcp_url: format!("http://127.0.0.1:{}/mcp/builder", cfg.port),
        mcp_headers: vec![("X-Rui-Task-Id".into(), task_id.to_string())],
        resume,
        partial: true,
    };
    match run_logged(db, live, "builder", task_id, &run).await {
        Ok(out) => {
            if let Some(id) = out.session_id {
                db.set_session(Role::Builder, Some(task_id), &id)?;
            }
        }
        Err(e) => {
            db.add_event(Some(task_id), "scheduler", "agent_error", &json!({"error": format!("{e:#}")}))?;
        }
    }
    finish_builder(db, &task, &wt, &done_command).await
}

async fn finish_builder(db: &Db, task: &Task, wt: &std::path::Path, done_command: &str) -> Result<()> {
    let current = db.get_task(task.id)?.context("task not found")?;
    if current.status == TaskStatus::Stuck {
        return Ok(());
    }
    let check = run_done_command(wt, done_command).await?;
    if after_done_check(db, task.id, &check)? != TaskStatus::Done {
        return Ok(());
    }
    let committed = commit_all(wt, &format!("rui: task {}: {}", task.id, task.title))?;
    db.add_event(Some(task.id), "scheduler", "commit", &json!({"committed": committed}))?;
    if !committed {
        return Ok(());
    }
    land_branch(db, task)
}

/// Merges the task branch into main; a conflict turns the finished task Stuck.
fn land_branch(db: &Db, task: &Task) -> Result<()> {
    let client = client_for(db, task)?;
    let branch = crate::worktree::branch_name(task.id);
    let message = format!("rui: task {}: {}", task.id, task.title);
    match land(&expand_home(&client.repo_path), &branch, &message)? {
        LandResult::Merged => {
            db.add_event(Some(task.id), "scheduler", "landed", &json!({"branch": branch}))?;
        }
        LandResult::Conflict(files) => {
            db.add_event(Some(task.id), "scheduler", "land_failed", &json!({"files": files}))?;
            db.set_task_status(task.id, TaskStatus::Stuck)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_line_event_keeps_raw_fields() {
        let line = r#"{"type":"system","subtype":"init","session_id":"s","model":"claude-x","apiKeySource":"none"}"#;
        let p = crate::runner::parse_line(line).unwrap();
        let (kind, payload) = line_event(&p).unwrap();
        assert_eq!(kind, "init");
        assert_eq!(payload["model"], "claude-x");
        assert_eq!(payload["apiKeySource"], "none");
        assert_eq!(payload["session_id"], "s");
    }
    #[test]
    fn stream_lines_are_not_stored() {
        let line = r#"{"type":"stream_event","event":{"type":"message_start"}}"#;
        let p = crate::runner::parse_line(line).unwrap();
        assert!(line_event(&p).is_none());
    }
    use crate::db::NewTask;
    use rui_core::SourceKind;

    #[test]
    fn instructions_block_formats_items() {
        assert_eq!(instructions_block(&[]), "");
        let items = vec![(1, "a".to_string()), (2, "b".to_string())];
        assert_eq!(instructions_block(&items), "The owner sent new instructions:\n- a\n- b\n\n");
    }

    fn mk(db: &Db, client: i64, parent: Option<i64>, status: TaskStatus) -> Task {
        db.create_task(&NewTask {
            client_id: client,
            parent_id: parent,
            title: "t".into(),
            body: String::new(),
            status,
            done_command: Some("true".into()),
            max_loops: 2,
            role: Role::Lead,
            source_kind: SourceKind::User,
            source_ref: None,
            skill: None,
        })
        .unwrap()
    }

    fn setup() -> (Db, i64) {
        let db = Db::open_test();
        let c = db.create_client("acme", "/repo", None, &[]).unwrap();
        (db, c.id)
    }

    fn plan(db: &Db) -> Vec<Action> {
        plan_tick(db, &HashSet::new()).unwrap()
    }

    #[test]
    fn recover_on_start_resets_orphans() {
        let (db, c) = setup();
        let parent = mk(&db, c, None, TaskStatus::Split);
        let sub = mk(&db, c, Some(parent.id), TaskStatus::InProgress);
        let clar = mk(&db, c, None, TaskStatus::Clarifying);
        let done = mk(&db, c, None, TaskStatus::Done);
        assert_eq!(recover_on_start(&db).unwrap(), 2);
        let status = |id| db.get_task(id).unwrap().unwrap().status;
        assert_eq!(status(sub.id), TaskStatus::Open);
        assert_eq!(status(clar.id), TaskStatus::New);
        assert_eq!(status(parent.id), TaskStatus::Split);
        assert_eq!(status(done.id), TaskStatus::Done);
        let evs = db.list_events(Some(sub.id), 10).unwrap();
        assert!(evs.iter().any(|e| e.kind == "recovered"));
        assert_eq!(recover_on_start(&db).unwrap(), 0);
    }

    #[test]
    fn new_task_starts_lead_unless_running() {
        let (db, c) = setup();
        let t = mk(&db, c, None, TaskStatus::New);
        assert_eq!(plan(&db), vec![Action::StartLead { task_id: t.id, answers: vec![] }]);
        let running = HashSet::from([t.id]);
        assert!(plan_tick(&db, &running).unwrap().is_empty());
    }

    #[test]
    fn waiting_answer_needs_all_answers() {
        let (db, c) = setup();
        let t = mk(&db, c, None, TaskStatus::WaitingAnswer);
        assert!(plan(&db).is_empty());
        let q1 = db.create_question(t.id, Role::Lead, "q1").unwrap();
        let q2 = db.create_question(t.id, Role::Lead, "q2").unwrap();
        db.answer_question(q1.id, "a1").unwrap();
        assert!(plan(&db).is_empty());
        db.answer_question(q2.id, "a2").unwrap();
        let answers = vec![("q1".into(), "a1".into()), ("q2".into(), "a2".into())];
        assert_eq!(plan(&db), vec![Action::StartLead { task_id: t.id, answers }]);
    }

    #[test]
    fn builder_waits_for_deps_and_split_parent() {
        let (db, c) = setup();
        let parent = mk(&db, c, None, TaskStatus::Split);
        let a = mk(&db, c, Some(parent.id), TaskStatus::Open);
        let b = mk(&db, c, Some(parent.id), TaskStatus::Open);
        db.add_task_dep(b.id, a.id).unwrap();
        assert_eq!(plan(&db), vec![Action::StartBuilder { task_id: a.id }]);
        db.set_task_status(a.id, TaskStatus::Done).unwrap();
        assert_eq!(plan(&db), vec![Action::StartBuilder { task_id: b.id }]);
    }

    #[test]
    fn open_subtask_under_non_split_parent_waits() {
        let (db, c) = setup();
        let parent = mk(&db, c, None, TaskStatus::Clarifying);
        mk(&db, c, Some(parent.id), TaskStatus::Open);
        assert!(plan(&db).is_empty());
    }

    #[test]
    fn rollup_done_and_stuck() {
        let (db, c) = setup();
        let p = mk(&db, c, None, TaskStatus::Split);
        mk(&db, c, Some(p.id), TaskStatus::Done);
        let b = mk(&db, c, Some(p.id), TaskStatus::InProgress);
        assert!(plan(&db).is_empty());
        db.set_task_status(b.id, TaskStatus::Stuck).unwrap();
        assert_eq!(plan(&db), vec![Action::Rollup { task_id: p.id, status: TaskStatus::Stuck }]);
        db.set_task_status(b.id, TaskStatus::Done).unwrap();
        assert_eq!(plan(&db), vec![Action::Rollup { task_id: p.id, status: TaskStatus::Done }]);
    }

    #[test]
    fn lead_turn_outcomes() {
        let (db, c) = setup();
        let t = mk(&db, c, None, TaskStatus::Clarifying);
        assert_eq!(after_lead_turn(&db, t.id).unwrap(), TaskStatus::Stuck);
        assert_eq!(db.list_events(Some(t.id), 10).unwrap()[0].kind, "lead_no_outcome");
        let w = mk(&db, c, None, TaskStatus::WaitingAnswer);
        assert_eq!(after_lead_turn(&db, w.id).unwrap(), TaskStatus::WaitingAnswer);
    }

    fn check(passed: bool) -> DoneCheck {
        DoneCheck { passed, output_tail: "out".into() }
    }

    #[test]
    fn done_check_outcomes() {
        let (db, c) = setup();
        let t = mk(&db, c, Some(1), TaskStatus::InProgress);
        assert_eq!(after_done_check(&db, t.id, &check(true)).unwrap(), TaskStatus::Done);
        let f = mk(&db, c, Some(1), TaskStatus::InProgress);
        assert_eq!(after_done_check(&db, f.id, &check(false)).unwrap(), TaskStatus::Open);
        assert_eq!(db.get_task(f.id).unwrap().unwrap().loops_used, 1);
        assert_eq!(after_done_check(&db, f.id, &check(false)).unwrap(), TaskStatus::Stuck);
        let s = mk(&db, c, Some(1), TaskStatus::Stuck);
        assert_eq!(after_done_check(&db, s.id, &check(true)).unwrap(), TaskStatus::Stuck);
    }

    #[test]
    fn last_output_is_newest() {
        let (db, c) = setup();
        let t = mk(&db, c, Some(1), TaskStatus::InProgress);
        assert_eq!(last_done_output(&db, t.id).unwrap(), None);
        for out in ["first", "second"] {
            let d = DoneCheck { passed: false, output_tail: out.into() };
            after_done_check(&db, t.id, &d).unwrap();
            db.set_task_status(t.id, TaskStatus::InProgress).unwrap();
        }
        assert_eq!(last_done_output(&db, t.id).unwrap().as_deref(), Some("second"));
    }

    #[test]
    fn rui_parent_in_split_behaves_like_lead_parent() {
        let (db, c) = setup();
        let parent = db
            .create_task(&NewTask {
                client_id: c,
                parent_id: None,
                title: "p".into(),
                body: String::new(),
                status: TaskStatus::Split,
                done_command: None,
                max_loops: 5,
                role: Role::Rui,
                source_kind: SourceKind::Chat,
                source_ref: None,
                skill: None,
            })
            .unwrap();
        let sub = mk(&db, c, Some(parent.id), TaskStatus::Open);
        assert_eq!(plan(&db), vec![Action::StartBuilder { task_id: sub.id }]);
        db.set_task_status(sub.id, TaskStatus::Done).unwrap();
        assert_eq!(plan(&db), vec![Action::Rollup { task_id: parent.id, status: TaskStatus::Done }]);
    }

    #[test]
    fn skill_tools_wrap_patterns() {
        let s = skills::parse("---\nname: x\ndescription: d\nbash: [\"uv *\", \"curl *\"]\n---\nb\n").unwrap();
        assert_eq!(skill_tools(&s), ["Bash(uv *)", "Bash(curl *)"]);
    }
}

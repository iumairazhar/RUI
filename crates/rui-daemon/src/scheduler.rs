//! The scheduler: plain rules that decide what runs next, plus the code that runs it.

use crate::apps::{self, AppInfo, Apps};
use crate::db::Db;
use crate::live::{Live, LiveGuard, LiveKey, on_line};
use crate::runner::{AgentRun, ParsedLine, RunOutcome, run_agent};
use crate::roster;
use crate::skills::{self, Skill};
use crate::worktree::{DoneCheck, LandResult, commit_all, create_worktree, diff_vs_main, land, run_done_command};
use anyhow::{Context, Result};
use rui_core::{ChatRole, Client, Question, QuestionStatus, Role, Rung, SourceKind, Task, TaskStatus};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const LEAD_PROMPT: &str = include_str!("../prompts/lead.md");
const BUILDER_PROMPT: &str = include_str!("../prompts/builder.md");
const DECIDE_PROMPT: &str = include_str!("../prompts/decide.md");
const LEAD_MCP: &str = "mcp__rui__create_subtask,mcp__rui__mark_split,mcp__rui__rule,mcp__rui__escalate,\
mcp__rui__set_limit,mcp__rui__set_done_command";
const MEMBER_MCP: &str = "mcp__rui__report_progress,mcp__rui__report_status,mcp__rui__ask_lead";
const RUI_DECIDE_TOOLS: [&str; 2] = ["mcp__rui__rule", "mcp__rui__escalate_to_owner"];
const READ_ONLY_TOOLS: [&str; 3] = ["Read", "Glob", "Grep"];

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
    /// Run a Lead or Rui decision turn for an open question at `level`.
    Decide { question_id: i64, level: String },
}

// ---- pure decision layer ----

/// Looks at the database and returns what should happen now, skipping running tasks.
/// Decision turns are tracked in `running` under the key `-question_id`.
pub fn plan_tick(db: &Db, running: &HashSet<i64>) -> Result<Vec<Action>> {
    let tasks = db.list_tasks()?;
    let by_id: HashMap<i64, &Task> = tasks.iter().map(|t| (t.id, t)).collect();
    let mut actions = Vec::new();
    for t in tasks.iter().filter(|t| !running.contains(&t.id)) {
        if let Some(a) = plan_task(db, t, &by_id)? {
            actions.push(a);
        }
    }
    actions.extend(plan_decisions(db, running, &by_id)?);
    Ok(actions)
}

/// One decision turn per open lead or rui question whose member is still waiting.
fn plan_decisions(db: &Db, running: &HashSet<i64>, by_id: &HashMap<i64, &Task>) -> Result<Vec<Action>> {
    let open = db.list_questions(Some(QuestionStatus::Open))?;
    let waiting = |q: &Question| by_id.get(&q.task_id).is_some_and(|t| t.status == TaskStatus::WaitingAnswer);
    Ok(open
        .into_iter()
        .filter(|q| q.level != "owner" && !running.contains(&-q.id) && waiting(q))
        .map(|q| Action::Decide { question_id: q.id, level: q.level })
        .collect())
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

/// Records the outcome of a done-command run as an event.
fn record_check(db: &Db, task_id: i64, check: &DoneCheck) -> Result<()> {
    let payload = json!({"passed": check.passed, "output_tail": check.output_tail});
    db.add_event(Some(task_id), "scheduler", "done_check", &payload)?;
    Ok(())
}

/// After a done-command run: decide done, open again, or ask the Lead at the loop limit.
pub fn after_done_check(db: &Db, data_dir: &Path, task_id: i64, check: &DoneCheck) -> Result<TaskStatus> {
    record_check(db, task_id, check)?;
    let task = db.get_task(task_id)?.context("task not found")?;
    if task.status == TaskStatus::Stuck {
        return Ok(TaskStatus::Stuck);
    }
    let next = if check.passed {
        TaskStatus::Done
    } else {
        let reason = format!("last check still fails: {}", check.output_tail);
        count_failed_loop(db, data_dir, &task, &reason)?
    };
    db.set_task_status(task_id, next)?;
    Ok(next)
}

/// Counts a failed attempt. Open to try again; at the loop limit the member waits on a Lead question.
fn count_failed_loop(db: &Db, data_dir: &Path, task: &Task, reason: &str) -> Result<TaskStatus> {
    let used = db.increment_loops(task.id)?;
    if used < task.max_loops {
        return Ok(TaskStatus::Open);
    }
    let text = format!(
        "Loop limit {} reached; {reason}. Raise the limit, narrow the criteria, or reassign?",
        task.max_loops
    );
    raise_lead_question(db, data_dir, task, &text)?;
    Ok(TaskStatus::WaitingAnswer)
}

/// Output tail of the most recent done check, if it failed.
pub fn last_done_output(db: &Db, task_id: i64) -> Result<Option<String>> {
    let events = db.list_events(Some(task_id), 1000)?;
    let last = events.iter().rev().find(|e| e.kind == "done_check");
    let failed = last.filter(|e| e.payload.get("passed").and_then(Value::as_bool) != Some(true));
    Ok(failed.and_then(|e| e.payload.get("output_tail")?.as_str().map(String::from)))
}

// ---- the question ladder ----

/// Opens a lead-level question for a member and makes the member wait for it.
pub fn raise_lead_question(db: &Db, data_dir: &Path, task: &Task, text: &str) -> Result<Question> {
    let title = roster::title_of(data_dir, task.agent.as_deref().unwrap_or("builder"));
    let ladder = [Rung { who: format!("{title} {}", task.id), why: text.into() }];
    let q = db.create_ladder_question(task.id, Role::Builder, "lead", &ladder, text)?;
    db.set_task_status(task.id, TaskStatus::WaitingAnswer)?;
    let payload = json!({"question_id": q.id, "level": "lead"});
    db.add_event(Some(task.id), "scheduler", "question_raised", &payload)?;
    Ok(q)
}

/// Stores an answer. For a member's question it also becomes an instruction and the member reopens.
pub fn answer_question(db: &Db, id: i64, answer: &str, why: Option<&str>, by: &str) -> Result<Question> {
    let q = db.get_question(id)?.context("question not found")?;
    if q.status == QuestionStatus::Answered {
        anyhow::bail!("question {id} is already answered");
    }
    let q = db.answer_question(id, answer, why)?.context("question not found")?;
    deliver_answer(db, &q, by)?;
    Ok(q)
}

fn deliver_answer(db: &Db, q: &Question, by: &str) -> Result<()> {
    let Some(task) = db.get_task(q.task_id)? else { return Ok(()) };
    if task.parent_id.is_none() {
        return answer_stuck_question(db, &task, q, by);
    }
    let answer = q.answer.as_deref().unwrap_or_default();
    let mut text = format!("Answer to your question \"{}\": {answer}", q.text);
    if let Some(why) = &q.why {
        text.push_str(&format!("\nWhy: {why}"));
    }
    db.add_event(Some(task.id), by, "instruction", &json!({"text": text}))?;
    if task.status == TaskStatus::WaitingAnswer {
        db.set_task_status(task.id, TaskStatus::Open)?;
    }
    Ok(())
}

/// The owner's reply to a "stopped" question on a top-level task: cancel leaves it stuck,
/// retry reopens the stuck subtasks, anything else goes to the Lead as an instruction.
fn answer_stuck_question(db: &Db, task: &Task, q: &Question, by: &str) -> Result<()> {
    if task.status != TaskStatus::Stuck || !q.text.contains(STOPPED_MARK) {
        return Ok(());
    }
    let answer = q.answer.as_deref().unwrap_or_default().trim();
    let word = answer.to_lowercase();
    if word.starts_with("cancel") {
        db.add_event(Some(task.id), by, "cancelled", &json!({"question_id": q.id}))?;
        return Ok(());
    }
    if word.starts_with("retry") {
        for sub in db.subtasks(task.id)?.iter().filter(|s| s.status == TaskStatus::Stuck) {
            db.set_task_status(sub.id, TaskStatus::Open)?;
        }
        let has_subs = !db.subtasks(task.id)?.is_empty();
        return db.set_task_status(task.id, if has_subs { TaskStatus::Split } else { TaskStatus::New });
    }
    db.add_event(Some(task.id), by, "instruction", &json!({"text": answer}))?;
    db.set_task_status(task.id, TaskStatus::New)
}

/// Passes an open question up one level: lead to rui, rui to owner.
pub fn escalate_question(db: &Db, id: i64, who: &str, why: &str) -> Result<Question> {
    let q = db.get_question(id)?.context("question not found")?;
    if q.status == QuestionStatus::Answered {
        anyhow::bail!("question {id} is already answered");
    }
    let next = match q.level.as_str() {
        "lead" => "rui",
        "rui" => "owner",
        _ => anyhow::bail!("question {id} is already with the owner"),
    };
    let mut ladder = q.ladder.clone();
    ladder.push(Rung { who: who.into(), why: why.into() });
    db.set_question_level(id, next, &ladder)?;
    let payload = json!({"question_id": id, "from": q.level, "to": next, "why": why});
    db.add_event(Some(q.task_id), "scheduler", "escalated", &payload)?;
    db.get_question(id)?.context("question not found")
}

/// After a decision turn: a question still open at `level` gets escalated with "no ruling".
pub fn settle_decision(db: &Db, id: i64, level: &str) -> Result<()> {
    let Some(q) = db.get_question(id)? else { return Ok(()) };
    if q.status != QuestionStatus::Open || q.level != level {
        return Ok(());
    }
    let who = if level == "lead" { "Lead" } else { "Rui" };
    escalate_question(db, id, who, "no ruling")?;
    Ok(())
}

// ---- effect layer ----

/// Abort handles of the turns currently running, keyed by task id (decision turns: `-question_id`).
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
            } else if status == TaskStatus::Stuck {
                let reason = stuck_reason(db, task_id).ok().flatten();
                if let Some(r) = reason {
                    let _ = raise_stuck_question(db, task_id, &r);
                }
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
        Action::Decide { question_id, level } => spawn_decision(db, cfg, running, live, question_id, level),
    }
}

/// Runs one decision turn in the background; at most one per question.
fn spawn_decision(db: &Arc<Db>, cfg: &Config, running: &Runs, live: &Live, qid: i64, level: String) {
    let key = -qid;
    let Ok(mut map) = running.lock() else { return };
    if map.contains_key(&key) {
        return;
    }
    let (db, cfg, live) = (db.clone(), cfg.clone(), live.clone());
    let guard = RunGuard(running.clone(), key);
    let handle = tokio::spawn(async move {
        let _guard = guard;
        if let Err(e) = decide(&db, &cfg, &live, qid, &level).await {
            report_error(&db, None, &e);
        }
        if let Err(e) = settle_decision(&db, qid, &level) {
            report_error(&db, None, &e);
        }
    });
    map.insert(key, handle.abort_handle());
}

/// After a top-level chat task is done, starts its project's apps and tells the chat.
fn launch_apps(db: &Arc<Db>, apps: &Apps, task_id: i64) {
    let Ok(Some(task)) = db.get_task(task_id) else { return };
    if task.parent_id.is_some() || task.source_kind != SourceKind::Chat {
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
    post_done_notice(db, task, &client.repo_path, &infos)
}

/// Posts the done message into the chat of the message the task came from (`chat:<message id>`).
fn post_done_notice(db: &Db, task: &Task, repo: &str, infos: &[AppInfo]) -> Result<()> {
    let Some(source) = task.source_ref.as_deref() else { return Ok(()) };
    let Some(chat_id) = db.chat_id_for_source_ref(source)? else { return Ok(()) };
    db.add_chat_message(chat_id, ChatRole::Rui, &done_message(&task.title, repo, infos))?;
    Ok(())
}

const STOPPED_MARK: &str = " stopped: ";

/// For a top-level task that got stuck by an error, asks the owner (shows in the Inbox) what to do.
/// Does nothing for subtasks, owner stops, or when such a question is already open.
fn raise_stuck_question(db: &Db, task_id: i64, reason: &str) -> Result<()> {
    let Some(task) = db.get_task(task_id)? else { return Ok(()) };
    if task.parent_id.is_some() {
        return Ok(());
    }
    let open = db.questions_for_task(task_id)?;
    if open.iter().any(|q| q.status == QuestionStatus::Open && q.text.contains(STOPPED_MARK)) {
        return Ok(());
    }
    let reason: String = reason.lines().next().unwrap_or_default().chars().take(200).collect();
    let text = format!("{}{STOPPED_MARK}{reason}. Retry, give instructions, or cancel?", task.title);
    let ladder = [Rung { who: "Scheduler".into(), why: reason }];
    let q = db.create_ladder_question(task_id, Role::Lead, "owner", &ladder, &text)?;
    db.add_event(Some(task_id), "scheduler", "question_raised", &json!({"question_id": q.id, "level": "owner"}))?;
    Ok(())
}

/// Why a stuck parent's subtasks stopped, when it was an error or a land conflict (not a stop or loop limit).
fn stuck_reason(db: &Db, parent_id: i64) -> Result<Option<String>> {
    for sub in db.subtasks(parent_id)?.iter().filter(|s| s.status == TaskStatus::Stuck) {
        let events = db.list_events(Some(sub.id), 30)?;
        let last = events.iter().rev().find(|e| ["agent_error", "scheduler_error", "land_failed", "stopped"].contains(&e.kind.as_str()));
        let Some(e) = last else { continue };
        match e.kind.as_str() {
            "land_failed" => return Ok(Some(format!("{} could not be merged (conflict in {})", sub.title, e.payload["files"]))),
            "stopped" => {}
            _ => return Ok(Some(format!("{}: {}", sub.title, e.payload["error"].as_str().unwrap_or("error")))),
        }
    }
    Ok(None)
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
            let _ = raise_stuck_question(&db, task_id, &format!("{e:#}"));
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
        let members: Vec<&str> = task.pod.iter().map(String::as_str).filter(|p| *p != "lead").collect();
        return format!(
            "Client: {}\nTask id: {}\nTitle: {}\n\n{}\n\nClient test command: {}\nYour pod (create subtasks only for these agents): {}",
            client.name,
            task.id,
            task.title,
            task.body,
            client.test_command.as_deref().unwrap_or("(none)"),
            if members.is_empty() { "(any role)".into() } else { members.join(", ") }
        );
    }
    let pairs: Vec<String> = answers.iter().map(|(q, a)| format!("Q: {q}\nA: {a}")).collect();
    format!("The owner answered:\n\n{}\n\nContinue: split the task.", pairs.join("\n\n"))
}

/// When the owner redirected a task that already has subtasks, lists them so the Lead can adjust them.
fn subtasks_note(db: &Db, task: &Task, instructions: &str) -> Result<String> {
    let subs = db.subtasks(task.id)?;
    if instructions.is_empty() || subs.is_empty() {
        return Ok(String::new());
    }
    let lines: Vec<String> = subs
        .iter()
        .map(|s| format!("- #{} {} [{}] ({})", s.id, s.title, s.status.as_str(), s.agent.as_deref().unwrap_or("any")))
        .collect();
    let rule = "Re-plan: adjust or add subtasks (set_done_command, set_loops, create_subtask) to match the new instructions, then call mark_split again. Do not duplicate finished work.";
    Ok(format!("\n\nExisting subtasks:\n{}\n\n{rule}", lines.join("\n")))
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

/// The role's AGENT.md body followed by the role prompt.
fn role_prompt(role: Option<&roster::Role>, prompt: &str) -> String {
    match role {
        Some(r) => format!("{}\n\n{prompt}", r.body),
        None => prompt.into(),
    }
}

/// Role tools plus MCP tools for a Lead, or read-only tools when the role is missing.
fn lead_tools(role: Option<&roster::Role>) -> Vec<String> {
    let mut tools: Vec<String> = LEAD_MCP.split(',').map(String::from).collect();
    match role {
        Some(r) => tools.extend(roster::cli_tools(r)),
        None => tools.extend(READ_ONLY_TOOLS.map(String::from)),
    }
    tools
}

/// Installs the role's skills plus the client's attached ones in `wt`, logs one `skill_used` event
/// per installed skill, and returns the installed names.
fn install_logged(db: &Db, cfg: &Config, wt: &Path, task: &Task, agent: &str, client: &str, role_skills: &[String]) -> Result<Vec<String>> {
    let mut names = role_skills.to_vec();
    names.extend(skills::client_skills(&cfg.data_dir, client));
    let names = skills::installable(&cfg.data_dir, &names);
    let mut unique: Vec<String> = Vec::new();
    for n in names {
        if !unique.contains(&n) {
            unique.push(n);
        }
    }
    skills::install_skills(wt, &cfg.data_dir, &unique)?;
    for n in &unique {
        db.add_event(Some(task.id), agent, "skill_used", &json!({"skill": n}))?;
    }
    Ok(unique)
}

/// Builds the Lead's run for `task` (a parent task) in the client's repo, with the role's skills installed.
fn lead_run(db: &Db, cfg: &Config, task: &Task, client: &Client, prompt: String) -> Result<AgentRun> {
    let role = roster::load(&cfg.data_dir, "lead");
    let repo = expand_home(&client.repo_path);
    let names = role.as_ref().map(|r| r.skills.clone()).unwrap_or_default();
    install_logged(db, cfg, &repo, task, "lead", &client.name, &names)?;
    Ok(AgentRun {
        role: Role::Lead,
        task_id: Some(task.id),
        cwd: repo,
        prompt,
        system_prompt: role_prompt(role.as_ref(), LEAD_PROMPT),
        allowed_tools: lead_tools(role.as_ref()),
        builtin_tools: None,
        permission_mode: None,
        mcp_url: format!("http://127.0.0.1:{}/mcp/lead", cfg.port),
        mcp_headers: vec![("X-Rui-Task-Id".into(), task.id.to_string())],
        resume: db.get_session(Role::Lead, Some(task.id))?,
        partial: true,
    })
}

async fn lead_turn(db: &Db, cfg: &Config, live: &Live, task_id: i64, answers: Vec<(String, String)>) -> Result<()> {
    let task = db.get_task(task_id)?.context("task not found")?;
    let client = client_for(db, &task)?;
    let block = take_instructions(db, task_id)?;
    let prompt = format!("{block}{}{}", lead_prompt(&task, &client, &answers), subtasks_note(db, &task, &block)?);
    let run = lead_run(db, cfg, &task, &client, prompt)?;
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
    db.set_task_status(task_id, TaskStatus::Stuck)?;
    raise_stuck_question(db, task_id, &format!("{e:#}"))
}

/// The prompt a Lead or Rui gets to decide one escalated question.
pub fn decision_prompt(q: &Question, member: &Task, parent: &Task, last_check: Option<&str>, escalate_tool: &str) -> String {
    let ladder: Vec<String> = q.ladder.iter().map(|r| format!("- {}: {}", r.who, r.why)).collect();
    format!(
        "Question {id} from member task {mid} ({title}):\n{text}\n\nLadder so far:\n{ladder}\n\n\
         Member job: {job}\nDone command: {done}\nLoops: {used} of {max}\n\
         Output of the member's last check:\n{check}\n\n\
         Task brief: {brief}\nPod: {pod}\n\n\
         Decide it: call rule(question_id={id}, answer, why). If you cannot, call {escalate_tool}(question_id={id}, why). \
         You must call one of them.",
        id = q.id,
        mid = member.id,
        title = member.title,
        text = q.text,
        ladder = ladder.join("\n"),
        job = member.body,
        done = member.done_command.as_deref().unwrap_or("(none)"),
        used = member.loops_used,
        max = member.max_loops,
        check = last_check.unwrap_or("(none)"),
        brief = parent.body,
        pod = parent.pod.join(", "),
    )
}

/// The member, its parent task and its client for a question.
fn question_context(db: &Db, q: &Question) -> Result<(Task, Task, Client)> {
    let member = db.get_task(q.task_id)?.context("question task not found")?;
    let parent_id = member.parent_id.context("question task has no parent")?;
    let parent = db.get_task(parent_id)?.context("parent task not found")?;
    let client = client_for(db, &parent)?;
    Ok((member, parent, client))
}

/// Runs a Lead (level "lead") or Rui (any other level) decision turn for one question.
async fn decide(db: &Db, cfg: &Config, live: &Live, qid: i64, level: &str) -> Result<()> {
    let q = db.get_question(qid)?.context("question not found")?;
    let (member, parent, client) = question_context(db, &q)?;
    let _live = LiveGuard(live.clone(), LiveKey::Task(parent.id));
    let last = last_done_output(db, member.id)?;
    let tool = if level == "lead" { "escalate" } else { "escalate_to_owner" };
    let prompt = decision_prompt(&q, &member, &parent, last.as_deref(), tool);
    if level == "lead" {
        let run = lead_run(db, cfg, &parent, &client, prompt)?;
        let out = run_logged(db, live, "lead", parent.id, &run).await?;
        if let Some(id) = out.session_id {
            db.set_session(Role::Lead, Some(parent.id), &id)?;
        }
        return Ok(());
    }
    run_logged(db, live, "rui", parent.id, &rui_decision_run(cfg, &parent, prompt)).await?;
    Ok(())
}

/// Headless Rui run: no chat transcript, only the question and the rule/escalate tools.
fn rui_decision_run(cfg: &Config, parent: &Task, prompt: String) -> AgentRun {
    AgentRun {
        role: Role::Rui,
        task_id: Some(parent.id),
        cwd: cfg.data_dir.clone(),
        prompt,
        system_prompt: DECIDE_PROMPT.into(),
        allowed_tools: RUI_DECIDE_TOOLS.map(String::from).to_vec(),
        builtin_tools: Some(String::new()),
        permission_mode: None,
        mcp_url: format!("http://127.0.0.1:{}/mcp/rui", cfg.port),
        mcp_headers: vec![],
        resume: None,
        partial: true,
    }
}

/// Extra Bash permissions a skill grants.
fn skill_tools(skill: &Skill) -> Vec<String> {
    skill.bash.iter().map(|p| format!("Bash({p})")).collect()
}

/// Tools for a member: its role's tools plus MCP tools. Bash is broad only with Shell;
/// otherwise just the done command, the client's allowed commands and the skills' patterns.
pub fn member_tools(role: Option<&roster::Role>, skills: &[Skill], client: &Client, done: &str) -> Vec<String> {
    let mut tools: Vec<String> = MEMBER_MCP.split(',').map(String::from).collect();
    let base: Vec<String> = match role {
        Some(r) => roster::cli_tools(r),
        None => ["Read", "Glob", "Grep", "Edit", "Write"].map(String::from).to_vec(),
    };
    let shell = base.iter().any(|t| t == "Bash");
    tools.extend(base);
    if shell {
        return tools;
    }
    let cmds = client.allowed_commands.iter().map(String::as_str).chain([done]);
    tools.extend(cmds.map(|c| format!("Bash({c})")));
    tools.extend(skills.iter().flat_map(skill_tools));
    tools
}

/// System prompt and allowed tools for a member; installs the role's skills in its worktree.
fn member_setup(db: &Db, cfg: &Config, task: &Task, client: &Client, done: &str, wt: &Path) -> Result<(String, Vec<String>)> {
    let role = task.agent.as_deref().and_then(|a| roster::load(&cfg.data_dir, a));
    if task.agent.is_some() && role.is_none() {
        let payload = json!({"agent": task.agent});
        db.add_event(Some(task.id), "scheduler", "role_missing", &payload)?;
    }
    let names = role.as_ref().map(|r| r.skills.clone()).unwrap_or_default();
    let agent = task.agent.as_deref().unwrap_or("builder");
    let names = install_logged(db, cfg, wt, task, agent, &client.name, &names)?;
    let loaded: Vec<Skill> = names.iter().filter_map(|n| skills::load(&cfg.data_dir, n)).collect();
    let tools = member_tools(role.as_ref(), &loaded, client, done);
    Ok((role_prompt(role.as_ref(), BUILDER_PROMPT), tools))
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
    let (system_prompt, allowed_tools) = member_setup(db, cfg, &task, &client, &done_command, &wt)?;
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
    finish_builder(db, cfg, live, &task, &wt, &done_command).await
}

async fn finish_builder(db: &Db, cfg: &Config, live: &Live, task: &Task, wt: &Path, done_command: &str) -> Result<()> {
    let current = db.get_task(task.id)?.context("task not found")?;
    if matches!(current.status, TaskStatus::Stuck | TaskStatus::WaitingAnswer) {
        return Ok(());
    }
    let check = run_done_command(wt, done_command).await?;
    if check.passed && wants_evaluator(db, &current)? {
        record_check(db, task.id, &check)?;
        return commit_evaluate_land(db, cfg, live, &current, wt, done_command).await;
    }
    if after_done_check(db, &cfg.data_dir, task.id, &check)? != TaskStatus::Done {
        return Ok(());
    }
    if commit_task(db, task, wt)? {
        land_branch(db, task)?;
    }
    Ok(())
}

/// Commits the worktree; false when there was nothing to commit.
fn commit_task(db: &Db, task: &Task, wt: &Path) -> Result<bool> {
    let committed = commit_all(wt, &format!("rui: task {}: {}", task.id, task.title))?;
    db.add_event(Some(task.id), "scheduler", "commit", &json!({"committed": committed}))?;
    Ok(committed)
}

/// True when the member's parent pod has an evaluator.
fn wants_evaluator(db: &Db, task: &Task) -> Result<bool> {
    let Some(pid) = task.parent_id else { return Ok(false) };
    let parent = db.get_task(pid)?;
    Ok(parent.is_some_and(|p| p.pod.iter().any(|r| r == "evaluator")))
}

/// Check passed: commit, let the evaluator review, then land on PASS or send back on FAIL.
async fn commit_evaluate_land(db: &Db, cfg: &Config, live: &Live, task: &Task, wt: &Path, done: &str) -> Result<()> {
    if !commit_task(db, task, wt)? {
        return db.set_task_status(task.id, TaskStatus::Done);
    }
    let text = evaluator_review(db, cfg, live, task, wt, done).await?;
    if parse_verdict(&text) == Some(Verdict::Pass) {
        db.set_task_status(task.id, TaskStatus::Done)?;
        return land_branch(db, task);
    }
    let findings = if parse_verdict(&text).is_some() { text } else { format!("No verdict was given.\n{text}") };
    send_back(db, &cfg.data_dir, task.id, &findings)?;
    Ok(())
}

/// The evaluator's final verdict.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Verdict {
    Pass,
    Fail,
}

/// Reads `VERDICT: PASS` or `VERDICT: FAIL` from the last line that has one.
pub fn parse_verdict(text: &str) -> Option<Verdict> {
    let plain = |c: char| matches!(c, '*' | '`' | '_' | ' ');
    text.lines().rev().find_map(|line| {
        let line = line.trim().trim_matches(plain).to_uppercase();
        match line.strip_prefix("VERDICT:")?.trim_matches(plain) {
            "PASS" => Some(Verdict::Pass),
            "FAIL" => Some(Verdict::Fail),
            _ => None,
        }
    })
}

fn evaluator_prompt(task: &Task, done: &str, diff: &str) -> String {
    format!(
        "Job:\n{}\n\nDone command: {done}\n\nReview this change (git diff main) against the job:\n```diff\n{diff}\n```\n\n\
         Reply with your findings. The final line must be exactly `VERDICT: PASS` or `VERDICT: FAIL`.",
        task.body
    )
}

/// Tools for the evaluator: its role's tools without Edit or Write.
fn evaluator_tools(role: Option<&roster::Role>) -> Vec<String> {
    let tools = match role {
        Some(r) => roster::cli_tools(r),
        None => READ_ONLY_TOOLS.map(String::from).to_vec(),
    };
    tools.into_iter().filter(|t| t != "Edit" && t != "Write").collect()
}

/// Runs the evaluator in the member's worktree and returns its final text.
async fn evaluator_review(db: &Db, cfg: &Config, live: &Live, task: &Task, wt: &Path, done: &str) -> Result<String> {
    let role = roster::load(&cfg.data_dir, "evaluator");
    let names = role.as_ref().map(|r| r.skills.clone()).unwrap_or_default();
    let client = client_for(db, task)?;
    install_logged(db, cfg, wt, task, "evaluator", &client.name, &names)?;
    let run = AgentRun {
        role: Role::Builder,
        task_id: Some(task.id),
        cwd: wt.to_path_buf(),
        prompt: evaluator_prompt(task, done, &diff_vs_main(wt)?),
        system_prompt: role.as_ref().map(|r| r.body.clone()).unwrap_or_default(),
        allowed_tools: evaluator_tools(role.as_ref()),
        builtin_tools: None,
        permission_mode: None,
        mcp_url: format!("http://127.0.0.1:{}/mcp/builder", cfg.port),
        mcp_headers: vec![("X-Rui-Task-Id".into(), task.id.to_string())],
        resume: None,
        partial: true,
    };
    let out = run_logged(db, live, "evaluator", task.id, &run).await?;
    Ok(out.result_text.unwrap_or_default())
}

/// Evaluator said FAIL: record the findings as an instruction, count a loop, reopen the member.
pub fn send_back(db: &Db, data_dir: &Path, task_id: i64, findings: &str) -> Result<TaskStatus> {
    db.add_event(Some(task_id), "evaluator", "sent_back", &json!({"findings": findings}))?;
    let text = format!("The evaluator sent your work back:\n{findings}");
    db.add_event(Some(task_id), "evaluator", "instruction", &json!({"text": text}))?;
    let task = db.get_task(task_id)?.context("task not found")?;
    let reason = format!("the evaluator still rejects it: {findings}");
    let next = count_failed_loop(db, data_dir, &task, &reason)?;
    db.set_task_status(task_id, next)?;
    Ok(next)
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
            agent: None,
            pod: vec![],
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
        db.answer_question(q1.id, "a1", None).unwrap();
        assert!(plan(&db).is_empty());
        db.answer_question(q2.id, "a2", None).unwrap();
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

    fn dd() -> PathBuf {
        std::env::temp_dir().join("rui-sched-test-none")
    }

    #[test]
    fn done_check_outcomes() {
        let (db, c) = setup();
        let t = mk(&db, c, Some(1), TaskStatus::InProgress);
        assert_eq!(after_done_check(&db, &dd(), t.id, &check(true)).unwrap(), TaskStatus::Done);
        let f = mk(&db, c, Some(1), TaskStatus::InProgress);
        assert_eq!(after_done_check(&db, &dd(), f.id, &check(false)).unwrap(), TaskStatus::Open);
        assert_eq!(db.get_task(f.id).unwrap().unwrap().loops_used, 1);
        let s = mk(&db, c, Some(1), TaskStatus::Stuck);
        assert_eq!(after_done_check(&db, &dd(), s.id, &check(true)).unwrap(), TaskStatus::Stuck);
    }

    #[test]
    fn loop_limit_raises_a_lead_question_instead_of_stuck() {
        let (db, c) = setup();
        let f = mk(&db, c, Some(1), TaskStatus::InProgress);
        assert_eq!(after_done_check(&db, &dd(), f.id, &check(false)).unwrap(), TaskStatus::Open);
        db.set_task_status(f.id, TaskStatus::InProgress).unwrap();
        assert_eq!(after_done_check(&db, &dd(), f.id, &check(false)).unwrap(), TaskStatus::WaitingAnswer);
        assert_eq!(db.get_task(f.id).unwrap().unwrap().status, TaskStatus::WaitingAnswer);
        let q = &db.questions_for_task(f.id).unwrap()[0];
        assert_eq!(q.level, "lead");
        assert_eq!(
            q.text,
            "Loop limit 2 reached; last check still fails: out. Raise the limit, narrow the criteria, or reassign?"
        );
        assert_eq!(q.ladder.len(), 1);
        assert!(q.ladder[0].who.ends_with(&f.id.to_string()));
    }

    #[test]
    fn raised_limit_ruling_lets_the_member_continue() {
        let (db, c) = setup();
        let f = mk(&db, c, Some(1), TaskStatus::InProgress);
        db.set_max_loops(f.id, 1).unwrap();
        after_done_check(&db, &dd(), f.id, &check(false)).unwrap();
        let q = db.questions_for_task(f.id).unwrap().remove(0);
        db.set_max_loops(f.id, 4).unwrap();
        answer_question(&db, q.id, "raised to 4", Some("flaky"), "lead").unwrap();
        let t = db.get_task(f.id).unwrap().unwrap();
        assert_eq!((t.status, t.max_loops), (TaskStatus::Open, 4));
        assert_eq!(db.pending_instructions(f.id).unwrap().len(), 1);
    }

    #[test]
    fn last_output_is_newest_failure() {
        let (db, c) = setup();
        let t = mk(&db, c, Some(1), TaskStatus::InProgress);
        assert_eq!(last_done_output(&db, t.id).unwrap(), None);
        for out in ["first", "second"] {
            let d = DoneCheck { passed: false, output_tail: out.into() };
            after_done_check(&db, &dd(), t.id, &d).unwrap();
            db.set_task_status(t.id, TaskStatus::InProgress).unwrap();
        }
        assert_eq!(last_done_output(&db, t.id).unwrap().as_deref(), Some("second"));
        after_done_check(&db, &dd(), t.id, &check(true)).unwrap();
        assert_eq!(last_done_output(&db, t.id).unwrap(), None);
    }

    /// A parent, a member waiting on an open question at `level`; returns (member id, question id).
    fn waiting_member(db: &Db, c: i64, level: &str) -> (i64, i64) {
        let parent = mk(db, c, None, TaskStatus::Split);
        let m = mk(db, c, Some(parent.id), TaskStatus::WaitingAnswer);
        let q = db.create_ladder_question(m.id, Role::Builder, level, &[], "q?").unwrap();
        (m.id, q.id)
    }

    #[test]
    fn open_lead_and_rui_questions_get_decision_turns() {
        let (db, c) = setup();
        let (_, lead_q) = waiting_member(&db, c, "lead");
        let (_, rui_q) = waiting_member(&db, c, "rui");
        waiting_member(&db, c, "owner");
        let want = vec![
            Action::Decide { question_id: lead_q, level: "lead".into() },
            Action::Decide { question_id: rui_q, level: "rui".into() },
        ];
        assert_eq!(plan(&db), want);
        let running = HashSet::from([-lead_q]);
        assert_eq!(plan_tick(&db, &running).unwrap(), want[1..]);
    }

    #[test]
    fn decisions_skip_members_that_are_not_waiting() {
        let (db, c) = setup();
        let (m, _) = waiting_member(&db, c, "lead");
        db.set_task_status(m, TaskStatus::Stuck).unwrap();
        assert!(plan(&db).iter().all(|a| !matches!(a, Action::Decide { .. })));
    }

    #[test]
    fn escalate_walks_lead_rui_owner() {
        let (db, c) = setup();
        let (m, q) = waiting_member(&db, c, "lead");
        let up = escalate_question(&db, q, "Lead", "unsure").unwrap();
        assert_eq!((up.level.as_str(), up.ladder.len()), ("rui", 1));
        let up = escalate_question(&db, q, "Rui", "owner call").unwrap();
        assert_eq!((up.level.as_str(), up.ladder[1].who.as_str()), ("owner", "Rui"));
        assert!(escalate_question(&db, q, "Rui", "x").is_err());
        assert_eq!(db.get_task(m).unwrap().unwrap().status, TaskStatus::WaitingAnswer);
        assert!(db.list_events(Some(m), 10).unwrap().iter().any(|e| e.kind == "escalated"));
    }

    #[test]
    fn no_ruling_escalates_automatically() {
        let (db, c) = setup();
        let (_, q) = waiting_member(&db, c, "lead");
        settle_decision(&db, q, "lead").unwrap();
        let after = db.get_question(q).unwrap().unwrap();
        assert_eq!((after.level.as_str(), after.ladder[0].why.as_str()), ("rui", "no ruling"));
        settle_decision(&db, q, "lead").unwrap();
        assert_eq!(db.get_question(q).unwrap().unwrap().ladder.len(), 1);
        db.answer_question(q, "a", None).unwrap();
        settle_decision(&db, q, "rui").unwrap();
        assert_eq!(db.get_question(q).unwrap().unwrap().level, "rui");
    }

    #[test]
    fn owner_answer_flows_down_like_a_ruling() {
        let (db, c) = setup();
        let (m, q) = waiting_member(&db, c, "owner");
        answer_question(&db, q, "yes", Some("because"), "owner").unwrap();
        assert_eq!(db.get_task(m).unwrap().unwrap().status, TaskStatus::Open);
        let (_, text) = db.pending_instructions(m).unwrap().remove(0);
        assert!(text.contains("yes") && text.contains("because"));
        assert!(answer_question(&db, q, "again", None, "owner").is_err());
    }

    #[test]
    fn verdict_parsing() {
        assert_eq!(parse_verdict("ok\nVERDICT: PASS"), Some(Verdict::Pass));
        assert_eq!(parse_verdict("bad\n**VERDICT: FAIL**\n"), Some(Verdict::Fail));
        assert_eq!(parse_verdict("verdict: pass  "), Some(Verdict::Pass));
        assert_eq!(parse_verdict("VERDICT: PASS\nlater\nVERDICT: FAIL"), Some(Verdict::Fail));
        assert_eq!(parse_verdict("VERDICT: MAYBE"), None);
        assert_eq!(parse_verdict("all good"), None);
    }

    #[test]
    fn sent_back_records_findings_counts_a_loop_and_reopens() {
        let (db, c) = setup();
        let t = mk(&db, c, Some(1), TaskStatus::InProgress);
        assert_eq!(send_back(&db, &dd(), t.id, "missing tests").unwrap(), TaskStatus::Open);
        let now = db.get_task(t.id).unwrap().unwrap();
        assert_eq!((now.status, now.loops_used), (TaskStatus::Open, 1));
        let evs = db.list_events(Some(t.id), 10).unwrap();
        assert!(evs.iter().any(|e| e.kind == "sent_back"));
        assert!(db.pending_instructions(t.id).unwrap()[0].1.contains("missing tests"));
        assert_eq!(send_back(&db, &dd(), t.id, "still bad").unwrap(), TaskStatus::WaitingAnswer);
        assert_eq!(db.questions_for_task(t.id).unwrap()[0].level, "lead");
    }

    #[test]
    fn evaluator_is_wanted_only_when_in_the_pod() {
        let (db, c) = setup();
        let mut parent = mk(&db, c, None, TaskStatus::Split);
        let member = mk(&db, c, Some(parent.id), TaskStatus::InProgress);
        assert!(!wants_evaluator(&db, &member).unwrap());
        let with = db
            .create_task(&NewTask {
                client_id: c,
                parent_id: None,
                title: "p".into(),
                body: String::new(),
                status: TaskStatus::Split,
                done_command: None,
                max_loops: 5,
                role: Role::Lead,
                source_kind: SourceKind::User,
                source_ref: None,
                agent: Some("lead".into()),
                pod: vec!["lead".into(), "evaluator".into()],
            })
            .unwrap();
        parent = with;
        let member = mk(&db, c, Some(parent.id), TaskStatus::InProgress);
        assert!(wants_evaluator(&db, &member).unwrap());
    }

    fn role(tools: &[&str]) -> roster::Role {
        let text = format!(
            "---\nname: r\ndescription: d\ntools: {}\n---\nI am r.\n",
            serde_json::to_string(tools).unwrap()
        );
        roster::parse(&text).unwrap()
    }

    fn client_with(cmds: &[&str]) -> Client {
        let cmds: Vec<String> = cmds.iter().map(|c| c.to_string()).collect();
        Client { id: 1, name: "a".into(), repo_path: "/r".into(), test_command: None, allowed_commands: cmds, created_at: String::new() }
    }

    #[test]
    fn member_tools_allow_bash_broadly_only_with_shell() {
        let skill = skills::parse("---\nname: x\ndescription: d\nbash: [\"uv *\"]\n---\nb\n").unwrap();
        let client = client_with(&["ls"]);
        let no_shell = member_tools(Some(&role(&["Read", "Edit", "Run tests"])), &[skill.clone()], &client, "make t");
        assert!(no_shell.contains(&"Edit".to_string()) && no_shell.contains(&"Write".to_string()));
        assert!(!no_shell.contains(&"Bash".to_string()));
        for want in ["Bash(make t)", "Bash(ls)", "Bash(uv *)", "mcp__rui__ask_lead", "mcp__rui__report_status"] {
            assert!(no_shell.contains(&want.to_string()), "{want}");
        }
        assert!(!no_shell.iter().any(|t| t.contains("report_stuck")));
        let shell = member_tools(Some(&role(&["Shell"])), &[skill], &client, "make t");
        assert!(shell.contains(&"Bash".to_string()));
        assert!(!shell.iter().any(|t| t.starts_with("Bash(")));
    }

    #[test]
    fn evaluator_tools_are_read_only() {
        let tools = evaluator_tools(Some(&role(&["Read", "Edit", "Shell"])));
        assert_eq!(tools, ["Read", "Bash"]);
        assert_eq!(evaluator_tools(None), ["Read", "Glob", "Grep"]);
    }

    #[test]
    fn decision_prompt_has_question_ladder_and_check() {
        let (db, c) = setup();
        let (m, q) = waiting_member(&db, c, "lead");
        let ladder = [Rung { who: "Backend 7".into(), why: "which db?".into() }];
        db.set_question_level(q, "lead", &ladder).unwrap();
        let (q, member) = (db.get_question(q).unwrap().unwrap(), db.get_task(m).unwrap().unwrap());
        let parent = db.get_task(member.parent_id.unwrap()).unwrap().unwrap();
        let p = decision_prompt(&q, &member, &parent, Some("boom"), "escalate");
        for want in ["q?", "- Backend 7: which db?", "boom", "rule(question_id=", "escalate(question_id="] {
            assert!(p.contains(want), "{want}");
        }
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
                agent: None,
            pod: vec![],
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

    fn chat_task(db: &Db, source_ref: Option<String>) -> Task {
        let c = db.create_client("c", "/tmp/c", None, &[]).unwrap();
        db.create_task(&NewTask {
            client_id: c.id,
            parent_id: None,
            title: "Guide".into(),
            body: String::new(),
            status: TaskStatus::Done,
            done_command: None,
            max_loops: 5,
            role: Role::Lead,
            source_kind: SourceKind::Chat,
            source_ref,
            agent: Some("lead".into()),
            pod: vec![],
        })
        .unwrap()
    }

    #[test]
    fn done_notice_goes_to_the_chat_of_the_source_message() {
        let db = Db::open_test();
        let _first = db.create_chat().unwrap();
        let chat = db.create_chat().unwrap();
        for _ in 0..40 {
            db.add_chat_message(_first.id, ChatRole::User, "pad").unwrap();
        }
        let msg = db.add_chat_message(chat.id, ChatRole::User, "make it").unwrap();
        assert!(msg.id > chat.id, "message id differs from chat id");
        let task = chat_task(&db, Some(format!("chat:{}", msg.id)));
        post_done_notice(&db, &task, "/tmp/c", &[]).unwrap();
        let last = db.list_chat(chat.id, 10).unwrap().pop().unwrap();
        assert_eq!(last.role, ChatRole::Rui);
        assert!(last.content.contains("Guide"));
        let nothing = chat_task_ref_missing(&db);
        post_done_notice(&db, &nothing, "/tmp/c", &[]).unwrap();
    }

    fn chat_task_ref_missing(db: &Db) -> Task {
        let mut t = db.list_tasks().unwrap().pop().unwrap();
        t.source_ref = Some("chat:99999".into());
        t
    }

    #[test]
    fn stuck_top_level_task_asks_the_owner_once() {
        let db = Db::open_test();
        let t = chat_task(&db, None);
        db.set_task_status(t.id, TaskStatus::Stuck).unwrap();
        raise_stuck_question(&db, t.id, "claude failed\nstack").unwrap();
        raise_stuck_question(&db, t.id, "again").unwrap();
        let qs = db.questions_for_task(t.id).unwrap();
        assert_eq!(qs.len(), 1);
        assert_eq!(qs[0].level, "owner");
        assert_eq!(qs[0].text, "Guide stopped: claude failed. Retry, give instructions, or cancel?");
        let sub = mk(&db, t.client_id, Some(t.id), TaskStatus::Stuck);
        raise_stuck_question(&db, sub.id, "x").unwrap();
        assert!(db.questions_for_task(sub.id).unwrap().is_empty());
    }

    #[test]
    fn stuck_reason_ignores_owner_stops_and_reports_conflicts() {
        let db = Db::open_test();
        let t = chat_task(&db, None);
        db.set_task_status(t.id, TaskStatus::Split).unwrap();
        let sub = mk(&db, t.client_id, Some(t.id), TaskStatus::Stuck);
        assert_eq!(stuck_reason(&db, t.id).unwrap(), None);
        db.add_event(Some(sub.id), "owner", "stopped", &json!({})).unwrap();
        assert_eq!(stuck_reason(&db, t.id).unwrap(), None);
        db.add_event(Some(sub.id), "scheduler", "land_failed", &json!({"files": ["a.rs"]})).unwrap();
        assert!(stuck_reason(&db, t.id).unwrap().unwrap().contains("conflict"));
    }

    #[test]
    fn owner_answer_to_a_stuck_question_retries_or_redirects() {
        let db = Db::open_test();
        let t = chat_task(&db, None);
        db.set_task_status(t.id, TaskStatus::Stuck).unwrap();
        let sub = mk(&db, t.client_id, Some(t.id), TaskStatus::Stuck);
        raise_stuck_question(&db, t.id, "boom").unwrap();
        let q = db.questions_for_task(t.id).unwrap().pop().unwrap();
        answer_question(&db, q.id, "Retry", None, "owner").unwrap();
        let status = |id| db.get_task(id).unwrap().unwrap().status;
        assert_eq!((status(t.id), status(sub.id)), (TaskStatus::Split, TaskStatus::Open));
        db.set_task_status(t.id, TaskStatus::Stuck).unwrap();
        db.set_task_status(sub.id, TaskStatus::Stuck).unwrap();
        raise_stuck_question(&db, t.id, "boom").unwrap();
        let q = db.questions_for_task(t.id).unwrap().pop().unwrap();
        answer_question(&db, q.id, "only the API part", None, "owner").unwrap();
        assert_eq!(status(t.id), TaskStatus::New);
        assert_eq!(db.pending_instructions(t.id).unwrap()[0].1, "only the API part");
    }
}

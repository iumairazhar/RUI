//! The scheduler: plain rules that decide what runs next, plus the code that runs it.

use crate::apps::{self, AppInfo, Apps};
use crate::results::ResultFile;
use crate::db::Db;
use crate::client_state;
use crate::prompts;
use crate::protected;
use crate::live::{Live, LiveGuard, LiveKey, on_line};
use crate::runner::{AgentRun, Cli, ParsedLine, RunOutcome, run_agent_with};
use crate::roster;
use crate::seats;
use crate::skills::{self, Skill};
use crate::worktree::{DoneCheck, LandResult, commit_all, create_worktree, ensure_task_worktree, merge_into, run_done_command};
use anyhow::{Context, Result};
use rui_core::{ChatRole, Client, Question, QuestionStatus, Role, Rung, SourceKind, Task, TaskStatus};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const ACCOUNT_LEAD_PROMPT: &str = include_str!("../prompts/account_lead.md");
const BUILDER_PROMPT: &str = include_str!("../prompts/builder.md");
const DECIDE_PROMPT: &str = include_str!("../prompts/decide.md");
const LEAD_MCP: &str = "mcp__rui__create_subtask,mcp__rui__mark_split,mcp__rui__rule,mcp__rui__escalate,\
mcp__rui__set_limit,mcp__rui__set_done_command,mcp__rui__log_decision,mcp__rui__send_back,mcp__rui__forward_pr,\
mcp__rui__ask_scout";
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
    /// All subtasks are done: wake the Account Lead to review the work.
    Review { task_id: i64 },
    /// Run a Lead or Rui decision turn for an open question at `level`.
    Decide { question_id: i64, level: String },
    /// The owner's undo window ran out: deliver the held answer.
    DeliverAnswer { question_id: i64, event_id: i64 },
    /// Run the Scout once for an unanswered `scout_requested` event.
    Scout { task_id: i64, request_event_id: i64 },
    /// Run the Coach once over a finished top-level task.
    Coach { task_id: i64 },
}

// ---- pure decision layer ----

/// Looks at the database and returns what should happen now, skipping running tasks.
/// Decision turns are tracked in `running` under the key `-question_id`.
pub fn plan_tick(db: &Db, running: &HashSet<i64>) -> Result<Vec<Action>> {
    plan_tick_at(db, running, crate::thread::now_secs())
}

/// `plan_tick` with the clock passed in, so tests can move time.
pub fn plan_tick_at(db: &Db, running: &HashSet<i64>, now: i64) -> Result<Vec<Action>> {
    let tasks = db.list_tasks()?;
    let by_id: HashMap<i64, &Task> = tasks.iter().map(|t| (t.id, t)).collect();
    let pending = pending_approvals(db)?;
    let held: HashSet<i64> = pending.iter().map(approval_question).collect();
    let mut actions = Vec::new();
    for t in tasks.iter().filter(|t| !running.contains(&t.id)) {
        if let Some(a) = plan_task(db, t, &by_id, &held)? {
            actions.push(a);
        }
    }
    actions.extend(plan_decisions(db, running, &by_id)?);
    actions.extend(plan_deliveries(&pending, now));
    actions.extend(crate::scout::plan(db, running)?);
    actions.extend(crate::coach::plan(db, running)?);
    Ok(actions)
}

/// One delivery per held answer whose undo window has run out.
fn plan_deliveries(pending: &[rui_core::Event], now: i64) -> Vec<Action> {
    pending
        .iter()
        .filter(|e| approval_run_at(e) <= now)
        .map(|e| Action::DeliverAnswer { question_id: approval_question(e), event_id: e.id })
        .collect()
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

fn plan_task(db: &Db, t: &Task, by_id: &HashMap<i64, &Task>, held: &HashSet<i64>) -> Result<Option<Action>> {
    match (t.parent_id, t.status) {
        (None, TaskStatus::New) => Ok(Some(Action::StartLead { task_id: t.id, answers: vec![] })),
        (None, TaskStatus::WaitingAnswer) => plan_answered(db, t, held),
        (Some(p), TaskStatus::Open) => plan_builder(db, t, by_id.get(&p).copied(), by_id),
        (_, TaskStatus::Split) => plan_rollup(db, t),
        _ => Ok(None),
    }
}

/// Wakes the Lead once every question is answered. An answer still in its undo window (`held`) does not count.
fn plan_answered(db: &Db, t: &Task, held: &HashSet<i64>) -> Result<Option<Action>> {
    let qs = db.questions_for_task(t.id)?;
    if qs.is_empty() || qs.iter().any(|q| q.status != QuestionStatus::Answered || held.contains(&q.id)) {
        return Ok(None);
    }
    let answers = qs
        .into_iter()
        .filter_map(|q| q.answer.map(|a| (q.text, a)))
        .collect();
    Ok(Some(Action::StartLead { task_id: t.id, answers }))
}

/// Starts a ready subtask unless the owner holds it (a checkpoint or a takeover).
fn plan_builder(db: &Db, t: &Task, parent: Option<&Task>, by_id: &HashMap<i64, &Task>) -> Result<Option<Action>> {
    if owner_hold(db, t.id)?.is_some() {
        return Ok(None);
    }
    Ok(builder_ready(t, parent, by_id))
}

/// Event kinds that set or clear an owner hold on a subtask.
pub const HOLD_KINDS: [&str; 4] = ["checkpoint_set", "checkpoint_cleared", "taken_over", "handed_back"];

/// The hold the newest hold event puts on a task: "checkpoint", "takeover" or none.
pub fn hold_of<'a>(events: impl IntoIterator<Item = &'a rui_core::Event>) -> Option<&'static str> {
    let last = events.into_iter().filter(|e| HOLD_KINDS.contains(&e.kind.as_str())).max_by_key(|e| e.id)?;
    match last.kind.as_str() {
        "checkpoint_set" => Some("checkpoint"),
        "taken_over" => Some("takeover"),
        _ => None,
    }
}

/// Whether the owner holds a subtask: Some("checkpoint") or Some("takeover").
pub fn owner_hold(db: &Db, task_id: i64) -> Result<Option<&'static str>> {
    Ok(hold_of(&db.events_for_tasks(&[task_id], &HOLD_KINDS, 0, i64::MAX)?))
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
        if forwarded_since_done(db, t.id, &subs)? {
            return Ok(rollup(TaskStatus::Done));
        }
        return Ok(Some(Action::Review { task_id: t.id }));
    }
    if subs.iter().any(|s| s.status == TaskStatus::Stuck) {
        return Ok(rollup(TaskStatus::Stuck));
    }
    Ok(None)
}

/// True when the Account Lead forwarded the task after the last subtask finished, and after
/// any PR feedback was routed to it (so a Lead handling feedback is not closed early).
fn forwarded_since_done(db: &Db, task_id: i64, subs: &[Task]) -> Result<bool> {
    let Some(ev) = latest_event(db, task_id, "forwarded")? else { return Ok(false) };
    if latest_event(db, task_id, "pr_feedback_routed")?.is_some_and(|r| r.id > ev.id) {
        return Ok(false);
    }
    Ok(subs.iter().all(|s| ev.created_at.as_str() >= s.updated_at.as_str()))
}

fn latest_event(db: &Db, task_id: i64, kind: &str) -> Result<Option<rui_core::Event>> {
    Ok(db.list_events(Some(task_id), 100_000)?.into_iter().rev().find(|e| e.kind == kind))
}

/// Payload of the task's latest `forwarded` event: the Account Lead's remarks and PR url.
pub fn latest_forwarded(db: &Db, task_id: i64) -> Result<Option<Value>> {
    Ok(latest_event(db, task_id, "forwarded")?.map(|e| e.payload))
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
pub(crate) fn count_failed_loop(db: &Db, data_dir: &Path, task: &Task, reason: &str) -> Result<TaskStatus> {
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

/// Seconds the owner has to undo an answer before it takes effect.
pub const UNDO_SECS: i64 = 10;

const APPROVAL_KINDS: [&str; 3] = ["approval_pending", "approval_cancelled", "approval_delivered"];

/// A held owner answer: the question, the `approval_pending` event id and when it goes out.
pub struct HeldAnswer {
    pub question: Question,
    pub event_id: i64,
    pub run_at: i64,
}

/// Stores the owner's answer but holds delivery for `UNDO_SECS`, logging `approval_pending`.
pub fn answer_with_undo(db: &Db, id: i64, answer: &str, why: Option<&str>, now: i64) -> Result<HeldAnswer> {
    let q = db.get_question(id)?.context("question not found")?;
    if q.status == QuestionStatus::Answered {
        anyhow::bail!("question {id} is already answered");
    }
    let question = db.answer_question(id, answer, why)?.context("question not found")?;
    let run_at = now + UNDO_SECS;
    let payload = json!({"question_id": id, "run_at": run_at});
    let event_id = db.add_event(Some(question.task_id), "owner", "approval_pending", &payload)?;
    Ok(HeldAnswer { question, event_id, run_at })
}

/// `approval_pending` events not yet cancelled or delivered, oldest first.
pub fn pending_approvals(db: &Db) -> Result<Vec<rui_core::Event>> {
    let evs = db.events_of_kinds(&APPROVAL_KINDS)?;
    let closed: HashSet<i64> =
        evs.iter().filter(|e| e.kind != "approval_pending").filter_map(|e| e.payload["event_id"].as_i64()).collect();
    Ok(evs.into_iter().filter(|e| e.kind == "approval_pending" && !closed.contains(&e.id)).collect())
}

pub fn approval_run_at(e: &rui_core::Event) -> i64 {
    e.payload["run_at"].as_i64().unwrap_or(0)
}

pub fn approval_question(e: &rui_core::Event) -> i64 {
    e.payload["question_id"].as_i64().unwrap_or(0)
}

/// Held while an undo or a delivery checks and closes a pending answer, so the two never both win.
static APPROVAL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn approval_lock() -> std::sync::MutexGuard<'static, ()> {
    APPROVAL_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Delivers a held owner answer, marks it `approval_delivered`, then writes the decisions log.
/// Does nothing if the event is no longer pending (cancelled or delivered), checked under the lock.
pub fn deliver_held(db: &Db, data_dir: &Path, event_id: i64) -> Result<()> {
    let _lock = approval_lock();
    let Some(e) = pending_approvals(db)?.into_iter().find(|e| e.id == event_id) else { return Ok(()) };
    let q = db.get_question(approval_question(&e))?.context("question not found")?;
    deliver_answer(db, &q, "owner")?;
    db.add_event(e.task_id, "scheduler", "approval_delivered", &json!({"event_id": event_id}))?;
    log_owner_answer(db, data_dir, &q)
}

/// Undoes a held answer while its window is open: logs `approval_cancelled` and reopens the question.
/// None when the event is not pending any more (delivered, checked under the lock) or its window has run out.
pub fn cancel_held(db: &Db, event_id: i64, now: i64) -> Result<Option<Question>> {
    let _lock = approval_lock();
    let Some(e) = pending_approvals(db)?.into_iter().find(|e| e.id == event_id) else { return Ok(None) };
    if now >= approval_run_at(&e) {
        return Ok(None);
    }
    db.add_event(e.task_id, "owner", "approval_cancelled", &json!({"event_id": event_id}))?;
    db.reopen_question(approval_question(&e))
}

/// Writes the owner's answer to the client's decisions log, where the Account Lead reads it.
pub fn log_owner_answer(db: &Db, data_dir: &Path, q: &Question) -> Result<()> {
    let Some(task) = db.get_task(q.task_id)? else { return Ok(()) };
    let Some(client) = db.get_client(task.client_id)? else { return Ok(()) };
    let answer: String = q.answer.as_deref().unwrap_or_default().chars().take(300).collect();
    let line = format!("Owner answered Q{} (task {}): {answer}", q.id, task.id);
    client_state::append_decision(data_dir, &client.name, &line)
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
    if task.status != TaskStatus::WaitingAnswer {
        return Ok(());
    }
    // The Lead may have raised the limit before answering, so read the task again.
    let fresh = db.get_task(task.id)?.unwrap_or(task);
    if fresh.loops_used < fresh.max_loops {
        return db.set_task_status(fresh.id, TaskStatus::Open);
    }
    let payload = json!({"loops_used": fresh.loops_used, "max_loops": fresh.max_loops, "by": by});
    db.add_event(Some(fresh.id), by, "loop_limit", &payload)?;
    db.set_task_status(fresh.id, TaskStatus::Stuck)
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
        return retry_stuck(db, task.id, by).map(|_| ());
    }
    db.add_event(Some(task.id), by, "instruction", &json!({"text": answer}))?;
    db.set_task_status(task.id, TaskStatus::New)
}

/// Puts a stuck task back to work: its stuck subtasks reopen and the task itself goes back
/// to split (with subtasks), new (a Lead), or open (a member). Logs `resumed`. Returns the resumed ids.
pub fn retry_stuck(db: &Db, id: i64, by: &str) -> Result<Vec<i64>> {
    let task = db.get_task(id)?.context("task not found")?;
    if task.status != TaskStatus::Stuck {
        return Ok(vec![]);
    }
    let mut ids = vec![id];
    let mut raised = vec![];
    for sub in db.subtasks(id)?.iter().filter(|s| s.status == TaskStatus::Stuck) {
        if sub.loops_used >= sub.max_loops {
            db.set_max_loops(sub.id, sub.loops_used + 3)?;
            raised.push(sub.id);
        }
        db.set_task_status(sub.id, TaskStatus::Open)?;
        ids.push(sub.id);
    }
    let has_subs = !db.subtasks(id)?.is_empty();
    let next = match (task.role, has_subs) {
        (Role::Builder, _) => TaskStatus::Open,
        (_, true) => TaskStatus::Split,
        (_, false) => TaskStatus::New,
    };
    db.set_task_status(id, next)?;
    db.add_event(Some(id), by, "resumed", &json!({"ids": ids, "by": by, "raised_limit": raised}))?;
    Ok(ids)
}

/// Passes an open question up one level: lead to rui, rui to owner.
pub fn escalate_question(db: &Db, id: i64, who: &str, why: &str) -> Result<Question> {
    escalate_question_with(db, id, who, why, &json!({}))
}

/// Like `escalate_question`, merging the keys of `extra` (an object) into the `escalated` event.
pub fn escalate_question_with(db: &Db, id: i64, who: &str, why: &str, extra: &serde_json::Value) -> Result<Question> {
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
    let mut payload = json!({"question_id": id, "from": q.level, "to": next, "why": why});
    if let (Some(p), Some(e)) = (payload.as_object_mut(), extra.as_object()) {
        p.extend(e.clone());
    }
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

/// One lock per client, so only one Account Lead wake runs at a time for it.
pub type ClientLocks = Arc<Mutex<HashMap<i64, Arc<tokio::sync::Mutex<()>>>>>;

/// The client's wake lock, created on first use.
pub fn client_lock(locks: &ClientLocks, client_id: i64) -> Arc<tokio::sync::Mutex<()>> {
    let mut map = locks.lock().unwrap_or_else(|e| e.into_inner());
    map.entry(client_id).or_default().clone()
}

/// Runs forever: every tick, plan and execute. Never exits on error.
pub async fn run(db: Arc<Db>, cfg: Config, running: Runs, locks: ClientLocks, live: Live, apps: Apps) {
    let mut last_beat = std::time::Instant::now();
    loop {
        tokio::time::sleep(cfg.tick).await;
        let snapshot: HashSet<i64> =
            running.lock().map(|r| r.keys().copied().collect()).unwrap_or_default();
        if last_beat.elapsed() >= Duration::from_secs(crate::heartbeat::HEARTBEAT_SECS) {
            last_beat = std::time::Instant::now();
            beat(&db, &cfg, &running, &snapshot);
        }
        match plan_gated(&db, &cfg.data_dir, &snapshot, crate::thread::now_secs()) {
            Ok(actions) => {
                for a in actions {
                    execute(&db, &cfg, &running, &locks, &live, &apps, a);
                }
            }
            Err(e) => report_error(&db, None, &e),
        }
    }
}

/// `plan_tick_at`, then the seat gate: while every seat is used up, no new turn starts.
pub fn plan_gated(db: &Db, data_dir: &Path, running: &HashSet<i64>, now: i64) -> Result<Vec<Action>> {
    let actions = plan_tick_at(db, running, now)?;
    let free = seats::check_gate(db, data_dir, now)?;
    Ok(hold_turns(actions, free))
}

/// Drops every action that would start an agent turn unless a seat is free. Running turns are not touched.
pub fn hold_turns(actions: Vec<Action>, seat_free: bool) -> Vec<Action> {
    if seat_free {
        return actions;
    }
    let keep = |a: &Action| matches!(a, Action::Rollup { .. } | Action::DeliverAnswer { .. });
    actions.into_iter().filter(keep).collect()
}

/// Runs the heartbeat once; errors are logged. PR polling runs on a blocking thread.
fn beat(db: &Arc<Db>, cfg: &Config, running: &Runs, snapshot: &HashSet<i64>) {
    let now = crate::thread::now_secs();
    if let Err(e) = crate::heartbeat::heartbeat(db, &cfg.data_dir, running, snapshot, now) {
        report_error(db, None, &e);
    }
    inputs_beat(db, &cfg.data_dir, now);
    crate::pr_watch::spawn_poll(db.clone(), cfg.data_dir.clone(), running.clone());
}

/// Reads the drop folder, asks the Scout for briefs of meetings about to start and fires due
/// schedule triggers; errors are logged.
fn inputs_beat(db: &Db, data_dir: &Path, now: i64) {
    let res = crate::drop::scan(db, data_dir).and_then(|_| crate::meeting::prep_due(db, data_dir, now));
    if let Err(e) = res {
        report_error(db, None, &e);
    }
    let local = chrono::Local::now().naive_local();
    if let Err(e) = crate::triggers::fire_scheduled(db, data_dir, local) {
        report_error(db, None, &e);
    }
}

pub(crate) fn report_error(db: &Db, task_id: Option<i64>, e: &anyhow::Error) {
    eprintln!("scheduler error: {e:#}");
    let _ = db.add_event(task_id, "scheduler", "scheduler_error", &json!({"error": format!("{e:#}")}));
}

fn execute(db: &Arc<Db>, cfg: &Config, running: &Runs, locks: &ClientLocks, live: &Live, apps: &Apps, action: Action) {
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
            let lock = lock_for_task(db, locks, task_id);
            spawn_turn(db, cfg, running, live, task_id, TaskStatus::Clarifying, lock, move |db, cfg, live| async move {
                lead_turn(&db, &cfg, &live, task_id, answers).await
            });
        }
        Action::Review { task_id } => {
            let lock = lock_for_task(db, locks, task_id);
            spawn_turn(db, cfg, running, live, task_id, TaskStatus::Clarifying, lock, move |db, cfg, live| async move {
                review_turn(&db, &cfg, &live, task_id).await
            });
        }
        Action::StartBuilder { task_id } => {
            spawn_turn(db, cfg, running, live, task_id, TaskStatus::InProgress, None, move |db, cfg, live| async move {
                builder_turn(&db, &cfg, &live, task_id).await
            });
        }
        Action::Decide { question_id, level } => spawn_decision(db, cfg, running, locks, live, question_id, level),
        Action::DeliverAnswer { event_id, .. } => {
            if let Err(e) = deliver_held(db, &cfg.data_dir, event_id) {
                report_error(db, None, &e);
            }
        }
        Action::Scout { task_id, request_event_id } => {
            let (db2, cfg2, live2) = (db.clone(), cfg.clone(), live.clone());
            spawn_keyed(running, crate::scout::run_key(request_event_id), async move {
                crate::scout::run(&db2, &cfg2, &live2, task_id, request_event_id).await
            }, db, Some(task_id));
        }
        Action::Coach { task_id } => {
            let (db2, cfg2, live2) = (db.clone(), cfg.clone(), live.clone());
            spawn_keyed(running, crate::coach::run_key(task_id), async move {
                crate::coach::run(&db2, &cfg2, &live2, task_id).await
            }, db, Some(task_id));
        }
    }
}

/// Runs one background turn under `key` in the running set; at most one per key. Errors are logged.
fn spawn_keyed<Fut>(running: &Runs, key: i64, turn: Fut, db: &Arc<Db>, task_id: Option<i64>)
where
    Fut: std::future::Future<Output = Result<()>> + Send + 'static,
{
    let Ok(mut map) = running.lock() else { return };
    if map.contains_key(&key) {
        return;
    }
    let (db, guard) = (db.clone(), RunGuard(running.clone(), key));
    let handle = tokio::spawn(async move {
        let _guard = guard;
        if let Err(e) = turn.await {
            report_error(&db, task_id, &e);
        }
    });
    map.insert(key, handle.abort_handle());
}

/// Runs one decision turn in the background; at most one per question.
fn spawn_decision(db: &Arc<Db>, cfg: &Config, running: &Runs, locks: &ClientLocks, live: &Live, qid: i64, level: String) {
    let key = -qid;
    let Ok(mut map) = running.lock() else { return };
    if map.contains_key(&key) {
        return;
    }
    let (db, cfg, live) = (db.clone(), cfg.clone(), live.clone());
    let guard = RunGuard(running.clone(), key);
    let lock = if level == "lead" { question_client(&db, qid).map(|c| client_lock(locks, c)) } else { None };
    let handle = tokio::spawn(async move {
        let _guard = guard;
        let _wake = match &lock {
            Some(l) => Some(l.lock().await),
            None => None,
        };
        if let Err(e) = decide(&db, &cfg, &live, qid, &level).await {
            report_error(&db, None, &e);
        }
        if let Err(e) = settle_decision(&db, qid, &level) {
            report_error(&db, None, &e);
        }
    });
    map.insert(key, handle.abort_handle());
}

/// The client of the task a question belongs to.
fn question_client(db: &Db, qid: i64) -> Option<i64> {
    let q = db.get_question(qid).ok()??;
    Some(db.get_task(q.task_id).ok()??.client_id)
}

/// The client's wake lock for a task, if the task exists.
fn lock_for_task(db: &Db, locks: &ClientLocks, task_id: i64) -> Option<Arc<tokio::sync::Mutex<()>>> {
    let task = db.get_task(task_id).ok()??;
    Some(client_lock(locks, task.client_id))
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
    let files = crate::results::collect(db, task).unwrap_or_default();
    let text = format!("{}{}", done_message(task, repo, infos, &files), lead_note(db, task.id)?);
    db.add_chat_message(chat_id, ChatRole::Rui, &text)?;
    Ok(())
}

/// The Account Lead's remarks (and PR link) for a forwarded task, or an empty string.
fn lead_note(db: &Db, task_id: i64) -> Result<String> {
    let Some(f) = latest_forwarded(db, task_id)? else { return Ok(String::new()) };
    let mut note = format!("\n\n**Account Lead:** {}", f["remarks"].as_str().unwrap_or_default());
    if let Some(url) = f["pr_url"].as_str() {
        note.push_str(&format!("\n\nPR: {url}"));
    }
    Ok(note)
}

const STOPPED_MARK: &str = " stopped: ";

/// For a top-level task that got stuck by an error, asks the owner (shows in the Inbox) what to do.
/// Does nothing for subtasks, owner stops, or when such a question is already open.
pub(crate) fn raise_stuck_question(db: &Db, task_id: i64, reason: &str) -> Result<()> {
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
        let last = events.iter().rev().find(|e| ["agent_error", "scheduler_error", "land_failed", "stopped", "loop_limit"].contains(&e.kind.as_str()));
        let Some(e) = last else { continue };
        match e.kind.as_str() {
            "land_failed" => return Ok(Some(format!("{} could not be merged (conflict in {})", sub.title, e.payload["files"]))),
            "stopped" => {}
            "loop_limit" => return Ok(Some(loop_limit_reason(sub, e))),
            _ => return Ok(Some(format!("{}: {}", sub.title, e.payload["error"].as_str().unwrap_or("error")))),
        }
    }
    Ok(None)
}

fn loop_limit_reason(sub: &Task, e: &rui_core::Event) -> String {
    let (used, max) = (&e.payload["loops_used"], &e.payload["max_loops"]);
    format!("{} hit its loop limit ({used}/{max}) and was not given more attempts", sub.title)
}

/// The chat message announcing a finished task, its apps and the files it produced.
fn done_message(task: &Task, repo: &str, infos: &[AppInfo], files: &[ResultFile]) -> String {
    let base = apps_message(&task.title, repo, infos);
    if files.is_empty() {
        return base;
    }
    let lines: Vec<String> = files
        .iter()
        .take(8)
        .map(|f| format!("- [{}](rui://result/{}/{})", f.path, f.task_id, f.path))
        .collect();
    format!("{base}\n\n**Files:**\n{}", lines.join("\n"))
}

fn apps_message(title: &str, repo: &str, infos: &[AppInfo]) -> String {
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
    lock: Option<Arc<tokio::sync::Mutex<()>>>,
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
        let _wake = match &lock {
            Some(l) => Some(l.lock().await),
            None => None,
        };
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

/// Runs the agent on the picked seat, logging every line as an event for `agent`.
/// The `result` event carries `seat_id`, which is how usage per seat is counted.
pub(crate) async fn run_logged(db: &Db, cfg: &Config, live: &Live, agent: &str, task_id: i64, run: AgentRun) -> Result<RunOutcome> {
    run_logged_on(Cli::Claude, db, cfg, live, agent, task_id, run).await
}

/// `run_logged` on the given agent CLI.
async fn run_logged_on(cli: Cli, db: &Db, cfg: &Config, live: &Live, agent: &str, task_id: i64, mut run: AgentRun) -> Result<RunOutcome> {
    let seat = seats::pick_from(db, &cfg.data_dir, crate::thread::now_secs())?.context("all seats are used up")?;
    run.seat = Some(seat.clone());
    let res = run_agent_with(cli, &run, |p| {
        on_line(live, LiveKey::Task(task_id), p);
        let Some((kind, payload)) = line_event(p) else { return };
        let _ = db.add_event(Some(task_id), agent, kind, &tag_seat(kind, payload, &seat.id));
    })
    .await;
    if seat_used_up(cli, &res) {
        seats::mark_exhausted(db, &seat, crate::thread::now_secs())?;
    }
    res
}

/// True when a Claude turn failed on a usage limit. A Codex turn never uses up a Claude seat.
pub fn seat_used_up(cli: Cli, res: &Result<RunOutcome>) -> bool {
    cli == Cli::Claude && limit_text(res).is_some_and(|t| seats::hit_limit(&t))
}

/// A turn that ran over its time limit. The CLI is killed when its future is dropped.
#[derive(Debug)]
pub struct TimedOut;

impl std::fmt::Display for TimedOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("timed out")
    }
}

impl std::error::Error for TimedOut {}

/// Runs `fut` with a time limit; an error of type [`TimedOut`] when it runs over.
pub async fn within<T>(limit: std::time::Duration, fut: impl std::future::Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(limit, fut).await.map_err(|_| anyhow::Error::new(TimedOut))?
}

/// Adds `seat_id` to a `result` payload.
pub fn tag_seat(kind: &str, mut payload: Value, seat_id: &str) -> Value {
    if kind == "result" && payload.is_object() {
        payload["seat_id"] = json!(seat_id);
    }
    payload
}

/// The text of a failed turn: its error result or the run error. None for a good turn.
fn limit_text(res: &Result<RunOutcome>) -> Option<String> {
    match res {
        Ok(out) if out.is_error => out.result_text.clone(),
        Ok(_) => None,
        Err(e) => Some(format!("{e:#}")),
    }
}

/// The task itself, plus the owner's answers when this wake follows questions.
/// Every wake is a fresh session, so the task details are always included.
fn lead_prompt(task: &Task, client: &Client, answers: &[(String, String)]) -> String {
    let members: Vec<&str> = task.pod.iter().map(String::as_str).filter(|p| *p != "lead").collect();
    let base = format!(
        "Client: {}\nTask id: {}\nTitle: {}\n\n{}\n\nClient test command: {}\nPod roles for this task (create subtasks only for these agents): {}",
        client.name,
        task.id,
        task.title,
        task.body,
        client.test_command.as_deref().unwrap_or("(none)"),
        if members.is_empty() { "(any role)".into() } else { members.join(", ") }
    );
    if answers.is_empty() {
        return base;
    }
    let pairs: Vec<String> = answers.iter().map(|(q, a)| format!("Q: {q}\nA: {a}")).collect();
    format!("{base}\n\nThe owner answered your questions:\n\n{}\n\nContinue: split the task.", pairs.join("\n\n"))
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

/// Role tools plus MCP tools for an Account Lead. It never writes files, so Edit and Write are dropped.
fn account_lead_tools(role: Option<&roster::Role>) -> Vec<String> {
    let mut tools: Vec<String> = LEAD_MCP.split(',').map(String::from).collect();
    match role {
        Some(r) => tools.extend(roster::cli_tools(r)),
        None => tools.extend(READ_ONLY_TOOLS.map(String::from)),
    }
    tools.retain(|t| t != "Edit" && t != "Write");
    tools
}

/// Last `max` chars of `text`, cut at a char boundary, with a leading "…" when cut.
fn tail_chars(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let tail: String = text.chars().skip(count - max).collect();
    format!("\u{2026}{tail}")
}

/// One line per pod role: its title and its own skills plus the pod's extra ones.
fn pod_roles_and_skills(data_dir: &Path, client: &Client) -> Result<String> {
    let pod = client_state::read_pod(data_dir, &client.name)?;
    let mut lines = Vec::new();
    for pr in &pod.roles {
        let mut skills = roster::load(data_dir, &pr.role).map(|r| r.skills).unwrap_or_default();
        skills.extend(pr.skills.iter().cloned());
        let mut seen = HashSet::new();
        skills.retain(|s| seen.insert(s.clone()));
        let title = roster::title_of(data_dir, &pr.role);
        lines.push(format!("- {} ({title}): skills {}", pr.role, skills.join(", ")));
    }
    Ok(lines.join("\n"))
}

/// Values for account_lead.md: the client, why it woke, its briefs and log, playbook and pod.
fn account_lead_vars(data_dir: &Path, client: &Client, event: &str) -> Result<Vec<(&'static str, String)>> {
    let briefs = tail_chars(&client_state::read_briefs(data_dir, &client.name)?, 6000);
    let log = client_state::read_log(data_dir, &client.name, 60)?;
    Ok(vec![
        ("client", client.name.clone()),
        ("event", event.to_string()),
        ("client_log", format!("Briefs:\n{briefs}\n\nDecisions log (latest last):\n{log}")),
        ("owner_knowledge", client_state::read_knowledge(data_dir, &client.name)?),
        ("client_playbook", client_state::read_playbook(data_dir, &client.name)?),
        ("pod_roles_and_skills", pod_roles_and_skills(data_dir, client)?),
    ])
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

/// The top-level task's worktree and branch, cut from the client's base branch.
fn task_worktree(cfg: &Config, client: &Client, repo: &Path, task_id: i64) -> Result<(PathBuf, String)> {
    let base = client_state::base_for(&cfg.data_dir, &client.name, repo)?;
    ensure_task_worktree(repo, &cfg.data_dir.join("worktrees"), task_id, &base)
}

/// Builds an Account Lead wake for `task`: a fresh session in the task worktree, read-only tools.
fn account_lead_run(db: &Db, cfg: &Config, task: &Task, client: &Client, event: &str, prompt: String) -> Result<AgentRun> {
    let role = roster::load(&cfg.data_dir, "lead");
    let repo = expand_home(&client.repo_path);
    let (wt, branch) = task_worktree(cfg, &client, &repo, task.id)?;
    record_parent_branch(db, task.id, &branch)?;
    let names = role.as_ref().map(|r| r.skills.clone()).unwrap_or_default();
    install_logged(db, cfg, &wt, task, "lead", &client.name, &names)?;
    let filled = prompts::fill(ACCOUNT_LEAD_PROMPT, &account_lead_vars(&cfg.data_dir, client, event)?);
    Ok(AgentRun {
        role: Role::Lead,
        task_id: Some(task.id),
        cwd: wt,
        prompt,
        system_prompt: role_prompt(role.as_ref(), &filled),
        allowed_tools: account_lead_tools(role.as_ref()),
        builtin_tools: None,
        permission_mode: None,
        permission_prompt_tool: None,
        mcp_url: format!("http://127.0.0.1:{}/mcp/lead", cfg.port),
        mcp_headers: vec![("X-Rui-Task-Id".into(), task.id.to_string())],
        resume: None,
        partial: true,
        seat: None,
    })
}

async fn lead_turn(db: &Db, cfg: &Config, live: &Live, task_id: i64, answers: Vec<(String, String)>) -> Result<()> {
    let task = db.get_task(task_id)?.context("task not found")?;
    let client = client_for(db, &task)?;
    let block = take_instructions(db, task_id)?;
    let prompt = format!("{block}{}{}", lead_prompt(&task, &client, &answers), subtasks_note(db, &task, &block)?);
    let run = account_lead_run(db, cfg, &task, &client, "a new task", prompt)?;
    if let Err(e) = run_logged(db, cfg, live, "lead", task_id, run).await {
        return agent_failed(db, task_id, &e);
    }
    after_lead_turn(db, task_id)?;
    Ok(())
}

/// The Account Lead reviews a task whose subtasks are all done and calls send_back or forward_pr.
async fn review_turn(db: &Db, cfg: &Config, live: &Live, task_id: i64) -> Result<()> {
    let task = db.get_task(task_id)?.context("task not found")?;
    let client = client_for(db, &task)?;
    let prompt = review_prompt(db, &cfg.data_dir, &task, &client)?;
    let run = account_lead_run(db, cfg, &task, &client, "finished work to review", prompt)?;
    if let Err(e) = run_logged(db, cfg, live, "lead", task_id, run).await {
        return agent_failed(db, task_id, &e);
    }
    if after_lead_turn(db, task_id)? == TaskStatus::Stuck {
        raise_stuck_question(db, task_id, "the Account Lead finished its review without send_back or forward_pr")?;
    }
    Ok(())
}

fn review_prompt(db: &Db, data_dir: &Path, task: &Task, client: &Client) -> Result<String> {
    let lines: Vec<String> = db
        .subtasks(task.id)?
        .iter()
        .map(|s| {
            format!(
                "- #{} {} [{}] done_command: {}",
                s.id,
                s.title,
                s.agent.as_deref().unwrap_or("any"),
                s.done_command.as_deref().unwrap_or("(none)")
            )
        })
        .collect();
    let base = client_state::base_for(data_dir, &client.name, &expand_home(&client.repo_path)).unwrap_or_else(|_| "main".into());
    Ok(format!(
        "{}\n\nAll subtasks are done. Review the work on branch {} (compare with `git diff {base}...HEAD`).\nSubtasks:\n{}\n\nCall send_back(task_id={id}, remarks) or forward_pr(task_id={id}, remarks).",
        lead_prompt(task, client, &[]),
        crate::worktree::task_branch_name(task.id),
        lines.join("\n"),
        id = task.id,
    ))
}

fn agent_failed(db: &Db, task_id: i64, e: &anyhow::Error) -> Result<()> {
    db.add_event(Some(task_id), "scheduler", "agent_error", &json!({"error": format!("{e:#}")}))?;
    db.set_task_status(task_id, TaskStatus::Stuck)?;
    raise_stuck_question(db, task_id, &format!("{e:#}"))
}

/// The prompt a Lead or Rui gets to decide one escalated question.
pub fn decision_prompt(q: &Question, member: &Task, parent: &Task, last_check: Option<&str>, owner_answers: &str, escalate_tool: &str) -> String {
    let ladder: Vec<String> = q.ladder.iter().map(|r| format!("- {}: {}", r.who, r.why)).collect();
    format!(
        "Question {id} from member task {mid} ({title}):\n{text}\n\nLadder so far:\n{ladder}\n\n\
         Member job: {job}\nDone command: {done}\nLoops: {used} of {max}\n\
         Output of the member's last check:\n{check}\n\n\
         Task brief: {brief}\nPod: {pod}\n\n\
         Owner answers on this task:\n{owner}\n\n\
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
        owner = if owner_answers.is_empty() { "(none)" } else { owner_answers },
    )
}

/// Answered owner-level questions from all subtasks of `parent_id`, as "- Q<id>: <first line> \u{2192} <answer>".
fn owner_answers(db: &Db, parent_id: i64) -> Result<String> {
    let mut lines = Vec::new();
    for sub in db.subtasks(parent_id)? {
        for q in db.questions_for_task(sub.id)?.into_iter().filter(|q| q.level == "owner") {
            let Some(answer) = q.answer.as_deref() else { continue };
            let first = q.text.lines().next().unwrap_or_default();
            lines.push(format!("- Q{}: {first} \u{2192} {answer}", q.id));
        }
    }
    Ok(lines.join("\n"))
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
    let answers = owner_answers(db, parent.id)?;
    let prompt = decision_prompt(&q, &member, &parent, last.as_deref(), &answers, tool);
    if level == "lead" {
        let run = account_lead_run(db, cfg, &parent, &client, "a member's question", prompt)?;
        run_logged(db, cfg, live, "lead", parent.id, run).await?;
        return Ok(());
    }
    let playbook = client_state::read_playbook(&cfg.data_dir, &client.name)?;
    let mut vars = decide_vars(db, &q, &member, &parent, playbook)?;
    vars.push(("global_playbook", client_state::read_global_playbook(&cfg.data_dir)?));
    vars.push(("owner_knowledge", client_state::read_knowledge(&cfg.data_dir, &client.name)?));
    let system = prompts::fill(DECIDE_PROMPT, &vars);
    run_logged(db, cfg, live, "rui", parent.id, rui_decision_run(cfg, &parent, prompt, system)).await?;
    Ok(())
}

/// Values for decide.md: the question, who asked and why it came up, the brief, earlier rulings.
fn decide_vars(db: &Db, q: &Question, member: &Task, parent: &Task, client_playbook: String) -> Result<Vec<(&'static str, String)>> {
    let lead_why = q.ladder.last().map(|r| r.why.clone()).unwrap_or_default();
    Ok(vec![
        ("question", q.text.clone()),
        ("member_role", member.agent.clone().unwrap_or_else(|| "builder".into())),
        ("subtask_title", member.title.clone()),
        ("lead_why", lead_why),
        ("brief", parent.body.clone()),
        ("client_playbook", client_playbook),
        ("prior_rulings", prior_rulings(db, parent.id, q.id)?),
    ])
}

/// Answered questions from this task's members, other than `skip`, as "- question → answer".
fn prior_rulings(db: &Db, parent_id: i64, skip: i64) -> Result<String> {
    let mut lines = Vec::new();
    for sub in db.subtasks(parent_id)? {
        let answered = db.questions_for_task(sub.id)?.into_iter().filter(|x| x.id != skip);
        for x in answered.filter_map(|x| Some((x.text.lines().next()?.to_string(), x.answer?))) {
            lines.push(format!("- {} \u{2192} {}", x.0, x.1));
        }
    }
    Ok(lines.join("\n"))
}

/// Headless Rui run: no chat transcript, only the question and the rule/escalate tools.
fn rui_decision_run(cfg: &Config, parent: &Task, prompt: String, system_prompt: String) -> AgentRun {
    AgentRun {
        role: Role::Rui,
        task_id: Some(parent.id),
        cwd: cfg.data_dir.clone(),
        prompt,
        system_prompt,
        allowed_tools: RUI_DECIDE_TOOLS.map(String::from).to_vec(),
        builtin_tools: Some(String::new()),
        permission_mode: None,
        permission_prompt_tool: None,
        mcp_url: format!("http://127.0.0.1:{}/mcp/rui", cfg.port),
        mcp_headers: vec![],
        resume: None,
        partial: true,
        seat: None,
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
    let playbook = client_state::read_playbook(&cfg.data_dir, &client.name)?;
    let prompt = prompts::fill(BUILDER_PROMPT, &builder_vars(db, task, role.as_ref(), done, playbook)?);
    Ok((role_prompt(role.as_ref(), &prompt), tools))
}

/// Values for builder.md: who the member is, its job, and its own progress notes so far.
fn builder_vars(db: &Db, task: &Task, role: Option<&roster::Role>, done: &str, playbook: String) -> Result<Vec<(&'static str, String)>> {
    let who = role.map(|r| r.title.clone()).or_else(|| task.agent.clone()).unwrap_or_else(|| "builder".into());
    Ok(vec![
        ("role", who),
        ("title", task.title.clone()),
        ("job", task.body.clone()),
        ("done_command", done.to_string()),
        ("progress_file", progress_notes(db, task.id)?),
        ("playbook", playbook),
    ])
}

/// The member's report_progress notes, oldest first, one per line.
fn progress_notes(db: &Db, task_id: i64) -> Result<String> {
    let events = db.list_events(Some(task_id), 400)?;
    let notes = events.iter().filter(|e| e.kind == "progress").filter_map(|e| e.payload["note"].as_str());
    Ok(notes.map(|n| format!("- {n}")).collect::<Vec<_>>().join("\n"))
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

/// A clarifying top-level task that already has subtasks was split; it must not get a "new task" wake again.
pub(crate) fn recovered_status(db: &Db, t: &Task, to: TaskStatus) -> Result<TaskStatus> {
    if to == TaskStatus::New && t.parent_id.is_none() && !db.subtasks(t.id)?.is_empty() {
        return Ok(TaskStatus::Split);
    }
    Ok(to)
}

/// On daemon start: orphaned in_progress tasks go back to open, clarifying ones to new (split if they already have subtasks).
pub fn recover_on_start(db: &Db) -> Result<usize> {
    let moves = [
        (TaskStatus::InProgress, TaskStatus::Open),
        (TaskStatus::Clarifying, TaskStatus::New),
    ];
    let mut count = 0;
    for (from, to) in moves {
        for t in db.tasks_with_status(from)? {
            let to = recovered_status(db, &t, to)?;
            db.set_task_status(t.id, to)?;
            let payload = json!({"from": from.as_str(), "to": to.as_str()});
            db.add_event(Some(t.id), "scheduler", "recovered", &payload)?;
            count += 1;
        }
    }
    Ok(count)
}

async fn builder_turn(db: &Arc<Db>, cfg: &Config, live: &Live, task_id: i64) -> Result<()> {
    let task = db.get_task(task_id)?.context("task not found")?;
    let done_command = task.done_command.clone().context("subtask has no done_command")?;
    let client = client_for(db, &task)?;
    let repo = expand_home(&client.repo_path);
    let parent_id = task.parent_id.context("subtask has no parent")?;
    let worktrees = cfg.data_dir.join("worktrees");
    let (_, task_branch) = task_worktree(cfg, &client, &repo, parent_id)?;
    record_parent_branch(db, parent_id, &task_branch)?;
    let (wt, branch) = create_worktree(&repo, &worktrees, task_id, Some(&task_branch))?;
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
        permission_prompt_tool: None,
        mcp_url: format!("http://127.0.0.1:{}/mcp/builder", cfg.port),
        mcp_headers: vec![("X-Rui-Task-Id".into(), task_id.to_string())],
        resume,
        partial: true,
        seat: None,
    };
    let cli = crate::codex::for_member(&cfg.data_dir, &client.name, task.agent.as_deref().unwrap_or("builder"));
    match run_logged_on(cli, db, cfg, live, "builder", task_id, run).await {
        Ok(out) => {
            if let Some(id) = out.session_id {
                db.set_session(Role::Builder, Some(task_id), &id)?;
            }
        }
        Err(e) => {
            db.add_event(Some(task_id), "scheduler", "agent_error", &json!({"error": format!("{e:#}")}))?;
        }
    }
    finish_builder(db, cfg, &task, &wt, &done_command).await
}

/// Remembers the per-task branch on the parent, once.
fn record_parent_branch(db: &Db, parent_id: i64, branch: &str) -> Result<()> {
    let parent = db.get_task(parent_id)?.context("parent task not found")?;
    if parent.branch.is_none() {
        db.set_task_branch(parent_id, branch)?;
    }
    Ok(())
}

async fn finish_builder(db: &Arc<Db>, cfg: &Config, task: &Task, wt: &Path, done_command: &str) -> Result<()> {
    let current = db.get_task(task.id)?.context("task not found")?;
    if matches!(current.status, TaskStatus::Stuck | TaskStatus::WaitingAnswer) {
        return Ok(());
    }
    if reject_protected(db, cfg, task, wt)? {
        return Ok(());
    }
    let check = run_done_command(wt, done_command).await?;
    if after_done_check(db, &cfg.data_dir, task.id, &check)? != TaskStatus::Done {
        return Ok(());
    }
    let (db, cfg, task, wt) = (db.clone(), cfg.clone(), task.clone(), wt.to_path_buf());
    tokio::task::spawn_blocking(move || commit_and_merge(&db, &cfg, &task, &wt)).await?
}

/// Commits a passing subtask and merges it into the task branch (sync git, run off the async runtime).
fn commit_and_merge(db: &Db, cfg: &Config, task: &Task, wt: &Path) -> Result<()> {
    if commit_task(db, task, wt)? {
        merge_subtask(db, cfg, task)?;
    }
    Ok(())
}

/// The worktree a subtask's builder works in.
pub fn subtask_worktree(data_dir: &Path, task_id: i64) -> PathBuf {
    data_dir.join("worktrees").join(task_id.to_string())
}

/// What came of the owner handing a taken-over subtask back.
#[derive(Debug, serde::Serialize)]
pub struct HandBack {
    pub status: TaskStatus,
    pub passed: bool,
    pub output_tail: String,
}

/// Runs the check on a step the owner did by hand. A pass, or `force`, lands it like a
/// builder's work; a failure sends the owner's note to the agent and opens the step again.
pub async fn hand_back(db: &Arc<Db>, data_dir: &Path, task: &Task, note: &str, force: bool) -> Result<HandBack> {
    // In progress while the check runs, so the scheduler does not start the agent meanwhile.
    db.set_task_status(task.id, TaskStatus::InProgress)?;
    db.add_event(Some(task.id), "owner", "handed_back", &json!({"note": note, "force": force}))?;
    let check = hand_back_check(data_dir, task).await;
    record_check(db, task.id, &check)?;
    let status = if check.passed || force {
        land_handed_back(db, data_dir, task, !check.passed).await?
    } else {
        reopen_with_note(db, task.id, note)?
    };
    Ok(HandBack { status, passed: check.passed, output_tail: check.output_tail })
}

/// Runs the subtask's done_command in its worktree; a missing command or a spawn error fails.
async fn hand_back_check(data_dir: &Path, task: &Task) -> DoneCheck {
    let Some(cmd) = task.done_command.as_deref() else {
        return DoneCheck { passed: false, output_tail: "this step has no done_command".into() };
    };
    let wt = subtask_worktree(data_dir, task.id);
    run_done_command(&wt, cmd)
        .await
        .unwrap_or_else(|e| DoneCheck { passed: false, output_tail: format!("{e:#}") })
}

/// Marks the step done (logging `forced_done` when forced), then commits and merges it.
async fn land_handed_back(db: &Arc<Db>, data_dir: &Path, task: &Task, forced: bool) -> Result<TaskStatus> {
    db.set_task_status(task.id, TaskStatus::Done)?;
    if forced {
        db.add_event(Some(task.id), "owner", "forced_done", &json!({}))?;
    }
    // commit_and_merge only reads the data dir from the config.
    let cfg = Config { port: 0, tick: Duration::from_secs(1), data_dir: data_dir.to_path_buf() };
    let wt = subtask_worktree(data_dir, task.id);
    let (db2, task2) = (db.clone(), task.clone());
    tokio::task::spawn_blocking(move || commit_and_merge(&db2, &cfg, &task2, &wt)).await??;
    Ok(db.get_task(task.id)?.context("task not found")?.status)
}

/// Saves the owner's note as an instruction for the agent's next turn and opens the step again.
fn reopen_with_note(db: &Db, task_id: i64, note: &str) -> Result<TaskStatus> {
    let text = match note.trim() {
        "" => "The owner handed this step back but the check still fails. Make the done command pass.",
        n => n,
    };
    db.add_event(Some(task_id), "owner", "instruction", &json!({"text": text}))?;
    db.set_task_status(task_id, TaskStatus::Open)?;
    Ok(TaskStatus::Open)
}

/// Undoes edits to protected files and removes new protected files. True when something was rejected (the failed loop is counted).
fn reject_protected(db: &Db, cfg: &Config, task: &Task, wt: &Path) -> Result<bool> {
    let client = client_for(db, task)?;
    let parent_id = task.parent_id.context("subtask has no parent")?;
    let repo = expand_home(&client.repo_path);
    let (task_wt, task_branch) = task_worktree(cfg, &client, &repo, parent_id)?;
    let pod = client_state::read_pod(&cfg.data_dir, &client.name)?;
    let siblings = db.subtasks(parent_id)?;
    let set = protected::protected_set(&task_wt, task, &siblings, &pod, &granted_paths(db, task)?)?;
    let mut bad = protected::touched_existing(wt, &task_branch)?;
    bad.retain(|f| protected::is_protected(f, &set));
    let mut bad_new = protected::added_files(wt, &task_branch)?;
    bad_new.retain(|f| protected::is_protected_new(f, &set));
    if bad.is_empty() && bad_new.is_empty() {
        return Ok(false);
    }
    crate::worktree::restore_files(wt, &task_branch, &bad)?;
    crate::worktree::remove_new_files(wt, &bad_new)?;
    record_rejection(db, cfg, task, &bad, &bad_new)?;
    Ok(true)
}

/// Union of the protected paths the Lead granted this subtask.
fn granted_paths(db: &Db, task: &Task) -> Result<Vec<String>> {
    let mut paths = Vec::new();
    for e in db.events_for_tasks(&[task.id], &["paths_granted"], 0, i64::MAX)? {
        let list = e.payload["paths"].as_array().into_iter().flatten();
        paths.extend(list.filter_map(|p| p.as_str().map(String::from)));
    }
    Ok(paths)
}

/// Records the rejection events and counts the failed loop.
fn record_rejection(db: &Db, cfg: &Config, task: &Task, bad: &[String], bad_new: &[String]) -> Result<()> {
    let all: Vec<&String> = bad.iter().chain(bad_new).collect();
    let files = all.iter().map(|f| f.as_str()).collect::<Vec<_>>().join(", ");
    db.add_event(Some(task.id), "scheduler", "protected_rejected", &json!({"files": all, "new": bad_new}))?;
    let text = format!(
        "Your changes to protected files were rejected and undone: {files}. Tests, eval files and files the done_command uses must not be edited, deleted or skipped. Make the done_command pass by doing the job."
    );
    db.add_event(Some(task.id), "scheduler", "instruction", &json!({"text": text}))?;
    let status = count_failed_loop(db, &cfg.data_dir, task, &format!("changed protected files: {files}"))?;
    db.set_task_status(task.id, status)?;
    Ok(())
}

/// Commits the worktree; false when there was nothing to commit.
fn commit_task(db: &Db, task: &Task, wt: &Path) -> Result<bool> {
    let committed = commit_all(wt, &format!("rui: task {}: {}", task.id, task.title))?;
    db.add_event(Some(task.id), "scheduler", "commit", &json!({"committed": committed}))?;
    Ok(committed)
}

/// Merges a finished subtask branch into its parent's task branch; a conflict turns it Stuck.
fn merge_subtask(db: &Db, cfg: &Config, task: &Task) -> Result<()> {
    let client = client_for(db, task)?;
    let parent_id = task.parent_id.context("subtask has no parent")?;
    let repo = expand_home(&client.repo_path);
    let (task_wt, task_branch) = task_worktree(cfg, &client, &repo, parent_id)?;
    let branch = crate::worktree::branch_name(task.id);
    let message = format!("rui: task {}: {}", task.id, task.title);
    match merge_into(&task_wt, &branch, &message)? {
        LandResult::Merged => {
            let payload = json!({"branch": branch, "into": task_branch});
            db.add_event(Some(task.id), "scheduler", "merged_to_task_branch", &payload)?;
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
    fn ask_scout_plans_one_scout_turn_and_the_lead_keeps_its_state() {
        let (db, c) = setup();
        let t = mk(&db, c, None, TaskStatus::Split);
        let ctx = crate::mcp::Ctx { task_id: Some(t.id), source_ref: None };
        let args = crate::mcp::AskScoutArgs { question: "Why does the build fail?".into() };
        let id = crate::mcp::logic::ask_scout(&db, &ctx, args).unwrap()["request_event_id"].as_i64().unwrap();
        assert_eq!(plan(&db), vec![Action::Scout { task_id: t.id, request_event_id: id }]);
        assert_eq!(db.get_task(t.id).unwrap().unwrap().status, TaskStatus::Split);
        let report = crate::scout::Report { summary: "Missing env var".into(), findings: vec![], sources: vec![] };
        crate::scout::report(&db, id, Some(t.id), report).unwrap();
        assert!(plan(&db).is_empty());
        assert!(take_instructions(&db, t.id).unwrap().contains("Missing env var"));
    }

    #[test]
    fn members_cannot_ask_the_scout() {
        let (db, c) = setup();
        let p = mk(&db, c, None, TaskStatus::Split);
        let sub = mk(&db, c, Some(p.id), TaskStatus::InProgress);
        let ctx = crate::mcp::Ctx { task_id: Some(sub.id), source_ref: None };
        assert!(crate::mcp::logic::ask_scout(&db, &ctx, crate::mcp::AskScoutArgs { question: "q".into() }).is_err());
    }

    #[test]
    fn done_task_with_a_send_back_gets_one_coach_turn() {
        let (db, c) = setup();
        let t = mk(&db, c, None, TaskStatus::Done);
        db.add_event(Some(t.id), "lead", "sent_back", &json!({"remarks": "r"})).unwrap();
        assert_eq!(plan(&db), vec![Action::Coach { task_id: t.id }]);
        db.add_event(Some(t.id), "coach", "coach_reviewed", &json!({})).unwrap();
        assert!(plan(&db).is_empty());
    }

    #[test]
    fn support_turns_wait_for_a_free_seat() {
        let actions = vec![Action::Scout { task_id: 1, request_event_id: 2 }, Action::Coach { task_id: 1 }];
        assert!(hold_turns(actions, false).is_empty());
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
    fn recover_sends_a_split_clarifying_task_to_split() {
        let (db, c) = setup();
        let parent = mk(&db, c, None, TaskStatus::Clarifying);
        mk(&db, c, Some(parent.id), TaskStatus::Open);
        assert_eq!(recover_on_start(&db).unwrap(), 1);
        assert_eq!(db.get_task(parent.id).unwrap().unwrap().status, TaskStatus::Split);
    }

    #[test]
    fn account_lead_vars_fill_every_placeholder() {
        let (db, c) = setup();
        let client = db.get_client(c).unwrap().unwrap();
        let data = tempfile::tempdir().unwrap();
        client_state::ensure(data.path(), "acme", "/repo", None).unwrap();
        client_state::append_brief(data.path(), "acme", 1, "Title", "the brief").unwrap();
        client_state::append_decision(data.path(), "acme", "picked sqlite").unwrap();
        client_state::append_knowledge(data.path(), "acme", "Owner says: no Docker").unwrap();
        let pod = client_state::Pod {
            roles: vec![client_state::PodRole { role: "backend".into(), skills: vec!["extra-skill".into()] }],
            protected: vec![],
            cli: None,
        };
        client_state::write_pod(data.path(), "acme", &pod).unwrap();
        let vars = account_lead_vars(data.path(), &client, "a new task").unwrap();
        let get = |k: &str| vars.iter().find(|v| v.0 == k).unwrap().1.clone();
        assert!(get("client_log").contains("the brief") && get("client_log").contains("picked sqlite"));
        assert!(get("pod_roles_and_skills").contains("backend") && get("pod_roles_and_skills").contains("extra-skill"));
        assert_eq!(get("event"), "a new task");
        assert!(get("owner_knowledge").contains("Owner says: no Docker"));
        let filled = prompts::fill(ACCOUNT_LEAD_PROMPT, &vars);
        assert!(prompts::unfilled(&filled).is_empty());
        assert!(filled.find("no Docker").unwrap() < filled.find("Client Playbook").unwrap());
    }

    #[test]
    fn account_lead_never_gets_edit_or_write() {
        let role = roster::parse("---\nname: lead\ncode: L\ntitle: Lead\ndescription: d\ntools: [\"Read\", \"Edit\", \"Shell\"]\n---\nbody").unwrap();
        let tools = account_lead_tools(Some(&role));
        assert!(!tools.iter().any(|t| t == "Edit" || t == "Write"));
        assert!(tools.iter().any(|t| t == "mcp__rui__log_decision"));
        assert!(tools.iter().any(|t| t == "Bash"));
    }

    fn git_in(dir: &Path, args: &[&str]) {
        let o = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    }

    /// A repo with `tests/test_a.py` on main, a parent task, one subtask and its worktree.
    fn protected_setup(data: &Path, repo: &Path) -> (Db, Config, Task, PathBuf) {
        git_in(repo, &["init", "-b", "main"]);
        std::fs::create_dir_all(repo.join("tests")).unwrap();
        std::fs::write(repo.join("tests/test_a.py"), "orig").unwrap();
        git_in(repo, &["add", "."]);
        git_in(repo, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "init"]);
        let db = Db::open_test();
        let client = db.create_client("acme", &repo.to_string_lossy(), None, &[]).unwrap();
        let parent = mk(&db, client.id, None, TaskStatus::Split);
        let sub = mk(&db, client.id, Some(parent.id), TaskStatus::InProgress);
        let cfg = Config { port: 0, tick: Duration::from_secs(1), data_dir: data.to_path_buf() };
        let worktrees = data.join("worktrees");
        let (_, tb) = ensure_task_worktree(repo, &worktrees, parent.id, &crate::worktree::base_branch(repo).unwrap()).unwrap();
        let (wt, _) = create_worktree(repo, &worktrees, sub.id, Some(&tb)).unwrap();
        (db, cfg, sub, wt)
    }

    #[test]
    fn reject_protected_undoes_edits_and_counts_a_loop() {
        let (data, repo) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (db, cfg, sub, wt) = protected_setup(data.path(), repo.path());
        assert!(!reject_protected(&db, &cfg, &sub, &wt).unwrap());
        std::fs::write(wt.join("tests/test_a.py"), "cheat").unwrap();
        std::fs::write(wt.join("new.py"), "new").unwrap();
        assert!(reject_protected(&db, &cfg, &sub, &wt).unwrap());
        assert_eq!(std::fs::read_to_string(wt.join("tests/test_a.py")).unwrap(), "orig");
        assert!(wt.join("new.py").exists());
        let events = db.list_events(Some(sub.id), 50).unwrap();
        assert!(events.iter().any(|e| e.kind == "protected_rejected"));
        assert!(events.iter().any(|e| e.kind == "instruction"));
        let after = db.get_task(sub.id).unwrap().unwrap();
        assert_eq!((after.loops_used, after.status), (1, TaskStatus::Open));
    }

    fn rejected(db: &Db, id: i64) -> bool {
        db.list_events(Some(id), 50).unwrap().iter().any(|e| e.kind == "protected_rejected")
    }

    #[test]
    fn reject_protected_removes_new_conftest() {
        let (data, repo) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (db, cfg, sub, wt) = protected_setup(data.path(), repo.path());
        std::fs::write(wt.join("conftest.py"), "x").unwrap();
        assert!(reject_protected(&db, &cfg, &sub, &wt).unwrap());
        assert!(!wt.join("conftest.py").exists());
        assert!(rejected(&db, sub.id));
    }

    #[test]
    fn reject_protected_restores_edited_package_json() {
        let (data, repo) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::create_dir_all(repo.path()).unwrap();
        std::fs::write(repo.path().join("package.json"), "{\"scripts\":{\"test\":\"jest\"}}").unwrap();
        let (db, cfg, sub, wt) = {
            let r = repo.path();
            git_in(r, &["init", "-b", "main"]);
            git_in(r, &["add", "."]);
            git_in(r, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "pj"]);
            let db = Db::open_test();
            let client = db.create_client("acme", &r.to_string_lossy(), None, &[]).unwrap();
            let parent = mk(&db, client.id, None, TaskStatus::Split);
            let sub = mk(&db, client.id, Some(parent.id), TaskStatus::InProgress);
            let cfg = Config { port: 0, tick: Duration::from_secs(1), data_dir: data.path().to_path_buf() };
            let worktrees = data.path().join("worktrees");
            let (_, tb) = ensure_task_worktree(r, &worktrees, parent.id, &crate::worktree::base_branch(r).unwrap()).unwrap();
            let (wt, _) = create_worktree(r, &worktrees, sub.id, Some(&tb)).unwrap();
            (db, cfg, sub, wt)
        };
        std::fs::write(wt.join("package.json"), "{\"scripts\":{\"test\":\"true\"}}").unwrap();
        assert!(reject_protected(&db, &cfg, &sub, &wt).unwrap());
        assert_eq!(std::fs::read_to_string(wt.join("package.json")).unwrap(), "{\"scripts\":{\"test\":\"jest\"}}");
        assert!(rejected(&db, sub.id));
    }

    #[test]
    fn reject_protected_honours_a_paths_granted_event() {
        for granted in [false, true] {
            let (data, repo) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
            let (db, cfg, sub, wt) = protected_setup(data.path(), repo.path());
            if granted {
                db.add_event(Some(sub.id), "lead", "paths_granted", &json!({"paths": ["package.json"]})).unwrap();
            }
            std::fs::write(wt.join("package.json"), "{}").unwrap();
            assert_eq!(reject_protected(&db, &cfg, &sub, &wt).unwrap(), !granted);
        }
    }

    #[test]
    fn reject_protected_allows_plain_new_source_file() {
        let (data, repo) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (db, cfg, sub, wt) = protected_setup(data.path(), repo.path());
        std::fs::create_dir_all(wt.join("src")).unwrap();
        std::fs::write(wt.join("src/new.py"), "x").unwrap();
        assert!(!reject_protected(&db, &cfg, &sub, &wt).unwrap());
        assert!(wt.join("src/new.py").exists());
    }

    #[test]
    fn client_lock_is_per_client() {
        let locks = ClientLocks::default();
        assert!(Arc::ptr_eq(&client_lock(&locks, 1), &client_lock(&locks, 1)));
        assert!(!Arc::ptr_eq(&client_lock(&locks, 1), &client_lock(&locks, 2)));
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
    fn held_subtask_is_not_planned_until_released() {
        let (db, c) = setup();
        let parent = mk(&db, c, None, TaskStatus::Split);
        let a = mk(&db, c, Some(parent.id), TaskStatus::Open);
        db.add_event(Some(a.id), "owner", "checkpoint_set", &json!({})).unwrap();
        assert_eq!(owner_hold(&db, a.id).unwrap(), Some("checkpoint"));
        assert!(plan(&db).is_empty());
        db.add_event(Some(a.id), "owner", "checkpoint_cleared", &json!({})).unwrap();
        assert_eq!(plan(&db), vec![Action::StartBuilder { task_id: a.id }]);
        db.add_event(Some(a.id), "owner", "taken_over", &json!({})).unwrap();
        assert!(plan(&db).is_empty());
        db.add_event(Some(a.id), "owner", "handed_back", &json!({})).unwrap();
        assert_eq!(owner_hold(&db, a.id).unwrap(), None);
    }

    /// The protected-files repo with the subtask taken over and running `cmd` as its check.
    fn handed_setup(data: &Path, repo: &Path, cmd: &str) -> (Arc<Db>, Task, PathBuf) {
        let (db, _cfg, sub, wt) = protected_setup(data, repo);
        db.set_done_command(sub.id, cmd).unwrap();
        db.add_event(Some(sub.id), "owner", "taken_over", &json!({})).unwrap();
        let sub = db.get_task(sub.id).unwrap().unwrap();
        (Arc::new(db), sub, wt)
    }

    fn has_event(db: &Db, id: i64, kind: &str) -> bool {
        db.list_events(Some(id), 1000).unwrap().iter().any(|e| e.kind == kind)
    }

    #[tokio::test]
    async fn failing_hand_back_reopens_with_the_note() {
        let (data, repo) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (db, sub, _wt) = handed_setup(data.path(), repo.path(), "false");
        let out = hand_back(&db, data.path(), &sub, "fix the edge case", false).await.unwrap();
        assert_eq!((out.status, out.passed), (TaskStatus::Open, false));
        assert_eq!(db.get_task(sub.id).unwrap().unwrap().status, TaskStatus::Open);
        let pending = db.pending_instructions(sub.id).unwrap();
        assert_eq!(pending.last().unwrap().1, "fix the edge case");
        assert_eq!(owner_hold(&db, sub.id).unwrap(), None);
    }

    #[tokio::test]
    async fn forced_hand_back_is_done_and_logged() {
        let (data, repo) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (db, sub, _wt) = handed_setup(data.path(), repo.path(), "false");
        let out = hand_back(&db, data.path(), &sub, "", true).await.unwrap();
        assert_eq!((out.status, out.passed), (TaskStatus::Done, false));
        assert!(has_event(&db, sub.id, "forced_done"));
        assert!(!has_event(&db, sub.id, "instruction"));
    }

    #[tokio::test]
    async fn passing_hand_back_merges_into_the_task_branch() {
        let (data, repo) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (db, sub, wt) = handed_setup(data.path(), repo.path(), "test -f done.txt");
        std::fs::write(wt.join("done.txt"), "by hand").unwrap();
        let out = hand_back(&db, data.path(), &sub, "", false).await.unwrap();
        assert_eq!((out.status, out.passed), (TaskStatus::Done, true));
        assert!(has_event(&db, sub.id, "merged_to_task_branch"));
        assert!(!has_event(&db, sub.id, "forced_done"));
        let task_wt = data.path().join("worktrees").join(format!("task-{}", sub.parent_id.unwrap()));
        assert!(task_wt.join("done.txt").exists());
    }

    #[test]
    fn all_seats_used_holds_new_turns() {
        let (db, c) = setup();
        let d = tempfile::tempdir().unwrap();
        let t = mk(&db, c, None, TaskStatus::New);
        let now = crate::thread::now_secs();
        let lead = vec![Action::StartLead { task_id: t.id, answers: vec![] }];
        assert_eq!(plan_gated(&db, d.path(), &HashSet::new(), now).unwrap(), lead);
        let only = seats::Seat { budget_usd: Some(1.0), ..seats::default_seat() };
        seats::upsert(d.path(), only).unwrap();
        db.add_event(Some(t.id), "lead", "result", &json!({"total_cost_usd": 0.95, "seat_id": "default"})).unwrap();
        assert!(plan_gated(&db, d.path(), &HashSet::new(), now).unwrap().is_empty());
        assert_eq!(db.events_of_kinds(&["seats_exhausted"]).unwrap().len(), 1);
        let rollup = Action::Rollup { task_id: t.id, status: TaskStatus::Done };
        let mixed = vec![rollup, Action::StartBuilder { task_id: 9 }, Action::Review { task_id: 9 }];
        assert_eq!(hold_turns(mixed, false), [Action::Rollup { task_id: t.id, status: TaskStatus::Done }]);
    }

    #[test]
    fn only_result_events_get_a_seat_id() {
        assert_eq!(tag_seat("result", json!({"total_cost_usd": 1.0}), "w")["seat_id"], "w");
        assert!(tag_seat("assistant", json!({}), "w").get("seat_id").is_none());
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
        assert_eq!(plan(&db), vec![Action::Review { task_id: p.id }]);
        forward(&db, p.id);
        assert_eq!(plan(&db), vec![Action::Rollup { task_id: p.id, status: TaskStatus::Done }]);
    }

    fn forward(db: &Db, task_id: i64) {
        std::thread::sleep(Duration::from_millis(5));
        db.add_event(Some(task_id), "lead", "forwarded", &json!({"remarks": "ok", "pr_url": null})).unwrap();
        std::thread::sleep(Duration::from_millis(5));
    }

    #[test]
    fn review_wakes_again_when_a_new_subtask_finishes_after_forwarding() {
        let (db, c) = setup();
        let p = mk(&db, c, None, TaskStatus::Split);
        mk(&db, c, Some(p.id), TaskStatus::Done);
        assert_eq!(plan(&db), vec![Action::Review { task_id: p.id }]);
        forward(&db, p.id);
        assert_eq!(plan(&db), vec![Action::Rollup { task_id: p.id, status: TaskStatus::Done }]);
        let extra = mk(&db, c, Some(p.id), TaskStatus::Open);
        db.set_task_status(extra.id, TaskStatus::Done).unwrap();
        assert_eq!(plan(&db), vec![Action::Review { task_id: p.id }]);
    }

    #[test]
    fn review_prompt_lists_subtasks_and_tools() {
        let (db, c) = setup();
        let p = mk(&db, c, None, TaskStatus::Split);
        let sub = mk(&db, c, Some(p.id), TaskStatus::Done);
        let client = db.get_client(c).unwrap().unwrap();
        let text = review_prompt(&db, std::path::Path::new("/nonexistent-data"), &p, &client).unwrap();
        assert!(text.contains(&format!("rui/task-{}", p.id)) && text.contains(&format!("- #{} ", sub.id)));
        assert!(text.contains(&format!("send_back(task_id={}", p.id)) && text.contains("forward_pr("));
    }

    #[test]
    fn done_notice_includes_the_account_leads_remarks() {
        let db = Db::open_test();
        let chat = db.create_chat().unwrap();
        let msg = db.add_chat_message(chat.id, ChatRole::User, "make it").unwrap();
        let task = chat_task(&db, Some(format!("chat:{}", msg.id)));
        db.add_event(Some(task.id), "lead", "forwarded", &json!({"remarks": "Mind the schema", "pr_url": "https://x.test/pr/2"})).unwrap();
        post_done_notice(&db, &task, "/tmp/c", &[]).unwrap();
        let last = db.list_chat(chat.id, 10).unwrap().pop().unwrap();
        assert!(last.content.contains("**Account Lead:** Mind the schema"));
        assert!(last.content.contains("PR: https://x.test/pr/2"));
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
    fn decision_prompt_has_question_ladder_and_check() {
        let (db, c) = setup();
        let (m, q) = waiting_member(&db, c, "lead");
        let ladder = [Rung { who: "Backend 7".into(), why: "which db?".into() }];
        db.set_question_level(q, "lead", &ladder).unwrap();
        let (q, member) = (db.get_question(q).unwrap().unwrap(), db.get_task(m).unwrap().unwrap());
        let parent = db.get_task(member.parent_id.unwrap()).unwrap().unwrap();
        let p = decision_prompt(&q, &member, &parent, Some("boom"), "", "escalate");
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
        assert_eq!(plan(&db), vec![Action::Review { task_id: parent.id }]);
        forward(&db, parent.id);
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

    #[test]
    fn done_message_lists_file_links() {
        let db = Db::open_test();
        let task = chat_task(&db, None);
        let file = |p: &str| ResultFile { path: p.into(), abs_path: format!("/r/{p}"), size: 1, modified: 0, task_id: 7, agent: None };
        let files: Vec<ResultFile> = (0..10).map(|i| file(&format!("d/f{i}.md"))).collect();
        let msg = done_message(&task, "/r", &[], &files);
        assert!(msg.contains("**Files:**\n- [d/f0.md](rui://result/7/d/f0.md)"));
        assert!(msg.contains("f7.md") && !msg.contains("f8.md"));
        assert!(!done_message(&task, "/r", &[], &[]).contains("Files"));
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

    /// A split parent with one waiting member at its loop limit and an open lead question.
    fn limited_member(db: &Db, c: i64) -> (Task, Task, i64) {
        let (m, q) = waiting_member(db, c, "lead");
        db.increment_loops(m).unwrap();
        db.increment_loops(m).unwrap();
        let member = db.get_task(m).unwrap().unwrap();
        let parent = db.get_task(member.parent_id.unwrap()).unwrap().unwrap();
        (parent, member, q)
    }

    #[test]
    fn answer_at_the_loop_limit_sticks_instead_of_reopening() {
        let (db, c) = setup();
        let (parent, member, q) = limited_member(&db, c);
        answer_question(&db, q, "try harder", None, "lead").unwrap();
        let status = |id| db.get_task(id).unwrap().unwrap().status;
        assert_eq!(status(member.id), TaskStatus::Stuck);
        let events = db.list_events(Some(member.id), 100).unwrap();
        assert!(events.iter().any(|e| e.kind == "loop_limit"));
        assert!(events.iter().any(|e| e.kind == "instruction"));
        assert!(matches!(plan_rollup(&db, &parent).unwrap(), Some(Action::Rollup { status: TaskStatus::Stuck, .. })));
        assert!(stuck_reason(&db, parent.id).unwrap().unwrap().contains("loop limit (2/2)"));
    }

    #[test]
    fn answer_below_or_after_raising_the_limit_reopens() {
        let (db, c) = setup();
        let (m, q) = waiting_member(&db, c, "lead");
        answer_question(&db, q, "go", None, "lead").unwrap();
        assert_eq!(db.get_task(m).unwrap().unwrap().status, TaskStatus::Open);
        let (_, member, q) = limited_member(&db, c);
        db.set_max_loops(member.id, 5).unwrap();
        answer_question(&db, q, "go", None, "lead").unwrap();
        assert_eq!(db.get_task(member.id).unwrap().unwrap().status, TaskStatus::Open);
    }

    #[test]
    fn retry_raises_the_limit_of_a_loop_limited_subtask() {
        let (db, c) = setup();
        let (parent, member, q) = limited_member(&db, c);
        answer_question(&db, q, "go", None, "lead").unwrap();
        db.set_task_status(parent.id, TaskStatus::Stuck).unwrap();
        retry_stuck(&db, parent.id, "owner").unwrap();
        let after = db.get_task(member.id).unwrap().unwrap();
        assert_eq!((after.status, after.max_loops), (TaskStatus::Open, 5));
        let events = db.list_events(Some(parent.id), 100).unwrap();
        let resumed = events.iter().find(|e| e.kind == "resumed").unwrap();
        assert_eq!(resumed.payload["raised_limit"], json!([member.id]));
    }

    #[test]
    fn decision_prompt_lists_owner_answers_from_sibling_subtasks() {
        let (db, c) = setup();
        let (m, q) = waiting_member(&db, c, "lead");
        let member = db.get_task(m).unwrap().unwrap();
        let sibling = mk(&db, c, member.parent_id, TaskStatus::Open);
        let asked = db.create_ladder_question(sibling.id, Role::Builder, "owner", &[], "Which db?\nmore").unwrap();
        db.answer_question(asked.id, "postgres", None).unwrap();
        let parent = db.get_task(member.parent_id.unwrap()).unwrap().unwrap();
        let q = db.get_question(q).unwrap().unwrap();
        let answers = owner_answers(&db, parent.id).unwrap();
        let p = decision_prompt(&q, &member, &parent, None, &answers, "escalate");
        assert!(p.contains(&format!("- Q{}: Which db? \u{2192} postgres", asked.id)));
        assert!(decision_prompt(&q, &member, &parent, None, "", "escalate").contains("Owner answers on this task:\n(none)"));
    }

    fn deliveries(db: &Db, now: i64) -> Vec<Action> {
        let all = plan_tick_at(db, &HashSet::new(), now).unwrap();
        all.into_iter().filter(|a| matches!(a, Action::DeliverAnswer { .. })).collect()
    }

    fn instructions(db: &Db, task: i64) -> usize {
        db.list_events(Some(task), 100).unwrap().iter().filter(|e| e.kind == "instruction").count()
    }

    #[test]
    fn owner_answer_waits_for_the_undo_window() {
        let (db, c) = setup();
        let data = tempfile::tempdir().unwrap();
        let (m, q) = waiting_member(&db, c, "owner");
        let held = answer_with_undo(&db, q, "yes", None, 1000).unwrap();
        assert_eq!(held.run_at, 1000 + UNDO_SECS);
        assert_eq!(instructions(&db, m), 0);
        assert_eq!(db.get_task(m).unwrap().unwrap().status, TaskStatus::WaitingAnswer);
        assert!(deliveries(&db, 1000 + UNDO_SECS - 1).is_empty());
        let want = vec![Action::DeliverAnswer { question_id: q, event_id: held.event_id }];
        assert_eq!(deliveries(&db, 1000 + UNDO_SECS), want);
        deliver_held(&db, data.path(), held.event_id).unwrap();
        assert_eq!(instructions(&db, m), 1);
        assert_eq!(db.get_task(m).unwrap().unwrap().status, TaskStatus::Open);
        assert!(deliveries(&db, 2000).is_empty());
        deliver_held(&db, data.path(), held.event_id).unwrap();
        assert_eq!(instructions(&db, m), 1);
        let log = client_state::read_log(data.path(), "acme", 50).unwrap();
        assert_eq!(log.matches(&format!("Owner answered Q{q}")).count(), 1);
    }

    #[test]
    fn codex_limit_text_never_uses_up_a_seat() {
        let limited = || RunOutcome { session_id: None, result_text: Some("Claude AI usage limit reached|1700000000".into()), is_error: true, exit_ok: false };
        assert!(seat_used_up(Cli::Claude, &Ok(limited())));
        assert!(!seat_used_up(Cli::Codex, &Ok(limited())));
        assert!(!seat_used_up(Cli::Codex, &Err(anyhow::anyhow!("429 rate_limit_error"))));
    }

    #[tokio::test]
    async fn within_reports_a_timeout() {
        let slow = within(std::time::Duration::from_millis(10), async {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            Ok(())
        });
        assert!(slow.await.unwrap_err().is::<TimedOut>());
        assert_eq!(within(std::time::Duration::from_secs(1), async { Ok(7) }).await.unwrap(), 7);
    }

    #[test]
    fn cancel_and_deliver_racing_close_each_answer_once() {
        let (db, c) = setup();
        let data = tempfile::tempdir().unwrap();
        let mut held_ids = vec![];
        for _ in 0..20 {
            let (_, q) = waiting_member(&db, c, "owner");
            held_ids.push(answer_with_undo(&db, q, "yes", None, 0).unwrap().event_id);
        }
        let db = std::sync::Arc::new(db);
        let (d1, d2, ids1, ids2, path) = (db.clone(), db.clone(), held_ids.clone(), held_ids.clone(), data.path().to_path_buf());
        let a = std::thread::spawn(move || ids1.iter().for_each(|id| deliver_held(&d1, &path, *id).unwrap()));
        let b = std::thread::spawn(move || ids2.iter().for_each(|id| { cancel_held(&d2, *id, 1).unwrap(); }));
        a.join().unwrap();
        b.join().unwrap();
        let closes = db.events_of_kinds(&["approval_cancelled", "approval_delivered"]).unwrap();
        for id in held_ids {
            let n = closes.iter().filter(|e| e.payload["event_id"].as_i64() == Some(id)).count();
            assert_eq!(n, 1, "approval {id} closed {n} times");
        }
    }

    #[test]
    fn cancel_inside_the_window_reopens_and_blocks_delivery() {
        let (db, c) = setup();
        let data = tempfile::tempdir().unwrap();
        let (m, q) = waiting_member(&db, c, "owner");
        let held = answer_with_undo(&db, q, "yes", Some("r"), 1000).unwrap();
        let back = cancel_held(&db, held.event_id, 1005).unwrap().unwrap();
        assert_eq!((back.status, back.answer, back.why), (QuestionStatus::Open, None, None));
        assert!(deliveries(&db, 5000).is_empty());
        deliver_held(&db, data.path(), held.event_id).unwrap();
        assert_eq!(instructions(&db, m), 0);
        assert!(cancel_held(&db, held.event_id, 1006).unwrap().is_none());
        assert!(client_state::read_log(data.path(), "acme", 50).unwrap().is_empty());
    }

    #[test]
    fn cancel_after_delivery_or_window_is_refused() {
        let (db, c) = setup();
        let data = tempfile::tempdir().unwrap();
        let (_, q) = waiting_member(&db, c, "owner");
        let held = answer_with_undo(&db, q, "yes", None, 1000).unwrap();
        assert!(cancel_held(&db, held.event_id, 1000 + UNDO_SECS).unwrap().is_none());
        deliver_held(&db, data.path(), held.event_id).unwrap();
        assert!(cancel_held(&db, held.event_id, 1001).unwrap().is_none());
        assert_eq!(db.get_question(q).unwrap().unwrap().status, QuestionStatus::Answered);
    }

    #[test]
    fn held_answer_on_a_top_level_task_does_not_wake_the_lead() {
        let (db, c) = setup();
        let top = mk(&db, c, None, TaskStatus::WaitingAnswer);
        let q = db.create_ladder_question(top.id, Role::Lead, "owner", &[], "which db?").unwrap();
        let held = answer_with_undo(&db, q.id, "sqlite", None, 1000).unwrap();
        let wakes = |now| plan_tick_at(&db, &HashSet::new(), now).unwrap().into_iter().filter(|a| matches!(a, Action::StartLead { .. })).count();
        assert_eq!(wakes(1001), 0);
        db.add_event(Some(top.id), "scheduler", "approval_delivered", &json!({"event_id": held.event_id})).unwrap();
        assert_eq!(wakes(1001), 1);
    }

    #[test]
    fn non_owner_answers_deliver_at_once() {
        let (db, c) = setup();
        let (m, q) = waiting_member(&db, c, "lead");
        answer_question(&db, q, "go", None, "lead").unwrap();
        assert_eq!(instructions(&db, m), 1);
        assert!(pending_approvals(&db).unwrap().is_empty());
    }
}


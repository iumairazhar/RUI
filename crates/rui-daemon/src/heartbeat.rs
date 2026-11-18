//! The heartbeat: every few minutes, finds stalled or orphaned work and acts on it.
//! State lives in events: `stalled` marks a quiet run, `heartbeat` records what was done.

use crate::db::Db;
use crate::scheduler::{self, Runs};
use crate::thread::STALL_SECS;
use anyhow::{Context, Result};
use rui_core::{Event, Task, TaskStatus};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::Path;

/// How often the scheduler runs the heartbeat.
pub const HEARTBEAT_SECS: u64 = 300;
/// Work quieter than this, with no agent running, counts as orphaned.
const ORPHAN_GRACE_SECS: i64 = 60;
/// A running turn quiet for this long is aborted.
const ABORT_AFTER_SECS: i64 = 3 * STALL_SECS;
const ABORT_REASON: &str = "no activity for 30 minutes";

/// One thing the heartbeat decided to do.
#[derive(Debug, PartialEq)]
pub enum Beat {
    /// An in-progress subtask with no agent: open it so the scheduler starts it again.
    Restart { task_id: i64 },
    /// A clarifying top-level task with no lead turn: back to `status` (new, or split if it has subtasks).
    RestartLead { task_id: i64, status: TaskStatus },
    /// A running turn has been quiet past the stall limit.
    Stalled { task_id: i64, idle_secs: i64 },
    /// A running turn has been quiet so long it gets aborted.
    Abort { task_id: i64, idle_secs: i64 },
}

/// Finds and fixes orphaned and stalled work. Errors on one task are logged, not fatal.
pub fn heartbeat(db: &Db, data_dir: &Path, runs: &Runs, running: &HashSet<i64>, now: i64) -> Result<()> {
    for beat in heartbeat_actions(db, running, now)? {
        if let Err(e) = apply_beat(db, data_dir, runs, &beat) {
            scheduler::report_error(db, Some(beat_task(&beat)), &e);
        }
    }
    Ok(())
}

/// What the heartbeat should do now. `running` holds the task ids with a live turn.
pub fn heartbeat_actions(db: &Db, running: &HashSet<i64>, now: i64) -> Result<Vec<Beat>> {
    let watched = |t: &Task| running.contains(&t.id) || matches!(t.status, TaskStatus::InProgress | TaskStatus::Clarifying);
    let tasks: Vec<Task> = db.list_tasks()?.into_iter().filter(watched).collect();
    let ids: Vec<i64> = tasks.iter().map(|t| t.id).collect();
    let last = db.last_activity(&ids)?;
    let mut beats = vec![];
    for t in &tasks {
        let last_at = last.get(&t.id).copied().unwrap_or(now);
        if let Some(b) = beat_for(db, t, running.contains(&t.id), last_at, now)? {
            beats.push(b);
        }
    }
    Ok(beats)
}

fn beat_for(db: &Db, t: &Task, is_running: bool, last_at: i64, now: i64) -> Result<Option<Beat>> {
    let idle = now - last_at;
    if is_running {
        return stall_beat(db, t.id, last_at, idle);
    }
    if idle <= ORPHAN_GRACE_SECS {
        return Ok(None);
    }
    match (t.parent_id, t.status) {
        (Some(_), TaskStatus::InProgress) => orphan_subtask(db, t.id),
        (None, TaskStatus::Clarifying) => {
            let status = scheduler::recovered_status(db, t, TaskStatus::New)?;
            Ok(Some(Beat::RestartLead { task_id: t.id, status }))
        }
        _ => Ok(None),
    }
}

/// An in-progress subtask with no agent, unless the owner holds it or a hand-back check is running.
fn orphan_subtask(db: &Db, id: i64) -> Result<Option<Beat>> {
    if scheduler::owner_hold(db, id)?.is_some() || checking_hand_back(db, id)? {
        return Ok(None);
    }
    Ok(Some(Beat::Restart { task_id: id }))
}

/// A hand-back keeps the step in progress while its check runs; nothing else logs until the check ends.
fn checking_hand_back(db: &Db, id: i64) -> Result<bool> {
    Ok(db.list_events(Some(id), 1)?.last().is_some_and(|e| e.kind == "handed_back"))
}

fn stall_beat(db: &Db, id: i64, last_at: i64, idle: i64) -> Result<Option<Beat>> {
    if idle > ABORT_AFTER_SECS {
        return Ok(Some(Beat::Abort { task_id: id, idle_secs: idle }));
    }
    if idle <= STALL_SECS || flagged_since(db, id, last_at)? {
        return Ok(None);
    }
    Ok(Some(Beat::Stalled { task_id: id, idle_secs: idle }))
}

/// True when a `stalled` event was already logged after the task's last real activity.
fn flagged_since(db: &Db, id: i64, last_at: i64) -> Result<bool> {
    let evs = db.events_for_tasks(&[id], &["stalled"], 0, i64::MAX)?;
    Ok(evs.last().and_then(event_secs).is_some_and(|t| t >= last_at))
}

fn event_secs(e: &Event) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(&e.created_at).ok().map(|d| d.timestamp())
}

fn beat_task(beat: &Beat) -> i64 {
    match beat {
        Beat::Restart { task_id } | Beat::RestartLead { task_id, .. } => *task_id,
        Beat::Stalled { task_id, .. } | Beat::Abort { task_id, .. } => *task_id,
    }
}

/// Carries out one beat.
pub fn apply_beat(db: &Db, data_dir: &Path, runs: &Runs, beat: &Beat) -> Result<()> {
    match *beat {
        Beat::Restart { task_id } => {
            restart(db, task_id, json!({"action": "restart", "reason": "no agent was running"}), TaskStatus::Open)
        }
        Beat::RestartLead { task_id, status } => {
            restart(db, task_id, json!({"action": "restart_lead", "status": status}), status)
        }
        Beat::Stalled { task_id, idle_secs } => {
            db.add_event(Some(task_id), "scheduler", "stalled", &json!({"idle_secs": idle_secs}))?;
            Ok(())
        }
        Beat::Abort { task_id, idle_secs } => abort_stalled(db, data_dir, runs, task_id, idle_secs),
    }
}

fn restart(db: &Db, id: i64, payload: Value, status: TaskStatus) -> Result<()> {
    db.add_event(Some(id), "scheduler", "heartbeat", &payload)?;
    db.set_task_status(id, status)
}

/// Aborts a run that went quiet. A subtask counts a failed loop; a top-level task goes stuck and asks the owner.
fn abort_stalled(db: &Db, data_dir: &Path, runs: &Runs, id: i64, idle: i64) -> Result<()> {
    crate::control::abort_run(runs, id);
    db.add_event(Some(id), "scheduler", "heartbeat", &json!({"action": "aborted_stalled", "idle_secs": idle}))?;
    let task = db.get_task(id)?.context("task not found")?;
    if task.parent_id.is_none() {
        db.set_task_status(id, TaskStatus::Stuck)?;
        return scheduler::raise_stuck_question(db, id, ABORT_REASON);
    }
    let next = scheduler::count_failed_loop(db, data_dir, &task, ABORT_REASON)?;
    db.set_task_status(id, next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::NewTask;
    use crate::thread::now_secs;
    use rui_core::{Role, SourceKind};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    fn mk(db: &Db, client: i64, parent: Option<i64>, status: TaskStatus) -> Task {
        let t = NewTask {
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
        };
        db.create_task(&t).unwrap()
    }

    fn setup() -> (Db, i64) {
        let db = Db::open_test();
        let c = db.create_client("acme", "/repo", None, &[]).unwrap();
        (db, c.id)
    }

    fn status(db: &Db, id: i64) -> TaskStatus {
        db.get_task(id).unwrap().unwrap().status
    }

    fn kinds(db: &Db, id: i64, kind: &str) -> Vec<Event> {
        db.list_events(Some(id), 100).unwrap().into_iter().filter(|e| e.kind == kind).collect()
    }

    fn beat(db: &Db, runs: &Runs, now: i64) {
        let running: HashSet<i64> = runs.lock().unwrap().keys().copied().collect();
        heartbeat(db, Path::new("/nonexistent"), runs, &running, now).unwrap();
    }

    fn no_runs() -> Runs {
        Arc::new(Mutex::new(HashMap::new()))
    }

    #[test]
    fn orphaned_subtask_is_reopened() {
        let (db, c) = setup();
        let parent = mk(&db, c, None, TaskStatus::Split);
        let sub = mk(&db, c, Some(parent.id), TaskStatus::InProgress);
        beat(&db, &no_runs(), now_secs() + 120);
        assert_eq!(status(&db, sub.id), TaskStatus::Open);
        let hb = kinds(&db, sub.id, "heartbeat");
        assert_eq!(hb.len(), 1);
        assert_eq!(hb[0].payload["action"], "restart");
        assert_eq!(hb[0].payload["reason"], "no agent was running");
    }

    #[test]
    fn taken_over_subtask_is_left_alone() {
        let (db, c) = setup();
        let parent = mk(&db, c, None, TaskStatus::Split);
        let sub = mk(&db, c, Some(parent.id), TaskStatus::InProgress);
        db.add_event(Some(sub.id), "owner", "taken_over", &json!({})).unwrap();
        beat(&db, &no_runs(), now_secs() + 120);
        assert_eq!(status(&db, sub.id), TaskStatus::InProgress);
        assert!(kinds(&db, sub.id, "heartbeat").is_empty());
    }

    #[test]
    fn hand_back_check_in_flight_is_left_alone() {
        let (db, c) = setup();
        let parent = mk(&db, c, None, TaskStatus::Split);
        let sub = mk(&db, c, Some(parent.id), TaskStatus::InProgress);
        db.add_event(Some(sub.id), "owner", "handed_back", &json!({})).unwrap();
        beat(&db, &no_runs(), now_secs() + 120);
        assert_eq!(status(&db, sub.id), TaskStatus::InProgress);
    }

    #[test]
    fn recent_activity_is_left_alone() {
        let (db, c) = setup();
        let parent = mk(&db, c, None, TaskStatus::Split);
        let sub = mk(&db, c, Some(parent.id), TaskStatus::InProgress);
        let clar = mk(&db, c, None, TaskStatus::Clarifying);
        beat(&db, &no_runs(), now_secs() + 30);
        assert_eq!(status(&db, sub.id), TaskStatus::InProgress);
        assert_eq!(status(&db, clar.id), TaskStatus::Clarifying);
    }

    #[test]
    fn orphaned_clarifying_parent_with_subtasks_goes_to_split() {
        let (db, c) = setup();
        let parent = mk(&db, c, None, TaskStatus::Clarifying);
        mk(&db, c, Some(parent.id), TaskStatus::Open);
        let lone = mk(&db, c, None, TaskStatus::Clarifying);
        beat(&db, &no_runs(), now_secs() + 120);
        assert_eq!(status(&db, parent.id), TaskStatus::Split);
        assert_eq!(status(&db, lone.id), TaskStatus::New);
        assert_eq!(kinds(&db, parent.id, "heartbeat")[0].payload["action"], "restart_lead");
    }

    fn running_with(id: i64) -> (Runs, tokio::task::JoinHandle<()>) {
        let join = tokio::spawn(std::future::pending::<()>());
        let runs = no_runs();
        runs.lock().unwrap().insert(id, join.abort_handle());
        (runs, join)
    }

    #[tokio::test]
    async fn stalled_run_is_flagged_once() {
        let (db, c) = setup();
        let parent = mk(&db, c, None, TaskStatus::Split);
        let sub = mk(&db, c, Some(parent.id), TaskStatus::InProgress);
        let (runs, _join) = running_with(sub.id);
        let now = now_secs();
        beat(&db, &runs, now + STALL_SECS + 100);
        beat(&db, &runs, now + STALL_SECS + 400);
        assert_eq!(kinds(&db, sub.id, "stalled").len(), 1);
        assert_eq!(status(&db, sub.id), TaskStatus::InProgress);
        let last = db.last_activity(&[sub.id]).unwrap()[&sub.id];
        assert!(last <= now + 1, "stalled events must not count as activity");
    }

    #[tokio::test]
    async fn long_idle_run_is_aborted_and_counts_a_loop() {
        let (db, c) = setup();
        let parent = mk(&db, c, None, TaskStatus::Split);
        let sub = mk(&db, c, Some(parent.id), TaskStatus::InProgress);
        let (runs, join) = running_with(sub.id);
        beat(&db, &runs, now_secs() + 3 * STALL_SECS + 10);
        assert!(join.await.unwrap_err().is_cancelled());
        assert!(runs.lock().unwrap().is_empty());
        let task = db.get_task(sub.id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Open);
        assert_eq!(task.loops_used, 1);
        assert_eq!(kinds(&db, sub.id, "heartbeat")[0].payload["action"], "aborted_stalled");
    }
}

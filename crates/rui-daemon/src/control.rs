//! Stopping and redirecting tasks. Shared by the HTTP handlers and Rui's MCP tools.

use crate::db::Db;
use crate::scheduler::Runs;
use rui_core::{Role, Task, TaskStatus};
use serde_json::json;

/// Why a control call failed.
#[derive(Debug)]
pub enum ControlError {
    NotFound(String),
    Conflict(String),
    Other(anyhow::Error),
}

impl From<anyhow::Error> for ControlError {
    fn from(e: anyhow::Error) -> Self {
        ControlError::Other(e)
    }
}

impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ControlError::NotFound(m) | ControlError::Conflict(m) => write!(f, "{m}"),
            ControlError::Other(e) => write!(f, "{e:#}"),
        }
    }
}

type Res<T> = Result<T, ControlError>;

fn load(db: &Db, id: i64) -> Res<Task> {
    db.get_task(id)?.ok_or_else(|| ControlError::NotFound("task not found".into()))
}

/// Aborts the running turn of a task, if any.
pub fn abort_run(runs: &Runs, id: i64) {
    let handle = runs.lock().ok().and_then(|mut m| m.remove(&id));
    if let Some(h) = handle {
        h.abort();
    }
}

/// Aborts the run, marks the task stuck and logs a `stopped` event.
fn halt(db: &Db, runs: &Runs, id: i64, by: &str, reason: Option<&str>) -> Res<()> {
    abort_run(runs, id);
    db.set_task_status(id, TaskStatus::Stuck)?;
    db.add_event(Some(id), by, "stopped", &json!({"by": by, "reason": reason}))?;
    Ok(())
}

/// Stops one task that is neither done nor split.
pub fn stop_one(db: &Db, runs: &Runs, id: i64, by: &str, reason: Option<&str>) -> Res<Task> {
    let task = load(db, id)?;
    if matches!(task.status, TaskStatus::Done | TaskStatus::Split) {
        return Err(ControlError::Conflict("task is already done or split".into()));
    }
    halt(db, runs, id, by, reason)?;
    load(db, id)
}

/// Stops a task and, when it has subtasks, every subtask that is not done. Returns the stopped ids.
pub fn stop_tree(db: &Db, runs: &Runs, id: i64, by: &str, reason: Option<&str>) -> Res<Vec<i64>> {
    let task = load(db, id)?;
    if task.status == TaskStatus::Done {
        return Err(ControlError::Conflict("task is already done".into()));
    }
    let mut ids = vec![id];
    ids.extend(db.subtasks(id)?.iter().filter(|s| s.status != TaskStatus::Done).map(|s| s.id));
    for i in &ids {
        halt(db, runs, *i, by, reason)?;
    }
    Ok(ids)
}

/// A stuck parent goes back to split so its other subtasks can run again.
fn reopen_stuck_parent(db: &Db, task: &Task) -> Res<()> {
    let Some(pid) = task.parent_id else { return Ok(()) };
    if db.get_task(pid)?.is_some_and(|p| p.status == TaskStatus::Stuck) {
        db.set_task_status(pid, TaskStatus::Split)?;
    }
    Ok(())
}

/// Saves an instruction, aborts any run and queues the task again.
/// A Lead task (a parent, even a split one) gets a new Lead turn that carries the instruction.
pub fn instruct(db: &Db, runs: &Runs, id: i64, text: &str, by: &str) -> Res<Task> {
    let task = load(db, id)?;
    let is_lead = task.role == Role::Lead;
    let blocked = match task.status {
        TaskStatus::Done | TaskStatus::WaitingAnswer => true,
        TaskStatus::Split => !is_lead,
        _ => false,
    };
    if blocked {
        return Err(ControlError::Conflict("task cannot take instructions in its current status".into()));
    }
    abort_run(runs, id);
    db.add_event(Some(id), by, "instruction", &json!({"text": text}))?;
    db.set_task_status(id, if is_lead { TaskStatus::New } else { TaskStatus::Open })?;
    reopen_stuck_parent(db, &task)?;
    load(db, id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::NewTask;
    use rui_core::SourceKind;

    fn task(db: &Db, client: i64, parent: Option<i64>, role: Role, status: TaskStatus) -> Task {
        db.create_task(&NewTask {
            client_id: client,
            parent_id: parent,
            title: "t".into(),
            body: String::new(),
            status,
            done_command: Some("true".into()),
            max_loops: 5,
            role,
            source_kind: SourceKind::User,
            source_ref: None,
            agent: None,
            pod: vec![],
        })
        .unwrap()
    }

    fn pod(db: &Db) -> (Task, Vec<Task>) {
        let c = db.create_client("c", "/tmp/c", None, &[]).unwrap();
        let parent = task(db, c.id, None, Role::Lead, TaskStatus::Split);
        let subs = [TaskStatus::Open, TaskStatus::InProgress, TaskStatus::Done]
            .map(|st| task(db, c.id, Some(parent.id), Role::Builder, st))
            .to_vec();
        (parent, subs)
    }

    #[test]
    fn stop_tree_stops_parent_and_unfinished_children() {
        let (db, runs) = (Db::open_test(), Runs::default());
        let (parent, subs) = pod(&db);
        let ids = stop_tree(&db, &runs, parent.id, "rui", Some("owner changed course")).unwrap();
        assert_eq!(ids, [parent.id, subs[0].id, subs[1].id]);
        let status = |id| db.get_task(id).unwrap().unwrap().status;
        assert_eq!((status(parent.id), status(subs[0].id), status(subs[1].id), status(subs[2].id)),
            (TaskStatus::Stuck, TaskStatus::Stuck, TaskStatus::Stuck, TaskStatus::Done));
        let ev = db.list_events(Some(subs[0].id), 5).unwrap();
        assert!(ev.iter().any(|e| e.kind == "stopped" && e.payload["reason"] == "owner changed course"));
        assert!(matches!(stop_tree(&db, &runs, subs[2].id, "rui", None), Err(ControlError::Conflict(_))));
        assert!(matches!(stop_tree(&db, &runs, 999, "rui", None), Err(ControlError::NotFound(_))));
    }

    #[tokio::test]
    async fn stop_tree_aborts_running_turns() {
        let (db, runs) = (Db::open_test(), Runs::default());
        let (parent, subs) = pod(&db);
        let join = tokio::spawn(tokio::time::sleep(std::time::Duration::from_secs(3600)));
        runs.lock().unwrap().insert(subs[1].id, join.abort_handle());
        stop_tree(&db, &runs, parent.id, "rui", None).unwrap();
        assert!(join.await.unwrap_err().is_cancelled());
    }

    #[test]
    fn instruct_on_a_split_parent_queues_a_lead_turn() {
        let (db, runs) = (Db::open_test(), Runs::default());
        let (parent, subs) = pod(&db);
        let t = instruct(&db, &runs, parent.id, "exam is Thursday", "rui").unwrap();
        assert_eq!(t.status, TaskStatus::New);
        assert_eq!(db.pending_instructions(parent.id).unwrap().len(), 1);
        let actions = crate::scheduler::plan_tick(&db, &Default::default()).unwrap();
        assert!(actions.iter().any(|a| matches!(a, crate::scheduler::Action::StartLead { task_id, .. } if *task_id == parent.id)));
        assert!(matches!(instruct(&db, &runs, subs[2].id, "x", "rui"), Err(ControlError::Conflict(_))));
    }
}

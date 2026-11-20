//! Hiding and removing threads from the Work board. The state lives in events on the top task:
//! a thread is hidden when its newest `thread_hidden`/`thread_unhidden` is `thread_hidden`, and
//! removed (dropped, hidden for good) once it has a `thread_removed` event.

use crate::control::{self, ControlError};
use crate::db::Db;
use crate::scheduler::Runs;
use anyhow::Result;
use rui_core::{Task, TaskStatus};
use serde_json::json;
use std::collections::HashSet;

/// The event kinds that decide whether a thread is hidden.
pub const KINDS: [&str; 3] = ["thread_hidden", "thread_unhidden", "thread_removed"];

/// Which top-level tasks are hidden, and which of those were removed.
#[derive(Debug, Default)]
pub struct Hidden {
    hidden: HashSet<i64>,
    removed: HashSet<i64>,
}

impl Hidden {
    pub fn is_hidden(&self, id: i64) -> bool {
        self.hidden.contains(&id)
    }

    pub fn is_removed(&self, id: i64) -> bool {
        self.removed.contains(&id)
    }
}

/// Reads every hide/unhide/remove event, oldest first, so the newest one wins.
pub fn hidden(db: &Db) -> Result<Hidden> {
    let mut h = Hidden::default();
    for e in db.events_of_kinds(&KINDS)? {
        let Some(id) = e.task_id else { continue };
        match e.kind.as_str() {
            "thread_hidden" => h.hidden.insert(id),
            "thread_unhidden" => h.hidden.remove(&id),
            _ => h.removed.insert(id),
        };
    }
    Ok(h)
}

/// The top-level task behind a thread id: 404 when unknown, 409 for a subtask.
fn top_task(db: &Db, id: i64) -> Result<Task, ControlError> {
    let task = db.get_task(id)?.ok_or_else(|| ControlError::NotFound("thread not found".into()))?;
    if task.parent_id.is_some() {
        return Err(ControlError::Conflict("only top-level threads can be hidden".into()));
    }
    Ok(task)
}

/// Hides a thread from the board, the Inbox and the needs-you counts.
pub fn hide(db: &Db, id: i64) -> Result<(), ControlError> {
    top_task(db, id)?;
    if hidden(db)?.is_hidden(id) {
        return Ok(());
    }
    db.add_event(Some(id), "owner", "thread_hidden", &json!({"by": "owner"}))?;
    Ok(())
}

/// Brings a hidden thread back. Removed threads stay hidden.
pub fn unhide(db: &Db, id: i64) -> Result<(), ControlError> {
    top_task(db, id)?;
    let h = hidden(db)?;
    if h.is_removed(id) {
        return Err(ControlError::Conflict("removed threads stay hidden".into()));
    }
    if !h.is_hidden(id) {
        return Ok(());
    }
    db.add_event(Some(id), "owner", "thread_unhidden", &json!({"by": "owner"}))?;
    Ok(())
}

/// Drops an unfinished thread with its team, then hides it for good. Returns the dropped ids.
pub fn remove(db: &Db, runs: &Runs, id: i64) -> Result<Vec<i64>, ControlError> {
    let task = top_task(db, id)?;
    let dropped = match task.status {
        TaskStatus::Done => vec![],
        _ => control::drop_task(db, runs, id)?,
    };
    db.add_event(Some(id), "owner", "thread_removed", &json!({"by": "owner"}))?;
    hide(db, id)?;
    Ok(dropped)
}

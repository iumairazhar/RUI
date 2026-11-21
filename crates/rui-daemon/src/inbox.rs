//! The Inbox: everything that needs the owner, in one list. See docs/owner-flow.md.

use crate::db::Db;
use crate::owner_state::{self, OwnerState, PR_CLOSED_REASON, REVIEW_REASON};
use anyhow::Result;
use rui_core::{QuestionStatus, Task, TaskStatus};
use serde::Serialize;
use std::collections::HashSet;

/// One thing waiting for the owner.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InboxItem {
    pub kind: &'static str,
    pub id: i64,
    pub task_id: Option<i64>,
    pub title: String,
    pub reason: String,
    pub actions: Vec<String>,
    pub created_at: String,
    /// Longer text for the item, like the Account Lead's remarks on a review.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Where to act on the item outside RUI, like the PR to merge on GitHub.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or_default().trim().to_string()
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

/// Open owner-level questions.
fn question_items(db: &Db) -> Result<Vec<InboxItem>> {
    let open = db.list_questions(Some(QuestionStatus::Open))?;
    Ok(open
        .into_iter()
        .filter(|q| q.level == "owner")
        .map(|q| InboxItem {
            kind: "question",
            id: q.id,
            task_id: Some(q.task_id),
            title: crate::thread::owner_short(db, &q).unwrap_or_else(|| first_line(&q.text)),
            reason: q.ladder.last().map(|r| r.why.clone()).unwrap_or_else(|| q.asked_by.as_str().to_string()),
            actions: vec![],
            created_at: q.created_at,
            detail: None,
            link: None,
        })
        .collect())
}

/// Owner answers still inside their undo window, each with an "undo" button keyed by the event id.
fn undo_items(db: &Db, now: i64) -> Result<Vec<InboxItem>> {
    let pending = crate::scheduler::pending_approvals(db)?;
    let live = pending.into_iter().filter(|e| crate::scheduler::approval_run_at(e) > now);
    Ok(live
        .map(|e| InboxItem {
            kind: "undo",
            id: e.id,
            task_id: e.task_id,
            title: "Sending your answer\u{2026}".into(),
            reason: format!("Question {}", crate::scheduler::approval_question(&e)),
            actions: strings(&["undo"]),
            created_at: e.created_at,
            detail: None,
            link: None,
        })
        .collect())
}

/// Open chat asks, with their options as buttons.
fn ask_items(db: &Db) -> Result<Vec<InboxItem>> {
    let asks = db.list_chat_asks(Some("open"))?;
    Ok(asks
        .into_iter()
        .map(|a| InboxItem {
            kind: "ask",
            id: a.id,
            task_id: None,
            title: first_line(&a.text),
            reason: format!("Rui asked in chat: {}", a.chat_title),
            actions: a.options,
            created_at: a.created_at,
            detail: None,
            link: None,
        })
        .collect())
}

/// The top-level task a task belongs to, as an id.
fn top_of(t: &Task) -> i64 {
    t.parent_id.unwrap_or(t.id)
}

/// Stuck and review items for top-level tasks whose state says "Needs you".
/// A task with an open owner question on itself or a subtask already has an item.
fn task_items(db: &Db, questioned: &HashSet<i64>) -> Result<Vec<InboxItem>> {
    let tasks = db.list_tasks()?;
    let states = owner_state::all_states(db)?;
    let mut items = Vec::new();
    for (t, s) in tasks.iter().zip(&states) {
        if t.parent_id.is_some() || s.word != "Needs you" || questioned.contains(&t.id) {
            continue;
        }
        if t.status == TaskStatus::Stuck && crate::sandbox::blocked(db, t)?.is_some() {
            continue;
        }
        items.extend(task_item(db, t, s)?);
    }
    Ok(items)
}

fn task_item(db: &Db, t: &Task, s: &OwnerState) -> Result<Option<InboxItem>> {
    let (kind, mut actions) = match (t.status, s.reason.as_str()) {
        (TaskStatus::Done, REVIEW_REASON) => ("review", strings(&["Looks good", "Ask for changes"])),
        // "drop" and "retry" map to POST /tasks/{id}/drop and /tasks/{id}/retry.
        (TaskStatus::Stuck, PR_CLOSED_REASON) => ("pr_closed", strings(&["drop", "retry"])),
        (TaskStatus::Stuck, _) => ("stuck", strings(&["Retry", "New instructions", "Drop it"])),
        _ => return Ok(None),
    };
    let forwarded = if kind == "review" { crate::scheduler::latest_forwarded(db, t.id)? } else { None };
    let link = forwarded.as_ref().and_then(|f| f["pr_url"].as_str()).map(String::from);
    if link.is_some() {
        actions = strings(&["Merge on GitHub", "Ask for changes"]);
    }
    let detail = forwarded.as_ref().map(review_detail);
    Ok(Some(InboxItem { kind, id: t.id, task_id: Some(t.id), title: t.title.clone(), reason: s.reason.clone(), actions, created_at: s.since.clone(), detail, link }))
}

/// The Account Lead's remarks from the latest `forwarded` event, plus the PR link if there is one.
fn review_detail(f: &serde_json::Value) -> String {
    let mut text = f["remarks"].as_str().unwrap_or_default().to_string();
    if let Some(url) = f["pr_url"].as_str() {
        text.push_str(&format!("\nPR: {url}"));
    }
    text
}

/// Checkpointed subtasks whose dependencies are all done: ready for the owner to release.
fn checkpoint_items(db: &Db, tasks: &[Task]) -> Result<Vec<InboxItem>> {
    let done: HashSet<i64> = tasks.iter().filter(|t| t.status == TaskStatus::Done).map(|t| t.id).collect();
    let waiting = tasks.iter().filter(|t| t.parent_id.is_some() && matches!(t.status, TaskStatus::New | TaskStatus::Open));
    let mut items = Vec::new();
    for t in waiting.filter(|t| t.depends_on.iter().all(|d| done.contains(d))) {
        if crate::scheduler::owner_hold(db, t.id)? == Some("checkpoint") {
            items.push(checkpoint_item(t));
        }
    }
    Ok(items)
}

fn checkpoint_item(t: &Task) -> InboxItem {
    InboxItem {
        kind: "checkpoint",
        id: t.id,
        task_id: Some(t.id),
        title: format!("Ready: {} is waiting for you", t.title),
        reason: "Waiting for you to release this step".into(),
        actions: strings(&["Release"]),
        created_at: t.updated_at.clone(),
        detail: None,
        link: None,
    }
}

/// "All seats are used up", while the newest seat gate event says so.
fn seat_items(db: &Db) -> Result<Vec<InboxItem>> {
    let Some(e) = crate::seats::latest_gate_event(db)?.filter(|e| e.kind == "seats_exhausted") else {
        return Ok(vec![]);
    };
    Ok(vec![InboxItem {
        kind: "seats",
        id: e.id,
        task_id: None,
        title: "All seats are used up".into(),
        reason: e.payload["reason"].as_str().unwrap_or_default().to_string(),
        actions: vec![],
        created_at: e.created_at,
        detail: None,
        link: None,
    }])
}

/// Leaves out items that belong to a hidden thread.
fn drop_hidden(db: &Db, tasks: &[Task], items: &mut Vec<InboxItem>) -> Result<()> {
    let hidden = crate::hide::hidden(db)?;
    let top = |id: i64| tasks.iter().find(|t| t.id == id).map_or(id, top_of);
    items.retain(|i| i.task_id.is_none_or(|id| !hidden.is_hidden(top(id))));
    Ok(())
}

/// All inbox items, newest first.
pub fn inbox(db: &Db) -> Result<Vec<InboxItem>> {
    let mut items = question_items(db)?;
    let tasks = db.list_tasks()?;
    let questioned: HashSet<i64> = items
        .iter()
        .filter_map(|i| tasks.iter().find(|t| Some(t.id) == i.task_id))
        .map(top_of)
        .collect();
    items.extend(ask_items(db)?);
    items.extend(undo_items(db, crate::thread::now_secs())?);
    items.extend(task_items(db, &questioned)?);
    items.extend(checkpoint_items(db, &tasks)?);
    items.extend(seat_items(db)?);
    items.extend(crate::sandbox::inbox_items(db, &tasks)?);
    items.extend(crate::scout::inbox_items(db)?);
    items.extend(crate::coach::inbox_items(db)?);
    items.extend(crate::email::inbox_items(db)?);
    items.extend(crate::deploy::inbox_items(db)?);
    items.extend(crate::role_request::inbox_items(db)?);
    drop_hidden(db, &tasks, &mut items)?;
    items.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::NewTask;
    use rui_core::{Role, SourceKind};
    use serde_json::json;

    fn make(db: &Db, client: i64, parent: Option<i64>, status: TaskStatus) -> Task {
        db.create_task(&NewTask {
            client_id: client, parent_id: parent, title: "Plan".into(), body: String::new(), status, done_command: None,
            max_loops: 5, role: Role::Lead, source_kind: SourceKind::User, source_ref: None, agent: None, pod: vec![],
        })
        .unwrap()
    }

    #[test]
    fn lists_questions_asks_stuck_and_review() {
        let db = Db::open_test();
        let c = db.create_client("c", "/tmp/c", None, &[]).unwrap();
        let stuck = make(&db, c.id, None, TaskStatus::Stuck);
        db.add_event(Some(stuck.id), "scheduler", "agent_error", &json!({"error": "boom"})).unwrap();
        let stopped = make(&db, c.id, None, TaskStatus::Stuck);
        db.add_event(Some(stopped.id), "owner", "stopped", &json!({"by": "owner"})).unwrap();
        let asked = make(&db, c.id, None, TaskStatus::Stuck);
        db.create_ladder_question(asked.id, Role::Lead, "owner", &[], "Which one?\nlong").unwrap();
        make(&db, c.id, Some(asked.id), TaskStatus::Stuck);
        let chat = db.create_chat().unwrap();
        db.create_chat_ask(chat.id, None, "Pick", &["A".into(), "B".into()]).unwrap();

        let items = inbox(&db).unwrap();
        let find = |kind: &str| items.iter().filter(|i| i.kind == kind).collect::<Vec<_>>();
        assert_eq!(find("question").len(), 1);
        assert_eq!(find("question")[0].title, "Which one?");
        assert_eq!(find("ask")[0].actions, ["A", "B"]);
        let stuck_items = find("stuck");
        assert_eq!((stuck_items.len(), stuck_items[0].id), (1, stuck.id));
        assert_eq!(stuck_items[0].actions, ["Retry", "New instructions", "Drop it"]);
    }

    #[test]
    fn question_title_uses_the_short_question() {
        let db = Db::open_test();
        let c = db.create_client("c", "/tmp/c", None, &[]).unwrap();
        let t = make(&db, c.id, None, TaskStatus::Stuck);
        let q = db.create_ladder_question(t.id, Role::Lead, "owner", &[], "Long text\nmore").unwrap();
        let p = json!({"question_id": q.id, "from": "rui", "to": "owner", "why": "w", "question": "Short one?"});
        db.add_event(Some(t.id), "scheduler", "escalated", &p).unwrap();
        let items = inbox(&db).unwrap();
        assert_eq!(items.iter().find(|i| i.kind == "question").unwrap().title, "Short one?");
    }

    #[test]
    fn checkpoint_item_only_once_dependencies_are_done() {
        let db = Db::open_test();
        let c = db.create_client("c", "/tmp/c", None, &[]).unwrap();
        let top = make(&db, c.id, None, TaskStatus::Split);
        let dep = make(&db, c.id, Some(top.id), TaskStatus::Open);
        let held = make(&db, c.id, Some(top.id), TaskStatus::Open);
        db.add_task_dep(held.id, dep.id).unwrap();
        db.add_event(Some(held.id), "owner", "checkpoint_set", &json!({})).unwrap();
        let checkpoints = |db: &Db| inbox(db).unwrap().into_iter().filter(|i| i.kind == "checkpoint").collect::<Vec<_>>();
        assert!(checkpoints(&db).is_empty());
        db.set_task_status(dep.id, TaskStatus::Done).unwrap();
        let items = checkpoints(&db);
        assert_eq!((items.len(), items[0].id), (1, held.id));
        assert_eq!(items[0].title, "Ready: Plan is waiting for you");
        assert_eq!(items[0].actions, ["Release"]);
        db.add_event(Some(held.id), "owner", "checkpoint_cleared", &json!({})).unwrap();
        assert!(checkpoints(&db).is_empty());
    }

    #[test]
    fn seats_item_shows_until_a_seat_frees_up() {
        let db = Db::open_test();
        let seats = |db: &Db| inbox(db).unwrap().into_iter().filter(|i| i.kind == "seats").collect::<Vec<_>>();
        assert!(seats(&db).is_empty());
        db.add_event(None, "scheduler", "seats_exhausted", &json!({"reason": "Work: frees up 10:00"})).unwrap();
        let items = seats(&db);
        assert_eq!((items[0].title.as_str(), items[0].reason.as_str()), ("All seats are used up", "Work: frees up 10:00"));
        db.add_event(None, "scheduler", "seats_freed", &json!({})).unwrap();
        assert!(seats(&db).is_empty());
    }

    #[test]
    fn finished_task_becomes_a_review_item() {
        let db = Db::open_test();
        let c = db.create_client("c", "/tmp/c", None, &[]).unwrap();
        let done = make(&db, c.id, None, TaskStatus::Done);
        let state = OwnerState { task_id: done.id, word: "Needs you", reason: REVIEW_REASON.into(), since: done.updated_at.clone() };
        let item = task_item(&db, &done, &state).unwrap().unwrap();
        assert_eq!((item.detail, item.link), (None, None));
        assert_eq!((item.kind, item.actions), ("review", ["Looks good", "Ask for changes"].map(String::from).to_vec()));
    }

    #[test]
    fn review_item_carries_the_account_leads_remarks() {
        let db = Db::open_test();
        let c = db.create_client("c", "/tmp/c", None, &[]).unwrap();
        let done = make(&db, c.id, None, TaskStatus::Done);
        let payload = json!({"remarks": "Check the migration", "pr_url": "https://example.test/pr/1"});
        db.add_event(Some(done.id), "lead", "forwarded", &payload).unwrap();
        let state = OwnerState { task_id: done.id, word: "Needs you", reason: REVIEW_REASON.into(), since: done.updated_at.clone() };
        let item = task_item(&db, &done, &state).unwrap().unwrap();
        assert_eq!(item.detail.as_deref(), Some("Check the migration\nPR: https://example.test/pr/1"));
        assert_eq!(item.actions, ["Merge on GitHub", "Ask for changes"]);
        assert_eq!(item.link.as_deref(), Some("https://example.test/pr/1"));
    }

    #[test]
    fn review_item_without_a_pr_keeps_the_local_actions() {
        let db = Db::open_test();
        let c = db.create_client("c", "/tmp/c", None, &[]).unwrap();
        let done = make(&db, c.id, None, TaskStatus::Done);
        db.add_event(Some(done.id), "lead", "forwarded", &json!({"remarks": "Fine", "pr_url": null})).unwrap();
        let state = OwnerState { task_id: done.id, word: "Needs you", reason: REVIEW_REASON.into(), since: done.updated_at.clone() };
        let item = task_item(&db, &done, &state).unwrap().unwrap();
        assert_eq!(item.actions, ["Looks good", "Ask for changes"]);
        assert_eq!(item.link, None);
    }

    #[test]
    fn merged_pr_has_no_item_and_closed_pr_offers_drop_or_retry() {
        let db = Db::open_test();
        let c = db.create_client("c", "/tmp/c", None, &[]).unwrap();
        let merged = make(&db, c.id, None, TaskStatus::Done);
        db.add_event(Some(merged.id), "lead", "forwarded", &json!({"remarks": "ok", "pr_url": "https://x.test/pr/1"})).unwrap();
        assert_eq!(inbox(&db).unwrap().iter().filter(|i| i.kind == "review").count(), 1);
        db.add_event(Some(merged.id), "github", "pr_merged", &json!({"url": "https://x.test/pr/1"})).unwrap();
        assert!(inbox(&db).unwrap().iter().all(|i| i.task_id != Some(merged.id)));
        let closed = make(&db, c.id, None, TaskStatus::Stuck);
        db.add_event(Some(closed.id), "lead", "forwarded", &json!({"remarks": "ok", "pr_url": "https://x.test/pr/2"})).unwrap();
        db.add_event(Some(closed.id), "github", "pr_closed", &json!({"url": "https://x.test/pr/2"})).unwrap();
        let items = inbox(&db).unwrap();
        let item = items.iter().find(|i| i.task_id == Some(closed.id)).unwrap();
        assert_eq!((item.kind, item.reason.as_str()), ("pr_closed", "PR closed without merge"));
        assert_eq!(item.actions, ["drop", "retry"]);
    }
}

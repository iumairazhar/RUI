//! The Inbox: everything that needs the owner, in one list. See docs/owner-flow.md.

use crate::db::Db;
use crate::owner_state::{self, OwnerState, REVIEW_REASON};
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
            title: first_line(&q.text),
            reason: q.ladder.last().map(|r| r.why.clone()).unwrap_or_else(|| q.asked_by.as_str().to_string()),
            actions: vec![],
            created_at: q.created_at,
            detail: None,
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
        items.extend(task_item(db, t, s)?);
    }
    Ok(items)
}

fn task_item(db: &Db, t: &Task, s: &OwnerState) -> Result<Option<InboxItem>> {
    let (kind, actions) = match (t.status, s.reason.as_str()) {
        (TaskStatus::Done, REVIEW_REASON) => ("review", strings(&["Looks good", "Ask for changes"])),
        (TaskStatus::Stuck, _) => ("stuck", strings(&["Retry", "New instructions", "Drop it"])),
        _ => return Ok(None),
    };
    let detail = if kind == "review" { review_detail(db, t.id)? } else { None };
    Ok(Some(InboxItem { kind, id: t.id, task_id: Some(t.id), title: t.title.clone(), reason: s.reason.clone(), actions, created_at: s.since.clone(), detail }))
}

/// The Account Lead's remarks from the latest `forwarded` event, plus the PR link if there is one.
fn review_detail(db: &Db, task_id: i64) -> Result<Option<String>> {
    let Some(f) = crate::scheduler::latest_forwarded(db, task_id)? else { return Ok(None) };
    let mut text = f["remarks"].as_str().unwrap_or_default().to_string();
    if let Some(url) = f["pr_url"].as_str() {
        text.push_str(&format!("\nPR: {url}"));
    }
    Ok(Some(text))
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
    items.extend(task_items(db, &questioned)?);
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
    fn finished_task_becomes_a_review_item() {
        let db = Db::open_test();
        let c = db.create_client("c", "/tmp/c", None, &[]).unwrap();
        let done = make(&db, c.id, None, TaskStatus::Done);
        let state = OwnerState { task_id: done.id, word: "Needs you", reason: REVIEW_REASON.into(), since: done.updated_at.clone() };
        let item = task_item(&db, &done, &state).unwrap().unwrap();
        assert_eq!(item.detail, None);
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
    }
}

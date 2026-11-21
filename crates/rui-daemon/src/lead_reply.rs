//! The Account Lead's replies to the owner: `lead_reply` events, shown as a thread node and an
//! Inbox item until the owner dismisses them.

use crate::db::Db;
use crate::inbox::InboxItem;
use anyhow::Result;
use rui_core::Event;
use serde_json::json;
use std::collections::HashSet;

pub const EVENT: &str = "lead_reply";
const DISMISSED: &str = "lead_reply_dismissed";
/// Used when the Lead's turn ends with no text of its own.
pub const FALLBACK: &str = "The Lead read your message and continued the work.";
const OWNER_KINDS: [&str; 2] = ["instruction", "approval_pending"];

/// The newest owner instruction or answer on the task, the message this reply answers.
fn latest_owner_event(db: &Db, task_id: i64) -> Result<Option<i64>> {
    let evs = db.events_for_tasks(&[task_id], &OWNER_KINDS, 0, i64::MAX)?;
    Ok(evs.iter().rev().find(|e| e.agent == "owner").map(|e| e.id))
}

/// Records a reply from the Lead on `task_id` and returns its event id.
pub fn record(db: &Db, task_id: i64, text: &str) -> Result<i64> {
    let to = latest_owner_event(db, task_id)?;
    db.add_event(Some(task_id), "lead", EVENT, &json!({"text": text.trim(), "to_event_id": to}))
}

/// True when any of these pending instruction ids was written by the owner.
pub fn any_from_owner(db: &Db, ids: &[i64]) -> Result<bool> {
    if ids.is_empty() {
        return Ok(false);
    }
    Ok(db.events_by_ids(ids)?.iter().any(|e| e.agent == "owner"))
}

/// After a Lead turn that read owner input: adds a reply from `final_text` (or the fallback)
/// unless the turn already replied after event `since`.
pub fn ensure(db: &Db, task_id: i64, since: i64, final_text: Option<&str>) -> Result<()> {
    if !db.events_for_tasks(&[task_id], &[EVENT], since, 1)?.is_empty() {
        return Ok(());
    }
    let text = final_text.map(str::trim).filter(|t| !t.is_empty()).unwrap_or(FALLBACK);
    record(db, task_id, text).map(|_| ())
}

fn item(db: &Db, e: &Event) -> Result<InboxItem> {
    let text = e.payload["text"].as_str().unwrap_or_default();
    let task = e.task_id.map(|id| db.get_task(id)).transpose()?.flatten();
    let client = task.map(|t| db.get_client(t.client_id)).transpose()?.flatten().map(|c| c.name).unwrap_or_default();
    Ok(InboxItem {
        kind: "lead_reply",
        id: e.id,
        task_id: e.task_id,
        title: format!("{client}: the Lead replied"),
        reason: text.chars().take(200).collect(),
        actions: vec!["dismiss".into()],
        created_at: e.created_at.clone(),
        detail: Some(text.to_string()),
        link: None,
    })
}

/// Lead replies the owner has not dismissed, as Inbox items.
pub fn inbox_items(db: &Db) -> Result<Vec<InboxItem>> {
    let evs = db.events_of_kinds(&[EVENT, DISMISSED])?;
    let gone: HashSet<i64> = evs.iter().filter(|e| e.kind == DISMISSED).filter_map(|e| e.payload["event_id"].as_i64()).collect();
    evs.iter().filter(|e| e.kind == EVENT && !gone.contains(&e.id)).map(|e| item(db, e)).collect()
}

/// Hides a reply from the Inbox. False when there is no such open reply.
pub fn dismiss(db: &Db, event_id: i64) -> Result<bool> {
    let Some(found) = inbox_items(db)?.into_iter().find(|i| i.id == event_id) else { return Ok(false) };
    db.add_event(found.task_id, "owner", DISMISSED, &json!({"event_id": event_id}))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::NewTask;
    use rui_core::{Role, SourceKind, TaskStatus};

    fn setup() -> (Db, i64) {
        let db = Db::open_test();
        let c = db.create_client("acme", "/tmp/acme", None, &[]).unwrap();
        let t = db
            .create_task(&NewTask {
                client_id: c.id, parent_id: None, title: "Site".into(), body: String::new(), status: TaskStatus::New,
                done_command: None, max_loops: 5, role: Role::Lead, source_kind: SourceKind::User, source_ref: None,
                agent: None, pod: vec![],
            })
            .unwrap();
        (db, t.id)
    }

    fn payload(db: &Db, id: i64) -> serde_json::Value {
        db.events_by_ids(&[id]).unwrap().remove(0).payload
    }

    #[test]
    fn reply_points_at_the_latest_owner_message() {
        let (db, t) = setup();
        db.add_event(Some(t), "scout", "instruction", &json!({"text": "report"})).unwrap();
        assert_eq!(payload(&db, record(&db, t, "ok").unwrap())["to_event_id"], json!(null));
        let said = db.add_event(Some(t), "owner", "instruction", &json!({"text": "why astro?"})).unwrap();
        let r = record(&db, t, " Astro was picked for static pages. ").unwrap();
        let p = payload(&db, r);
        assert_eq!((p["text"].as_str().unwrap(), p["to_event_id"].as_i64()), ("Astro was picked for static pages.", Some(said)));
    }

    #[test]
    fn ensure_adds_a_reply_only_when_the_turn_gave_none() {
        let (db, t) = setup();
        let since = db.add_event(Some(t), "owner", "instruction", &json!({"text": "fix"})).unwrap();
        ensure(&db, t, since, Some("  ")).unwrap();
        let replies = |db: &Db| db.events_for_tasks(&[t], &[EVENT], 0, 100).unwrap();
        assert_eq!(replies(&db)[0].payload["text"], FALLBACK);
        let since = replies(&db)[0].id;
        record(&db, t, "Fixed the install step.").unwrap();
        ensure(&db, t, since, Some("final words")).unwrap();
        assert_eq!(replies(&db).len(), 2);
        ensure(&db, t, db.max_event_id(&[t]).unwrap(), Some("final words")).unwrap();
        assert_eq!(replies(&db)[2].payload["text"], "final words");
    }

    #[test]
    fn inbox_item_until_dismissed() {
        let (db, t) = setup();
        let id = record(&db, t, "I will retry the build.\nMore detail").unwrap();
        let items = inbox_items(&db).unwrap();
        assert_eq!((items[0].kind, items[0].id, items[0].title.as_str()), ("lead_reply", id, "acme: the Lead replied"));
        assert_eq!(items[0].actions, ["dismiss"]);
        assert!(dismiss(&db, id).unwrap());
        assert!(inbox_items(&db).unwrap().is_empty());
        assert!(!dismiss(&db, id).unwrap());
    }

    #[test]
    fn owner_items_are_told_apart() {
        let (db, t) = setup();
        let a = db.add_event(Some(t), "scout", "instruction", &json!({"text": "r"})).unwrap();
        let b = db.add_event(Some(t), "owner", "instruction", &json!({"text": "o"})).unwrap();
        assert!(!any_from_owner(&db, &[a]).unwrap());
        assert!(any_from_owner(&db, &[a, b]).unwrap());
        assert!(!any_from_owner(&db, &[]).unwrap());
    }
}

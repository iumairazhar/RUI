//! The Account Lead's `request_role`: add a missing library role to its client's Pod mid-task,
//! or ask the owner when the role is missing from the library or switched off.

use crate::client_state::{self, PodRole};
use crate::db::Db;
use crate::inbox::InboxItem;
use crate::library::{self, Kind};
use crate::roster;
use anyhow::{Result, anyhow, bail};
use rui_core::{Client, Role, Rung, Task};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::Path;

const ADDED: &str = "role_added";
const DISMISSED: &str = "role_added_dismissed";

/// Checks the role name (a simple slug, not "lead") and the reason; returns them trimmed.
fn check_args<'a>(role: &'a str, why: &'a str) -> Result<(&'a str, &'a str)> {
    let (role, why) = (role.trim(), why.trim());
    library::valid_name(role).map_err(|_| anyhow!("role must be a simple name like 'designer' (lowercase letters, digits, hyphens)"))?;
    if role == "lead" {
        bail!("the Account Lead is always in the Pod");
    }
    if why.is_empty() {
        bail!("why must not be empty");
    }
    Ok((role, why))
}

/// Why a library role can't be added, or None when it exists and is switched on.
fn unavailable(data_dir: &Path, role: &str) -> Option<&'static str> {
    if roster::load(data_dir, role).is_none() {
        return Some("It isn't in your role library.");
    }
    if !library::is_on(data_dir, Kind::Role, role) {
        return Some("It is switched off in your role library.");
    }
    None
}

/// Puts `role` on the task's own pod so create_subtask accepts it. Old tasks with no pod accept any role.
fn add_to_task(db: &Db, task: &Task, role: &str) -> Result<()> {
    if task.pod.is_empty() || task.pod.iter().any(|p| p == role) {
        return Ok(());
    }
    let mut pod = task.pod.clone();
    pod.push(role.to_string());
    db.set_task_pod(task.id, &pod)
}

/// Adds the role or asks the owner. Returns `{added, reason?}`.
pub fn request(db: &Db, data_dir: &Path, task: &Task, role: &str, why: &str) -> Result<Value> {
    let (role, why) = check_args(role, why)?;
    let client = db.get_client(task.client_id)?.ok_or_else(|| anyhow!("client of task {} not found", task.id))?;
    let mut pod = client_state::read_pod(data_dir, &client.name)?;
    if pod.roles.iter().any(|r| r.role == role) {
        add_to_task(db, task, role)?;
        return Ok(json!({"added": false, "reason": "already in the Pod"}));
    }
    if let Some(missing) = unavailable(data_dir, role) {
        ask_owner(db, task, role, why, missing)?;
        return Ok(json!({"added": false, "reason": format!("{missing} Asked the owner.")}));
    }
    pod.roles.push(PodRole { role: role.to_string(), skills: vec![] });
    client_state::write_pod(data_dir, &client.name, &pod)?;
    add_to_task(db, task, role)?;
    record_added(db, data_dir, task, &client, role, why)?;
    Ok(json!({"added": true}))
}

/// The `role_added` event and the decisions-log line.
fn record_added(db: &Db, data_dir: &Path, task: &Task, client: &Client, role: &str, why: &str) -> Result<()> {
    let title = roster::title_of(data_dir, role);
    let payload = json!({"role": role, "why": why, "by": "lead", "title": title, "client": client.name});
    db.add_event(Some(task.id), "lead", ADDED, &payload)?;
    let line = format!("Task {}: added {role} to the Pod (by Lead): {why}", task.id);
    client_state::append_decision(data_dir, &client.name, &line)
}

/// An owner-level question on the Lead's task, with tap options.
fn ask_owner(db: &Db, task: &Task, role: &str, why: &str, missing: &str) -> Result<()> {
    let text = format!("The Lead wants a {role} role for {}: {why}. {missing}", task.title);
    let ladder = [Rung { who: "Lead".into(), why: why.to_string() }];
    let q = db.create_ladder_question(task.id, Role::Lead, "owner", &ladder, &text)?;
    let short: String = format!("Add a {role} role to the library?").chars().take(140).collect();
    let options = ["Add it to the library", "Do without it"];
    let payload = json!({"question_id": q.id, "from": "lead", "to": "owner", "why": why, "question": short, "options": options});
    db.add_event(Some(task.id), "lead", "escalated", &payload)?;
    db.add_event(Some(task.id), "lead", "question_raised", &json!({"question_id": q.id, "level": "owner"}))?;
    Ok(())
}

/// `role_added` events not yet dismissed, as Inbox items.
pub fn inbox_items(db: &Db) -> Result<Vec<InboxItem>> {
    let evs = db.events_of_kinds(&[ADDED, DISMISSED])?;
    let gone: HashSet<i64> = evs.iter().filter(|e| e.kind == DISMISSED).filter_map(|e| e.payload["event_id"].as_i64()).collect();
    let open = evs.iter().filter(|e| e.kind == ADDED && !gone.contains(&e.id));
    Ok(open.map(inbox_item).collect())
}

fn inbox_item(e: &rui_core::Event) -> InboxItem {
    let p = &e.payload;
    let title = p["title"].as_str().or(p["role"].as_str()).unwrap_or_default();
    InboxItem {
        kind: ADDED,
        id: e.id,
        task_id: e.task_id,
        title: format!("Added {title} to {}'s Pod", p["client"].as_str().unwrap_or_default()),
        reason: p["why"].as_str().unwrap_or_default().to_string(),
        actions: vec!["dismiss".into()],
        created_at: e.created_at.clone(),
        detail: None,
        link: None,
    }
}

/// Hides a `role_added` item from the Inbox. False when there is no such open item.
pub fn dismiss(db: &Db, event_id: i64) -> Result<bool> {
    let Some(item) = inbox_items(db)?.into_iter().find(|i| i.id == event_id) else { return Ok(false) };
    db.add_event(item.task_id, "owner", DISMISSED, &json!({"event_id": event_id}))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::NewTask;
    use rui_core::{QuestionStatus, SourceKind, TaskStatus};

    fn add_role(data: &Path, name: &str) {
        let dir = data.join("roles").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let text = format!("---\nname: {name}\ntitle: The {name}\ndescription: d\n---\nI am {name}.\n");
        std::fs::write(dir.join("AGENT.md"), text).unwrap();
    }

    /// Db, data dir with roles backend and designer, client Acme whose Pod is [backend], and a Lead task.
    fn setup() -> (Db, tempfile::TempDir, Task) {
        let (db, data) = (Db::open_test(), tempfile::tempdir().unwrap());
        add_role(data.path(), "backend");
        add_role(data.path(), "designer");
        let c = db.create_client("Acme", "/tmp/acme", None, &[]).unwrap();
        client_state::ensure(data.path(), "Acme", "/tmp/acme", None).unwrap();
        let pod = client_state::Pod { roles: vec![PodRole { role: "backend".into(), skills: vec![] }], ..Default::default() };
        client_state::write_pod(data.path(), "Acme", &pod).unwrap();
        let task = db
            .create_task(&NewTask {
                client_id: c.id, parent_id: None, title: "Landing page".into(), body: String::new(), status: TaskStatus::Split,
                done_command: None, max_loops: 5, role: Role::Lead, source_kind: SourceKind::Chat, source_ref: None,
                agent: Some("lead".into()), pod: vec!["lead".into(), "backend".into()],
            })
            .unwrap();
        (db, data, task)
    }

    fn pod_names(data: &Path) -> Vec<String> {
        client_state::read_pod(data, "Acme").unwrap().roles.into_iter().map(|r| r.role).collect()
    }

    #[test]
    fn a_role_already_in_the_pod_changes_nothing() {
        let (db, data, task) = setup();
        let r = request(&db, data.path(), &task, "backend", "needs an API").unwrap();
        assert_eq!(r, json!({"added": false, "reason": "already in the Pod"}));
        assert_eq!(pod_names(data.path()), ["backend"]);
        assert!(db.events_of_kinds(&[ADDED]).unwrap().is_empty());
    }

    #[test]
    fn a_switched_on_library_role_is_added_with_event_and_log_line() {
        let (db, data, task) = setup();
        let r = request(&db, data.path(), &task, "designer", "the page needs a layout").unwrap();
        assert_eq!(r, json!({"added": true}));
        let pod = client_state::read_pod(data.path(), "Acme").unwrap();
        assert_eq!(pod.roles[1], PodRole { role: "designer".into(), skills: vec![] });
        let evs = db.events_of_kinds(&[ADDED]).unwrap();
        assert_eq!((evs[0].task_id, &evs[0].payload["role"], &evs[0].payload["by"]), (Some(task.id), &json!("designer"), &json!("lead")));
        assert_eq!(evs[0].payload["why"], "the page needs a layout");
        let log = client_state::read_log(data.path(), "Acme", 10).unwrap();
        assert!(log.contains("added designer to the Pod (by Lead): the page needs a layout"), "{log}");
        assert!(db.get_task(task.id).unwrap().unwrap().pod.contains(&"designer".to_string()));
    }

    #[test]
    fn a_missing_or_switched_off_role_asks_the_owner_and_leaves_the_pod() {
        let (db, data, task) = setup();
        library::set_status(data.path(), Kind::Role, "designer", "off").unwrap();
        for role in ["designer", "copywriter"] {
            let r = request(&db, data.path(), &task, role, "needs words").unwrap();
            assert_eq!(r["added"], false);
            assert!(r["reason"].as_str().unwrap().contains("role library"), "{r}");
        }
        assert_eq!(pod_names(data.path()), ["backend"]);
        let qs = db.questions_for_task(task.id).unwrap();
        assert_eq!(qs.len(), 2);
        assert!(qs.iter().all(|q| q.level == "owner" && q.status == QuestionStatus::Open));
        assert!(qs[0].text.starts_with("The Lead wants a designer role for Landing page: needs words."), "{}", qs[0].text);
        assert!(qs[1].text.contains("isn't in your role library"));
        let esc = db.events_of_kinds(&["escalated"]).unwrap();
        assert_eq!(esc[0].payload["options"], json!(["Add it to the library", "Do without it"]));
        assert!(db.events_of_kinds(&[ADDED]).unwrap().is_empty());
    }

    #[test]
    fn bad_names_and_empty_reasons_are_rejected() {
        let (db, data, task) = setup();
        for (role, why) in [("Design Team", "x"), ("../x", "x"), ("", "x"), ("lead", "x"), ("designer", "  ")] {
            assert!(request(&db, data.path(), &task, role, why).is_err(), "{role:?} {why:?}");
        }
        assert_eq!(pod_names(data.path()), ["backend"]);
    }

    #[test]
    fn inbox_shows_role_added_until_dismissed() {
        let (db, data, task) = setup();
        request(&db, data.path(), &task, "designer", "the page needs a layout").unwrap();
        let items: Vec<InboxItem> = crate::inbox::inbox(&db).unwrap().into_iter().filter(|i| i.kind == ADDED).collect();
        assert_eq!(items.len(), 1);
        assert_eq!((items[0].title.as_str(), items[0].reason.as_str()), ("Added The designer to Acme's Pod", "the page needs a layout"));
        assert_eq!((items[0].task_id, items[0].actions.clone()), (Some(task.id), vec!["dismiss".to_string()]));
        assert!(dismiss(&db, items[0].id).unwrap());
        assert!(!dismiss(&db, items[0].id).unwrap());
        assert!(crate::inbox::inbox(&db).unwrap().iter().all(|i| i.kind != ADDED));
    }
}

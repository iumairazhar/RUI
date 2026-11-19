//! The owner's view of a task: one of five plain words and a reason, worked out from
//! the task, its events and open questions. Nothing here is stored. See docs/owner-flow.md.

use crate::db::Db;
use anyhow::Result;
use rui_core::{Event, Question, QuestionStatus, Task, TaskStatus};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;

/// The reason line of a finished task that waits for the owner's review.
pub const REVIEW_REASON: &str = "Finished, review the result";
/// The reason line of a task whose PR was closed on GitHub without a merge.
pub const PR_CLOSED_REASON: &str = "PR closed without merge";
const CAUSES: [&str; 8] =
    ["stopped", "scheduler_error", "agent_error", "land_failed", "lead_no_outcome", "rollup", "loop_limit", "pr_closed"];
const KINDS: [&str; 14] = [
    "stopped", "scheduler_error", "agent_error", "land_failed", "lead_no_outcome", "rollup", "loop_limit", "resumed", "dropped",
    "reviewed", "pr_closed", "pr_merged", "forwarded", "escalated",
];

/// What the owner sees for one task.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OwnerState {
    pub task_id: i64,
    pub word: &'static str,
    pub reason: String,
    pub since: String,
}

/// Events and open owner questions per task id.
#[derive(Default)]
struct Facts {
    events: HashMap<i64, Vec<Event>>,
    questions: HashMap<i64, Question>,
}

impl Facts {
    fn events_of(&self, id: i64) -> &[Event] {
        self.events.get(&id).map(Vec::as_slice).unwrap_or(&[])
    }
}

/// The state of one task. `events` are this task's relevant events, oldest first.
/// A split parent only sees its subtasks' status here; `all_states` gives it their events too.
pub fn state(task: &Task, events: &[Event], all: &[Task], open_owner_q: Option<&Question>) -> OwnerState {
    let mut facts = Facts::default();
    facts.events.insert(task.id, events.to_vec());
    if let Some(q) = open_owner_q {
        facts.questions.insert(task.id, q.clone());
    }
    state_in(task, &facts, all)
}

/// The state of every task.
pub fn all_states(db: &Db) -> Result<Vec<OwnerState>> {
    let all = db.list_tasks()?;
    let mut facts = Facts::default();
    for e in db.events_of_kinds(&KINDS)? {
        for id in event_task_ids(&e) {
            facts.events.entry(id).or_default().push(e.clone());
        }
    }
    for q in db.list_questions(Some(QuestionStatus::Open))?.into_iter().filter(|q| q.level == "owner") {
        facts.questions.entry(q.task_id).or_insert(q);
    }
    Ok(all.iter().map(|t| state_in(t, &facts, &all)).collect())
}

/// The tasks an event is about. A `resumed` event also covers every id it lists.
fn event_task_ids(e: &Event) -> Vec<i64> {
    let mut ids: Vec<i64> = e.task_id.into_iter().collect();
    if e.kind == "resumed" {
        let listed = e.payload["ids"].as_array().into_iter().flatten().filter_map(Value::as_i64);
        ids.extend(listed.filter(|i| Some(*i) != e.task_id));
    }
    ids
}

fn state_in(task: &Task, facts: &Facts, all: &[Task]) -> OwnerState {
    let (word, reason) = decide(task, facts, all);
    OwnerState { task_id: task.id, word, reason, since: task.updated_at.clone() }
}

fn decide(t: &Task, facts: &Facts, all: &[Task]) -> (&'static str, String) {
    if let Some(q) = facts.questions.get(&t.id) {
        let short = crate::thread::owner_escalation(facts.events_of(t.id), q).and_then(|e| crate::thread::short_question(&e.payload));
        return ("Needs you", short.unwrap_or_else(|| first_line(&q.text)));
    }
    if parent_dropped(t, facts) {
        return ("Done", "Dropped by you".into());
    }
    let ev = facts.events_of(t.id);
    match t.status {
        TaskStatus::Stuck => stuck_state(t, facts, all),
        TaskStatus::Done => done_state(t, ev),
        TaskStatus::Clarifying | TaskStatus::InProgress => ("Working", format!("{} is working", who(t))),
        TaskStatus::WaitingAnswer => ("Waiting", "Waiting for the Lead to answer".into()),
        TaskStatus::Split => split_state(t, facts, all),
        TaskStatus::New | TaskStatus::Open => blocked_state(t, facts, all),
    }
}

/// A teammate of a dropped task is dropped too, unless its Lead was resumed afterwards.
fn parent_dropped(t: &Task, facts: &Facts) -> bool {
    let Some(pid) = t.parent_id else { return false };
    let ev = facts.events_of(pid);
    latest_id(ev, "dropped") > latest_id(ev, "resumed")
}

/// Who is doing the work, in words.
fn who(t: &Task) -> String {
    match (&t.agent, t.parent_id) {
        (Some(a), _) => a.clone(),
        (None, None) => "The Lead".into(),
        (None, Some(_)) => "The builder".into(),
    }
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or_default().trim().to_string()
}

/// Id of the newest event of the given kind, or 0.
fn latest_id(ev: &[Event], kind: &str) -> i64 {
    ev.iter().filter(|e| e.kind == kind).map(|e| e.id).max().unwrap_or(0)
}

fn stuck_state(t: &Task, facts: &Facts, all: &[Task]) -> (&'static str, String) {
    let ev = facts.events_of(t.id);
    let resumed = latest_id(ev, "resumed");
    if latest_id(ev, "dropped") > resumed {
        return ("Done", "Dropped by you".into());
    }
    let cause = ev.iter().filter(|e| e.id > resumed && is_cause(e)).max_by_key(|e| e.id);
    let Some(e) = cause else { return ("Needs you", "Stuck".into()) };
    match e.kind.as_str() {
        "stopped" => ("Stopped", stopped_reason(&e.payload)),
        "rollup" => rollup_state(t, facts, all),
        _ => ("Needs you", cause_reason(e)),
    }
}

/// A parent stuck because of its subtasks takes their word: it only needs the owner when a
/// subtask is really stuck; if they were only stopped, the parent counts as stopped too.
fn rollup_state(t: &Task, facts: &Facts, all: &[Task]) -> (&'static str, String) {
    let stuck = all.iter().filter(|s| s.parent_id == Some(t.id) && s.status == TaskStatus::Stuck);
    let states: Vec<(&Task, (&'static str, String))> = stuck.map(|s| (s, decide(s, facts, all))).collect();
    if let Some((s, (_, reason))) = states.iter().find(|(_, (w, _))| *w == "Needs you") {
        return ("Needs you", format!("{}: {reason}", s.title));
    }
    match states.first() {
        Some((s, _)) => ("Stopped", format!("{} was stopped", s.title)),
        None => ("Needs you", "A teammate is stuck".into()),
    }
}

fn is_cause(e: &Event) -> bool {
    match e.kind.as_str() {
        "rollup" => e.payload["status"] == "stuck",
        k => CAUSES.contains(&k),
    }
}

fn stopped_reason(p: &Value) -> String {
    let source = p["source"].as_str().filter(|s| !s.is_empty());
    let mut text = match (p["by"].as_str(), source) {
        (Some("rui"), _) => "Stopped by Rui".to_string(),
        (Some("owner"), Some(s)) => format!("Stopped by you from {s}"),
        (Some("owner"), None) => "Stopped by you".to_string(),
        _ => "Stopped by the system".to_string(),
    };
    if let Some(r) = p["reason"].as_str().filter(|r| !r.trim().is_empty()) {
        text.push_str(&format!(": {r}"));
    }
    text
}

fn cause_reason(e: &Event) -> String {
    match e.kind.as_str() {
        "scheduler_error" | "agent_error" => format!("Failed: {}", first_line(e.payload["error"].as_str().unwrap_or_default())),
        "land_failed" => format!("Couldn't merge: conflicts in {} files", file_count(&e.payload["files"])),
        "lead_no_outcome" => "The Lead stopped without a plan".into(),
        "rollup" => "A teammate is stuck".into(),
        "loop_limit" => "Hit its loop limit".into(),
        "pr_closed" => PR_CLOSED_REASON.into(),
        _ => "Stuck".into(),
    }
}

fn file_count(files: &Value) -> usize {
    files.as_array().map(Vec::len).or_else(|| files.as_u64().map(|n| n as usize)).unwrap_or(0)
}

fn done_state(t: &Task, ev: &[Event]) -> (&'static str, String) {
    if latest_id(ev, "dropped") > 0 {
        return ("Done", "Dropped by you".into());
    }
    let approved = ev.iter().filter(|e| e.kind == "reviewed" && e.payload["verdict"] == "ok").map(|e| e.id).max().unwrap_or(0);
    let forwarded = latest_id(ev, "forwarded");
    if t.parent_id.is_none() && latest_id(ev, "pr_merged") > forwarded && forwarded > 0 {
        return ("Done", "Merged on GitHub".into());
    }
    if t.parent_id.is_none() && forwarded > approved {
        return ("Needs you", REVIEW_REASON.into());
    }
    if approved > 0 {
        return ("Done", "Reviewed by you".into());
    }
    ("Done", "Finished".into())
}

/// A word for a task someone else is waiting on, without looking deeper.
fn shallow_word(t: &Task, facts: &Facts) -> &'static str {
    match t.status {
        TaskStatus::Stuck if facts.events_of(t.id).iter().any(|e| e.kind == "stopped") => "Stopped",
        TaskStatus::Stuck => "Stuck",
        TaskStatus::InProgress | TaskStatus::Clarifying => "Working",
        _ => "Waiting",
    }
}

fn blocked_state(t: &Task, facts: &Facts, all: &[Task]) -> (&'static str, String) {
    let find = |id: i64| all.iter().find(|x| x.id == id);
    let parent = t.parent_id.and_then(find).filter(|p| p.status == TaskStatus::Stuck);
    if let Some(p) = parent {
        let stopped = shallow_word(p, facts) == "Stopped";
        return ("Waiting", format!("Its Lead is {}", if stopped { "stopped" } else { "stuck" }));
    }
    let dep = t.depends_on.iter().filter_map(|d| find(*d)).find(|d| d.status != TaskStatus::Done);
    match dep {
        Some(d) => ("Waiting", format!("Waiting on {} ({})", d.title, shallow_word(d, facts))),
        None => ("Waiting", "Ready, waiting for a free seat".into()),
    }
}

/// Lower is more urgent.
fn urgency(word: &str) -> usize {
    ["Needs you", "Working", "Waiting", "Stopped", "Done"].iter().position(|w| *w == word).unwrap_or(5)
}

fn split_state(t: &Task, facts: &Facts, all: &[Task]) -> (&'static str, String) {
    let subs = all.iter().filter(|s| s.parent_id == Some(t.id));
    let states = subs.map(|s| (s, decide(s, facts, all)));
    match states.min_by_key(|(_, (word, _))| urgency(word)) {
        Some((s, (word, reason))) => (word, format!("{}: {reason}", s.title)),
        None => ("Working", "The Lead is planning".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rui_core::{Role, SourceKind};
    use serde_json::json;

    fn task(id: i64, parent: Option<i64>, status: TaskStatus, updated: &str) -> Task {
        Task {
            id,
            client_id: 1,
            parent_id: parent,
            title: format!("T{id}"),
            body: String::new(),
            status,
            done_command: None,
            max_loops: 5,
            loops_used: 0,
            role: if parent.is_some() { Role::Builder } else { Role::Lead },
            branch: None,
            source_kind: SourceKind::User,
            source_ref: None,
            depends_on: vec![],
            skill: None,
            agent: None,
            pod: vec![],
            created_at: updated.into(),
            updated_at: updated.into(),
        }
    }

    fn ev(id: i64, task_id: i64, kind: &str, payload: Value) -> Event {
        Event { id, task_id: Some(task_id), chat_id: None, agent: "x".into(), kind: kind.into(), payload, created_at: String::new() }
    }

    const NEW: &str = "2026-11-19T10:00:00.000Z";

    fn word(t: &Task, events: &[Event], all: &[Task]) -> (&'static str, String) {
        let s = state(t, events, all, None);
        (s.word, s.reason)
    }

    #[test]
    fn open_owner_question_needs_you() {
        let t = task(1, None, TaskStatus::WaitingAnswer, NEW);
        let q = Question {
            id: 1, task_id: 1, asked_by: Role::Lead, text: "Which level?\nmore".into(), answer: None,
            status: QuestionStatus::Open, created_at: NEW.into(), level: "owner".into(), ladder: vec![], why: None,
        };
        let s = state(&t, &[], &[], Some(&q));
        assert_eq!((s.word, s.reason.as_str()), ("Needs you", "Which level?"));
    }

    #[test]
    fn stopped_names_source_and_reason() {
        let t = task(1, None, TaskStatus::Stuck, NEW);
        let e = ev(1, 1, "stopped", json!({"by": "owner", "source": "Work stop all", "reason": "wrong plan"}));
        assert_eq!(word(&t, &[e], &[]), ("Stopped", "Stopped by you from Work stop all: wrong plan".into()));
        let e = ev(1, 1, "stopped", json!({"by": "rui", "source": "Rui chat", "reason": "changed"}));
        assert_eq!(word(&t, &[e], &[]), ("Stopped", "Stopped by Rui: changed".into()));
        let e = ev(1, 1, "stopped", json!({"by": "owner"}));
        assert_eq!(word(&t, &[e], &[]), ("Stopped", "Stopped by you".into()));
    }

    #[test]
    fn error_after_resume_needs_you() {
        let t = task(1, None, TaskStatus::Stuck, NEW);
        let events = [
            ev(1, 1, "stopped", json!({"by": "owner"})),
            ev(2, 1, "resumed", json!({"ids": [1]})),
            ev(3, 1, "agent_error", json!({"error": "boom\ntrace"})),
        ];
        assert_eq!(word(&t, &events, &[]), ("Needs you", "Failed: boom".into()));
        assert_eq!(word(&t, &events[..2], &[]), ("Needs you", "Stuck".into()));
    }

    #[test]
    fn other_stuck_causes_have_plain_reasons() {
        let t = task(1, None, TaskStatus::Stuck, NEW);
        let one = |kind: &str, p: Value| word(&t, &[ev(1, 1, kind, p)], &[]).1;
        assert_eq!(one("land_failed", json!({"files": ["a", "b"]})), "Couldn't merge: conflicts in 2 files");
        assert_eq!(one("lead_no_outcome", json!({})), "The Lead stopped without a plan");
        assert_eq!(one("rollup", json!({"status": "stuck"})), "A teammate is stuck");
        assert_eq!(one("scheduler_error", json!({"error": "bad"})), "Failed: bad");
    }

    #[test]
    fn dropped_is_done() {
        let t = task(1, None, TaskStatus::Stuck, NEW);
        let events = [ev(1, 1, "stopped", json!({"by": "owner"})), ev(2, 1, "dropped", json!({"by": "owner"}))];
        assert_eq!(word(&t, &events, &[]), ("Done", "Dropped by you".into()));
    }

    #[test]
    fn finished_work_needs_review_only_when_forwarded() {
        let top = task(1, None, TaskStatus::Done, NEW);
        let member = task(3, Some(1), TaskStatus::Done, NEW);
        let fwd = ev(1, 1, "forwarded", json!({"remarks": "ok"}));
        assert_eq!(word(&top, &[], &[]), ("Done", "Finished".into()));
        assert_eq!(word(&top, std::slice::from_ref(&fwd), &[]), ("Needs you", REVIEW_REASON.into()));
        let member_fwd = [ev(1, 3, "forwarded", json!({}))];
        assert_eq!(word(&member, &member_fwd, &[]), ("Done", "Finished".into()));
        let ok = [fwd.clone(), ev(2, 1, "reviewed", json!({"verdict": "ok"}))];
        assert_eq!(word(&top, &ok, &[]), ("Done", "Reviewed by you".into()));
        let changes = [fwd.clone(), ev(2, 1, "reviewed", json!({"verdict": "changes"}))];
        assert_eq!(word(&top, &changes, &[]).0, "Needs you");
        let again = [fwd, ev(2, 1, "reviewed", json!({"verdict": "ok"})), ev(3, 1, "forwarded", json!({}))];
        assert_eq!(word(&top, &again, &[]), ("Needs you", REVIEW_REASON.into()));
    }

    #[test]
    fn running_tasks_are_working() {
        let mut t = task(1, None, TaskStatus::InProgress, NEW);
        assert_eq!(word(&t, &[], &[]), ("Working", "The Lead is working".into()));
        t.agent = Some("Researcher 01".into());
        assert_eq!(word(&t, &[], &[]), ("Working", "Researcher 01 is working".into()));
        t.status = TaskStatus::WaitingAnswer;
        assert_eq!(word(&t, &[], &[]), ("Waiting", "Waiting for the Lead to answer".into()));
    }

    #[test]
    fn open_task_waits_on_dependency_or_stopped_lead() {
        let lead = task(1, None, TaskStatus::Stuck, NEW);
        let dep = task(2, Some(1), TaskStatus::InProgress, NEW);
        let mut t = task(3, Some(1), TaskStatus::Open, NEW);
        t.depends_on = vec![2];
        let all = [lead.clone(), dep, t.clone()];
        assert_eq!(word(&t, &[], &all), ("Waiting", "Its Lead is stuck".into()));
        let mut all = all.to_vec();
        all[0].status = TaskStatus::Split;
        assert_eq!(word(&t, &[], &all), ("Waiting", "Waiting on T2 (Working)".into()));
        all[1].status = TaskStatus::Done;
        assert_eq!(word(&t, &[], &all), ("Waiting", "Ready, waiting for a free seat".into()));
    }

    #[test]
    fn teammates_of_a_dropped_task_are_dropped() {
        let lead = task(1, None, TaskStatus::Done, NEW);
        let member = task(2, Some(1), TaskStatus::Stuck, NEW);
        let all = [lead, member.clone()];
        let mut facts = Facts::default();
        facts.events.insert(1, vec![ev(5, 1, "dropped", json!({"by": "owner"}))]);
        facts.events.insert(2, vec![ev(4, 2, "stopped", json!({"by": "owner"}))]);
        assert_eq!(decide(&member, &facts, &all), ("Done", "Dropped by you".into()));
        facts.events.get_mut(&1).unwrap().push(ev(6, 1, "resumed", json!({"ids": [1]})));
        assert_eq!(decide(&member, &facts, &all).0, "Stopped");
    }

    #[test]
    fn rollup_stuck_parent_takes_its_subtasks_word() {
        let lead = task(1, None, TaskStatus::Stuck, NEW);
        let researcher = task(2, Some(1), TaskStatus::Stuck, NEW);
        let writer = task(3, Some(1), TaskStatus::Open, NEW);
        let all = [lead.clone(), researcher, writer];
        let mut facts = Facts::default();
        facts.events.insert(1, vec![ev(5, 1, "rollup", json!({"status": "stuck"}))]);
        facts.events.insert(2, vec![ev(4, 2, "stopped", json!({"by": "owner"}))]);
        assert_eq!(decide(&lead, &facts, &all), ("Stopped", "T2 was stopped".into()));
        facts.events.insert(2, vec![ev(4, 2, "agent_error", json!({"error": "boom"}))]);
        assert_eq!(decide(&lead, &facts, &all), ("Needs you", "T2: Failed: boom".into()));
    }

    #[test]
    fn split_parent_shows_most_urgent_subtask() {
        let parent = task(1, None, TaskStatus::Split, NEW);
        let all = [
            parent.clone(),
            task(2, Some(1), TaskStatus::Done, NEW),
            task(3, Some(1), TaskStatus::Open, NEW),
            task(4, Some(1), TaskStatus::InProgress, NEW),
        ];
        assert_eq!(word(&parent, &[], &all), ("Working", "T4: The builder is working".into()));
    }

    #[test]
    fn all_states_reads_events_and_questions_from_the_db() {
        use crate::db::NewTask;
        let db = Db::open_test();
        let c = db.create_client("c", "/tmp/c", None, &[]).unwrap();
        let make = |status| {
            db.create_task(&NewTask {
                client_id: c.id, parent_id: None, title: "t".into(), body: String::new(), status, done_command: None,
                max_loops: 5, role: Role::Lead, source_kind: SourceKind::User, source_ref: None, agent: None, pod: vec![],
            })
            .unwrap()
        };
        let stopped = make(TaskStatus::Stuck);
        db.add_event(Some(stopped.id), "owner", "stopped", &json!({"by": "owner", "source": "Task page"})).unwrap();
        let asked = make(TaskStatus::WaitingAnswer);
        db.create_ladder_question(asked.id, Role::Lead, "owner", &[], "Pick one").unwrap();
        let states = all_states(&db).unwrap();
        assert_eq!((states[0].word, states[0].reason.as_str()), ("Stopped", "Stopped by you from Task page"));
        assert_eq!((states[1].word, states[1].reason.as_str()), ("Needs you", "Pick one"));
    }

    #[test]
    fn loop_limit_on_a_subtask_needs_you() {
        let parent = task(1, None, TaskStatus::Stuck, NEW);
        let sub = task(2, Some(1), TaskStatus::Stuck, NEW);
        let events = [ev(1, 1, "rollup", json!({"status": "stuck"})), ev(2, 2, "loop_limit", json!({"loops_used": 5, "max_loops": 5}))];
        let mut facts = Facts::default();
        for e in &events {
            facts.events.entry(e.task_id.unwrap()).or_default().push(e.clone());
        }
        let s = state_in(&parent, &facts, &[parent.clone(), sub]);
        assert_eq!(s.word, "Needs you");
        assert!(s.reason.contains("loop limit"), "{}", s.reason);
    }
}

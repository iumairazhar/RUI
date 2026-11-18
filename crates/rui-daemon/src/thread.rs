//! Thread projector: one top-level task seen as a conversation of nodes (what you asked, what the
//! team did, what waits for you, what came out). Read-only: built from tasks, questions and a few
//! event kinds each time. Assistant and tool-result events never become nodes.

use crate::db::Db;
use anyhow::Result;
use rui_core::{Event, Question, QuestionStatus, Rung, Task, TaskStatus};
use serde::Serialize;
use std::collections::HashMap;

/// A running task with no event for this long counts as stalled.
pub const STALL_SECS: i64 = 600;
/// Events per inspector page.
pub const PAGE: usize = 200;
const TAIL_CHARS: usize = 2000;
/// The only event kinds node building reads.
const KINDS: [&str; 14] = [
    "instruction", "sent_back", "forwarded", "landed", "land_failed", "loop_limit", "apps_started", "done_check", "stopped",
    "dropped", "agent_error", "reviewed", "resumed", "escalated",
];

#[derive(Debug, Clone, Serialize)]
pub struct ClientRef {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Attempts {
    pub used: i64,
    pub max: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReviewRef {
    pub kind: &'static str,
    pub task_id: i64,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Decision {
    pub source: &'static str,
    pub question_id: Option<i64>,
    pub ask: String,
    pub why: String,
    pub ladder: Vec<Rung>,
    pub risk: &'static str,
    pub reversible: Option<bool>,
    pub if_approved: Vec<String>,
    pub options: Vec<String>,
    pub review: Option<ReviewRef>,
    pub actions: Vec<String>,
    pub undo_secs: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Output {
    pub kind: &'static str,
    pub label: String,
    pub url: Option<String>,
    pub preview: Option<String>,
    pub output_tail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Node {
    pub id: String,
    pub lane: &'static str,
    pub kind: &'static str,
    pub status: &'static str,
    pub title: String,
    pub agent: Option<String>,
    pub subtask_id: Option<i64>,
    pub attempts: Option<Attempts>,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub after: Vec<String>,
    pub decision: Option<Decision>,
    pub output: Option<Output>,
    pub cost_usd: Option<f64>,
    pub event_ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Thread {
    pub id: i64,
    pub client: ClientRef,
    pub title: String,
    pub state: &'static str,
    pub story: Option<String>,
    pub deadline: Option<String>,
    pub needs_you: Vec<String>,
    pub cost_usd: f64,
    pub last_event_id: i64,
    pub nodes: Vec<Node>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ThreadSummary {
    pub id: i64,
    pub client: ClientRef,
    pub title: String,
    pub state: &'static str,
    pub story: Option<String>,
    pub needs_you_count: usize,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoneCheck {
    pub passed: bool,
    pub output_tail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Inspector {
    pub node_id: String,
    pub events: Vec<Event>,
    pub cost_usd: f64,
    pub done_checks: Vec<DoneCheck>,
    pub next_after_id: Option<i64>,
}

/// Everything node building reads for one thread.
struct Ctx<'a> {
    top: &'a Task,
    subs: &'a [Task],
    events: Vec<Event>,
    questions: Vec<Question>,
    last: HashMap<i64, i64>,
    cost: HashMap<i64, f64>,
    now: i64,
    stall: i64,
}

impl Ctx<'_> {
    fn evs(&self, task_id: i64) -> Vec<&Event> {
        self.events.iter().filter(|e| e.task_id == Some(task_id)).collect()
    }

    fn tasks(&self) -> impl Iterator<Item = &Task> {
        std::iter::once(self.top).chain(self.subs.iter())
    }

    fn owner_question_open(&self, task_id: i64) -> bool {
        self.questions.iter().any(|q| q.task_id == task_id && q.status == QuestionStatus::Open && q.level == "owner")
    }
}

fn latest<'a>(evs: &[&'a Event], kind: &str) -> Option<&'a Event> {
    evs.iter().copied().filter(|e| e.kind == kind).max_by_key(|e| e.id)
}

fn max_id(evs: &[&Event], kind: &str) -> i64 {
    latest(evs, kind).map_or(0, |e| e.id)
}

fn node(id: String, lane: &'static str, kind: &'static str, status: &'static str, title: String, at: &str) -> Node {
    Node {
        id, lane, kind, status, title, agent: None, subtask_id: None, attempts: None, started_at: at.into(), ended_at: None,
        after: vec![], decision: None, output: None, cost_usd: None, event_ids: vec![],
    }
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or_default().trim().to_string()
}

fn tail(s: &str) -> String {
    let skip = s.chars().count().saturating_sub(TAIL_CHARS);
    s.chars().skip(skip).collect()
}

// ---- node status ----

/// Stopped or dropped and not resumed since. A teammate also fades when its Lead was dropped.
fn is_halted(ctx: &Ctx, t: &Task) -> bool {
    let halted = |evs: &[&Event], kinds: &[&str]| {
        kinds.iter().map(|k| max_id(evs, k)).max().unwrap_or(0) > max_id(evs, "resumed")
    };
    if halted(&ctx.evs(t.id), &["stopped", "dropped"]) {
        return true;
    }
    t.parent_id.is_some() && halted(&ctx.evs(ctx.top.id), &["dropped"])
}

/// A `land_failed` event newer than any `landed`, on a task that is not done.
fn pending_land<'a>(ctx: &'a Ctx, t: &Task) -> Option<&'a Event> {
    let evs = ctx.evs(t.id);
    let failed = latest(&evs, "land_failed")?;
    (failed.id > max_id(&evs, "landed") && t.status != TaskStatus::Done).then_some(failed)
}

/// A `loop_limit` event on a task that is still stuck and was not resumed.
fn pending_loop<'a>(ctx: &'a Ctx, t: &Task) -> Option<&'a Event> {
    let evs = ctx.evs(t.id);
    let hit = latest(&evs, "loop_limit")?;
    (t.status == TaskStatus::Stuck && hit.id > max_id(&evs, "resumed")).then_some(hit)
}

/// The top task's newest `forwarded` event while it is neither approved nor landed.
fn pending_review<'a>(ctx: &'a Ctx) -> Option<&'a Event> {
    let evs = ctx.evs(ctx.top.id);
    let fwd = latest(&evs, "forwarded")?;
    let approved = evs.iter().filter(|e| e.kind == "reviewed" && e.payload["verdict"] == "ok").map(|e| e.id).max().unwrap_or(0);
    (fwd.id > approved && fwd.id > max_id(&evs, "landed")).then_some(fwd)
}

fn last_is_failure(evs: &[&Event]) -> bool {
    evs.last().is_some_and(|e| e.kind == "agent_error" || e.kind == "loop_limit")
}

fn running_or_stalled(ctx: &Ctx, t: &Task) -> &'static str {
    let last = ctx.last.get(&t.id).copied().unwrap_or(0);
    if ctx.now - last > ctx.stall { "stalled" } else { "running" }
}

fn task_status(ctx: &Ctx, t: &Task) -> &'static str {
    if is_halted(ctx, t) {
        return "done";
    }
    let review = t.parent_id.is_none() && pending_review(ctx).is_some();
    if ctx.owner_question_open(t.id) || pending_land(ctx, t).is_some() || review {
        return "needs_you";
    }
    match t.status {
        TaskStatus::Done => "done",
        TaskStatus::Stuck => "failed",
        TaskStatus::WaitingAnswer => "waiting",
        TaskStatus::Split => "running",
        TaskStatus::New | TaskStatus::Open => "planned",
        TaskStatus::Clarifying | TaskStatus::InProgress if last_is_failure(&ctx.evs(t.id)) => "failed",
        TaskStatus::Clarifying | TaskStatus::InProgress => running_or_stalled(ctx, t),
    }
}

/// First match wins. `owner` is reserved for a future takeover and is never produced.
fn thread_state(statuses: &[&str]) -> &'static str {
    let order = ["needs_you", "failed", "stalled", "owner", "waiting", "running"];
    if let Some(s) = order.iter().find(|s| statuses.contains(s)) {
        return s;
    }
    let open: Vec<_> = statuses.iter().filter(|s| **s != "done").collect();
    if !open.is_empty() && open.iter().all(|s| **s == "planned") { "planned" } else { "done" }
}

// ---- nodes ----

fn ev_id(id: i64) -> String {
    format!("ev:{id}")
}

fn task_in_node(ctx: &Ctx) -> Node {
    let title = format!("You asked: {}", ctx.top.title);
    node(format!("task:{}", ctx.top.id), "in", "task_in", "done", title, &ctx.top.created_at)
}

fn is_answer_text(e: &Event) -> bool {
    e.payload["text"].as_str().is_some_and(|t| t.starts_with("Answer to your question"))
}

fn instruction_nodes(ctx: &Ctx) -> Vec<Node> {
    // An answer to a question is stored as an instruction too; the answer node already shows it.
    let said = |e: &&Event| e.kind == "instruction" && e.agent == "owner" && !is_answer_text(e);
    ctx.events
        .iter()
        .filter(said)
        .map(|e| {
            let text = format!("You said: {}", e.payload["text"].as_str().unwrap_or_default());
            let mut n = node(ev_id(e.id), "in", "instruction", "done", text, &e.created_at);
            n.event_ids = vec![e.id];
            n
        })
        .collect()
}

fn answer_nodes(ctx: &Ctx) -> Vec<Node> {
    let answered = ctx.questions.iter().filter(|q| q.status == QuestionStatus::Answered && q.level == "owner");
    let title = |q: &Question| format!("You answered: {}", q.answer.as_deref().unwrap_or_default());
    answered.map(|q| node(format!("q:{}", q.id), "in", "answer", "done", title(q), &q.created_at)).collect()
}

fn plan_node(ctx: &Ctx) -> Option<Node> {
    let n = ctx.subs.len();
    if n == 0 {
        return None;
    }
    let title = format!("Lead split it into {n} step{}", if n == 1 { "" } else { "s" });
    Some(node(format!("plan:{}", ctx.top.id), "work", "plan", "done", title, &ctx.top.created_at))
}

fn role_name(t: &Task) -> String {
    let name = t.agent.as_deref().unwrap_or(t.role.as_str());
    let mut chars = name.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

fn done_checks(evs: &[Event]) -> Vec<DoneCheck> {
    let checks = evs.iter().filter(|e| e.kind == "done_check");
    let to_check = |e: &Event| DoneCheck {
        passed: e.payload["passed"].as_bool().unwrap_or(false),
        output_tail: tail(e.payload["output_tail"].as_str().unwrap_or_default()),
    };
    checks.map(to_check).collect()
}

fn test_output(check: &DoneCheck) -> Output {
    let label = if check.passed { "Tests pass" } else { "Tests fail" };
    Output { kind: "test", label: label.into(), url: None, preview: None, output_tail: Some(check.output_tail.clone()) }
}

fn subtask_node(ctx: &Ctx, t: &Task) -> Node {
    let status = task_status(ctx, t);
    let title = format!("{}: {}", role_name(t), t.title);
    let mut n = node(format!("task:{}", t.id), "work", "subtask", status, title, &t.created_at);
    n.agent = t.agent.clone();
    n.subtask_id = Some(t.id);
    n.attempts = Some(Attempts { used: t.loops_used, max: t.max_loops });
    n.after = t.depends_on.iter().map(|d| format!("task:{d}")).collect();
    n.ended_at = (status == "done").then(|| t.updated_at.clone());
    n.cost_usd = Some(ctx.cost.get(&t.id).copied().unwrap_or(0.0));
    let own: Vec<Event> = ctx.evs(t.id).into_iter().filter(|e| e.kind == "done_check").cloned().collect();
    n.event_ids = own.iter().map(|e| e.id).collect();
    n.output = done_checks(&own).last().map(test_output);
    n
}

fn review_nodes(ctx: &Ctx) -> Vec<Node> {
    let sent_back = ctx.events.iter().filter(|e| e.kind == "sent_back");
    let title = |e: &Event| format!("Lead sent it back: {}", e.payload["remarks"].as_str().unwrap_or_default());
    let build = |e: &Event| {
        let mut n = node(ev_id(e.id), "work", "review", "done", title(e), &e.created_at);
        n.event_ids = vec![e.id];
        n
    };
    sent_back.map(build).collect()
}

// ---- decisions ----

fn decision(source: &'static str, ask: String, actions: &[&str]) -> Decision {
    Decision {
        source, question_id: None, ask, why: String::new(), ladder: vec![], risk: "unknown", reversible: None,
        if_approved: vec![], options: vec![], review: None, actions: actions.iter().map(|a| a.to_string()).collect(), undo_secs: 0,
    }
}

/// Risk, reversibility and after-approval steps saved in an `escalated` or `forwarded` payload.
fn meta_from(payload: &serde_json::Value) -> (&'static str, Option<bool>, Vec<String>) {
    let risk = match payload["risk"].as_str() {
        Some("low") => "low",
        Some("medium") => "medium",
        Some("high") => "high",
        _ => "unknown",
    };
    let steps = payload["if_approved"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(String::from)).collect();
    (risk, payload["reversible"].as_bool(), steps)
}

fn apply_meta(d: &mut Decision, payload: &serde_json::Value) {
    (d.risk, d.reversible, d.if_approved) = meta_from(payload);
}

fn decision_node(id: String, title: String, at: &str, d: Decision) -> Node {
    let mut n = node(id, "work", "decision", "needs_you", title, at);
    n.decision = Some(d);
    n
}

fn question_decisions(ctx: &Ctx) -> Vec<Node> {
    let open = ctx.questions.iter().filter(|q| q.status == QuestionStatus::Open && q.level == "owner");
    let build = |q: &Question| {
        let mut d = decision("question", q.text.clone(), &["answer"]);
        d.question_id = Some(q.id);
        d.why = q.ladder.last().map(|r| r.why.clone()).unwrap_or_default();
        d.ladder = q.ladder.clone();
        let to_owner = |e: &&Event| e.payload["question_id"] == q.id && e.payload["to"] == "owner";
        let esc = ctx.evs(q.task_id).into_iter().filter(|e| e.kind == "escalated").filter(to_owner).max_by_key(|e| e.id);
        if let Some(e) = esc {
            apply_meta(&mut d, &e.payload);
        }
        let mut n = decision_node(format!("q:{}", q.id), first_line(&q.text), &q.created_at, d);
        n.subtask_id = (q.task_id != ctx.top.id).then_some(q.task_id);
        n
    };
    open.map(build).collect()
}

fn loop_decisions(ctx: &Ctx) -> Vec<Node> {
    let hits = ctx.tasks().filter(|t| !is_halted(ctx, t)).filter_map(|t| Some((t, pending_loop(ctx, t)?)));
    let build = |(t, e): (&Task, &Event)| {
        let ask = format!("{} used {} of {} attempts", t.title, t.loops_used, t.max_loops);
        let mut n = decision_node(ev_id(e.id), format!("Hit its loop limit: {}", t.title), &e.created_at, decision("loop_limit", ask, &["raise_limit", "stop"]));
        n.subtask_id = (t.id != ctx.top.id).then_some(t.id);
        n
    };
    hits.map(build).collect()
}

fn land_decisions(ctx: &Ctx) -> Vec<Node> {
    let failed = ctx.tasks().filter(|t| !is_halted(ctx, t)).filter_map(|t| Some((t, pending_land(ctx, t)?)));
    let build = |(t, e): (&Task, &Event)| {
        let files = e.payload["files"].as_array().map_or(0, Vec::len);
        let ask = format!("{} could not be merged: conflicts in {files} files", t.title);
        let mut n = decision_node(ev_id(e.id), format!("Could not merge: {}", t.title), &e.created_at, decision("land", ask, &["retry", "stop"]));
        n.subtask_id = (t.id != ctx.top.id).then_some(t.id);
        n
    };
    failed.map(build).collect()
}

fn pr_url(e: &Event) -> Option<String> {
    e.payload["pr_url"].as_str().filter(|u| !u.is_empty()).map(String::from)
}

fn review_decision(ctx: &Ctx) -> Option<Node> {
    let fwd = pending_review(ctx)?;
    let url = pr_url(fwd);
    let kind = if url.is_some() { "pr" } else { "diff" };
    let ask = fwd.payload["remarks"].as_str().filter(|r| !r.is_empty()).unwrap_or("Review the finished work");
    let mut d = decision("review", ask.to_string(), &["approve", "send_back"]);
    d.review = Some(ReviewRef { kind, task_id: ctx.top.id, url });
    apply_meta(&mut d, &fwd.payload);
    Some(decision_node(format!("rv:{}", fwd.id), "Review the result".into(), &fwd.created_at, d))
}

// ---- outputs ----

fn out_node(e: &Event, kind: &'static str, title: &str, out: Output) -> Node {
    let mut n = node(ev_id(e.id), "out", kind, "done", title.into(), &e.created_at);
    n.output = Some(out);
    n.event_ids = vec![e.id];
    n
}

fn forward_node(ctx: &Ctx) -> Option<Node> {
    let evs = ctx.evs(ctx.top.id);
    let fwd = latest(&evs, "forwarded")?;
    let out = |kind, label: &str, url| Output { kind, label: label.into(), url, preview: None, output_tail: None };
    match pr_url(fwd) {
        Some(url) => Some(out_node(fwd, "pr", "Pull request ready", out("pr", "Pull request", Some(url)))),
        None if max_id(&evs, "landed") > fwd.id => Some(out_node(fwd, "diff", "Changes merged", out("diff", "Changes merged", None))),
        None => None,
    }
}

fn app_nodes(ctx: &Ctx) -> Vec<Node> {
    let started = ctx.events.iter().filter(|e| e.kind == "apps_started" && e.task_id == Some(ctx.top.id));
    started.filter_map(|e| app_node(e)).collect()
}

fn app_node(e: &Event) -> Option<Node> {
    let app = e.payload["apps"].as_array()?.first()?;
    let label = format!("App running on :{}", app["port"].as_u64().unwrap_or(0));
    let url = app["url"].as_str().filter(|u| !u.is_empty()).map(String::from);
    let out = Output { kind: "app", label: label.clone(), url, preview: None, output_tail: None };
    Some(out_node(e, "app", &label, out))
}

// ---- assembly ----

fn nodes(ctx: &Ctx) -> Vec<Node> {
    let mut v = vec![task_in_node(ctx)];
    v.extend(instruction_nodes(ctx));
    v.extend(answer_nodes(ctx));
    v.extend(plan_node(ctx));
    v.extend(ctx.subs.iter().map(|t| subtask_node(ctx, t)));
    v.extend(review_nodes(ctx));
    v.extend(question_decisions(ctx));
    v.extend(loop_decisions(ctx));
    v.extend(land_decisions(ctx));
    v.extend(review_decision(ctx));
    v.extend(forward_node(ctx));
    v.extend(app_nodes(ctx));
    v
}

fn client_ref(db: &Db, top: &Task) -> Result<ClientRef> {
    let name = db.get_client(top.client_id)?.map(|c| c.name).unwrap_or_default();
    Ok(ClientRef { id: top.client_id, name })
}

fn open_questions(db: &Db, ids: &[i64]) -> Result<Vec<Question>> {
    let mut all = vec![];
    for id in ids {
        all.extend(db.questions_for_task(*id)?);
    }
    Ok(all)
}

/// Decision node ids, highest risk first (stable within equal risk).
fn needs_you_ids(nodes: &[Node]) -> Vec<String> {
    let rank = |n: &Node| match n.decision.as_ref().map(|d| d.risk) {
        Some("high") => 0,
        Some("medium") => 1,
        Some("low") => 2,
        _ => 3,
    };
    let mut ds: Vec<&Node> = nodes.iter().filter(|n| n.kind == "decision").collect();
    ds.sort_by_key(|n| rank(n));
    ds.into_iter().map(|n| n.id.clone()).collect()
}

/// Builds the thread of one top-level task. `now` is epoch seconds; a running task with no
/// event for `stall` seconds is stalled.
pub fn build(db: &Db, top: &Task, now: i64, stall: i64) -> Result<Thread> {
    let subs = db.subtasks(top.id)?;
    let ids: Vec<i64> = std::iter::once(top.id).chain(subs.iter().map(|s| s.id)).collect();
    let events = db.events_for_tasks(&ids, &KINDS, 0, i64::MAX)?;
    let ctx = Ctx { top, subs: &subs, events, questions: open_questions(db, &ids)?, last: db.last_activity(&ids)?, cost: db.cost_by_task(&ids)?, now, stall };
    let nodes = nodes(&ctx);
    let mut statuses: Vec<&str> = nodes.iter().map(|n| n.status).collect();
    let all_planned = !subs.is_empty() && subs.iter().all(|s| task_status(&ctx, s) == "planned");
    if !(top.status == TaskStatus::Split && all_planned) {
        statuses.push(task_status(&ctx, top));
    }
    let needs_you = needs_you_ids(&nodes);
    Ok(Thread {
        id: top.id,
        client: client_ref(db, top)?,
        title: top.title.clone(),
        state: thread_state(&statuses),
        story: db.latest_story_line(&ids)?,
        deadline: None,
        needs_you,
        cost_usd: ctx.cost.values().sum(),
        last_event_id: db.max_event_id(&ids)?,
        nodes,
    })
}

/// The thread of a top-level task. None when the id is missing or is not top-level.
pub fn thread(db: &Db, id: i64, now: i64, stall: i64) -> Result<Option<Thread>> {
    match db.get_task(id)? {
        Some(t) if t.parent_id.is_none() => Ok(Some(build(db, &t, now, stall)?)),
        _ => Ok(None),
    }
}

/// Current epoch seconds.
pub fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Every thread, optionally narrowed to one client and one state. Threads that need you come
/// first, then the most recently active.
pub fn summaries(db: &Db, client_id: Option<i64>, state: Option<&str>, now: i64, stall: i64) -> Result<Vec<ThreadSummary>> {
    let mut built = vec![];
    for t in db.list_tasks()?.into_iter().filter(|t| t.parent_id.is_none()) {
        if client_id.is_some_and(|c| c != t.client_id) {
            continue;
        }
        let th = build(db, &t, now, stall)?;
        if state.is_none_or(|s| s == th.state) {
            built.push((th, t.updated_at));
        }
    }
    built.sort_by_key(|(th, _)| (th.state != "needs_you", std::cmp::Reverse(th.last_event_id)));
    Ok(built.into_iter().map(|(th, updated_at)| summary(th, updated_at)).collect())
}

fn summary(th: Thread, updated_at: String) -> ThreadSummary {
    let count = th.needs_you.len();
    ThreadSummary { id: th.id, client: th.client, title: th.title, state: th.state, story: th.story, needs_you_count: count, updated_at }
}

/// One node with its events, oldest first, one page at a time. A subtask also shows its tool calls.
pub fn inspect(db: &Db, th: &Thread, node_id: &str, after_id: i64) -> Result<Option<Inspector>> {
    let Some(node) = th.nodes.iter().find(|n| n.id == node_id) else { return Ok(None) };
    let own = db.events_by_ids(&node.event_ids)?;
    let done_checks = done_checks(&own);
    let mut events: Vec<Event> = own.into_iter().filter(|e| e.id > after_id).collect();
    if let Some(sid) = node.subtask_id.filter(|_| node.kind == "subtask") {
        events.extend(db.events_for_tasks(&[sid], &["tool_call"], after_id, PAGE as i64 + 1)?);
    }
    events.sort_by_key(|e| e.id);
    events.dedup_by_key(|e| e.id);
    let more = events.len() > PAGE;
    events.truncate(PAGE);
    let next_after_id = if more { events.last().map(|e| e.id) } else { None };
    let cost_usd = node.cost_usd.unwrap_or(0.0);
    Ok(Some(Inspector { node_id: node.id.clone(), events, cost_usd, done_checks, next_after_id }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::NewTask;
    use rui_core::{Role, SourceKind};
    use serde_json::json;
    use std::time::Instant;

    fn add(db: &Db, client: i64, parent: Option<i64>, status: TaskStatus) -> i64 {
        let role = if parent.is_some() { Role::Builder } else { Role::Lead };
        db.create_task(&NewTask {
            client_id: client, parent_id: parent, title: "work".into(), body: String::new(), status, done_command: None,
            max_loops: 5, role, source_kind: SourceKind::User, source_ref: None, agent: None, pod: vec![],
        })
        .unwrap()
        .id
    }

    /// A split top task with one subtask in `status`.
    fn setup(status: TaskStatus) -> (Db, i64, i64) {
        let db = Db::open_test();
        let c = db.create_client("acme", "/tmp/acme", None, &[]).unwrap();
        let top = add(&db, c.id, None, TaskStatus::Split);
        let sub = add(&db, c.id, Some(top), status);
        (db, top, sub)
    }

    fn get(db: &Db, top: i64, now: i64) -> Thread {
        thread(db, top, now, STALL_SECS).unwrap().unwrap()
    }

    fn real_now() -> i64 {
        now_secs()
    }

    fn sub_status(th: &Thread, sub: i64) -> &'static str {
        th.nodes.iter().find(|n| n.id == format!("task:{sub}")).unwrap().status
    }

    fn kinds(th: &Thread) -> Vec<&'static str> {
        th.nodes.iter().map(|n| n.kind).collect()
    }

    #[test]
    fn planned_node() {
        let (db, top, sub) = setup(TaskStatus::Open);
        assert_eq!(sub_status(&get(&db, top, real_now()), sub), "planned");
    }

    #[test]
    fn running_node_and_stalled_with_fake_clock() {
        let (db, top, sub) = setup(TaskStatus::InProgress);
        let last = db.last_activity(&[sub]).unwrap()[&sub];
        assert_eq!(sub_status(&get(&db, top, last + 30), sub), "running");
        assert_eq!(sub_status(&get(&db, top, last + 601), sub), "stalled");
    }

    #[test]
    fn recent_assistant_events_keep_a_task_running() {
        let (db, top, sub) = setup(TaskStatus::InProgress);
        db.add_event(Some(sub), "builder", "assistant", &json!({})).unwrap();
        let last = db.last_activity(&[sub]).unwrap()[&sub];
        assert_eq!(sub_status(&get(&db, top, last + 599), sub), "running");
        assert_eq!(sub_status(&get(&db, top, last + 700), sub), "stalled");
    }

    #[test]
    fn waiting_node_has_an_open_lead_question() {
        let (db, top, sub) = setup(TaskStatus::WaitingAnswer);
        db.create_ladder_question(sub, Role::Builder, "lead", &[], "Which one?").unwrap();
        assert_eq!(sub_status(&get(&db, top, real_now()), sub), "waiting");
    }

    #[test]
    fn open_owner_question_needs_you_with_a_decision() {
        let (db, top, sub) = setup(TaskStatus::WaitingAnswer);
        let q = db.create_ladder_question(sub, Role::Builder, "owner", &[], "Pick one\nmore").unwrap();
        let th = get(&db, top, real_now());
        assert_eq!(sub_status(&th, sub), "needs_you");
        assert_eq!(th.needs_you, vec![format!("q:{}", q.id)]);
        let d = th.nodes.iter().find_map(|n| n.decision.as_ref()).unwrap();
        assert_eq!((d.source, d.question_id, d.risk), ("question", Some(q.id), "unknown"));
    }

    fn owner_question(db: &Db, sub: i64, text: &str, meta: serde_json::Value) -> i64 {
        let q = db.create_ladder_question(sub, rui_core::Role::Builder, "owner", &[], text).unwrap();
        let mut p = json!({"question_id": q.id, "from": "rui", "to": "owner", "why": "w"});
        p.as_object_mut().unwrap().extend(meta.as_object().unwrap().clone());
        db.add_event(Some(sub), "scheduler", "escalated", &p).unwrap();
        q.id
    }

    #[test]
    fn question_decision_reads_saved_risk_and_missing_is_unknown() {
        let (db, top, sub) = setup(TaskStatus::WaitingAnswer);
        owner_question(&db, sub, "A", json!({"risk": "high", "reversible": false, "if_approved": ["one", "two"]}));
        let th = get(&db, top, real_now());
        let d = th.nodes.iter().find_map(|n| n.decision.as_ref()).unwrap();
        assert_eq!((d.risk, d.reversible, d.if_approved.clone()), ("high", Some(false), vec!["one".to_string(), "two".to_string()]));
        let (db, top, sub) = setup(TaskStatus::WaitingAnswer);
        owner_question(&db, sub, "A", json!({}));
        let d = get(&db, top, real_now()).nodes.iter().find_map(|n| n.decision.clone()).unwrap();
        assert_eq!((d.risk, d.reversible, d.if_approved.len()), ("unknown", None, 0));
    }

    #[test]
    fn review_decision_reads_forwarded_meta() {
        let (db, top, _) = setup(TaskStatus::Done);
        db.set_task_status(top, TaskStatus::Done).unwrap();
        db.add_event(Some(top), "lead", "forwarded", &json!({"remarks": "ok", "pr_url": null, "risk": "low", "reversible": true, "if_approved": ["merge"]})).unwrap();
        let d = get(&db, top, real_now()).nodes.iter().find_map(|n| n.decision.clone()).unwrap();
        assert_eq!((d.source, d.risk, d.reversible, d.if_approved), ("review", "low", Some(true), vec!["merge".to_string()]));
    }

    #[test]
    fn needs_you_lists_high_risk_first() {
        let (db, top, sub) = setup(TaskStatus::WaitingAnswer);
        let low = owner_question(&db, sub, "L", json!({"risk": "low"}));
        let unknown = owner_question(&db, sub, "U", json!({}));
        let high = owner_question(&db, sub, "H", json!({"risk": "high"}));
        let ids = get(&db, top, real_now()).needs_you;
        assert_eq!(ids, vec![format!("q:{high}"), format!("q:{low}"), format!("q:{unknown}")]);
    }

    #[test]
    fn forwarded_without_approval_needs_you() {
        let (db, top, _) = setup(TaskStatus::Done);
        db.set_task_status(top, TaskStatus::Done).unwrap();
        db.add_event(Some(top), "lead", "forwarded", &json!({"remarks": "ok", "pr_url": null})).unwrap();
        let th = get(&db, top, real_now());
        assert_eq!(th.state, "needs_you");
        assert_eq!(th.nodes.iter().find_map(|n| n.decision.as_ref()).unwrap().source, "review");
    }

    #[test]
    fn land_failed_needs_you() {
        let (db, top, sub) = setup(TaskStatus::InProgress);
        db.add_event(Some(sub), "scheduler", "land_failed", &json!({"files": ["a"]})).unwrap();
        let th = get(&db, top, real_now());
        assert_eq!(sub_status(&th, sub), "needs_you");
        assert_eq!(th.nodes.iter().find_map(|n| n.decision.as_ref()).unwrap().source, "land");
    }

    #[test]
    fn stuck_is_failed_and_loop_limit_adds_a_decision() {
        let (db, top, sub) = setup(TaskStatus::Stuck);
        assert_eq!(sub_status(&get(&db, top, real_now()), sub), "failed");
        db.add_event(Some(sub), "scheduler", "loop_limit", &json!({"loops_used": 5, "max_loops": 5})).unwrap();
        let th = get(&db, top, real_now());
        assert_eq!(th.nodes.iter().find_map(|n| n.decision.as_ref()).unwrap().source, "loop_limit");
        assert_eq!(th.state, "needs_you");
    }

    #[test]
    fn agent_error_on_a_running_task_is_failed() {
        let (db, top, sub) = setup(TaskStatus::InProgress);
        db.add_event(Some(sub), "builder", "agent_error", &json!({"error": "x"})).unwrap();
        assert_eq!(sub_status(&get(&db, top, real_now()), sub), "failed");
    }

    #[test]
    fn done_and_dropped_nodes_are_done() {
        let (db, top, sub) = setup(TaskStatus::Done);
        assert_eq!(sub_status(&get(&db, top, real_now()), sub), "done");
        let (db, top, sub) = setup(TaskStatus::Stuck);
        db.add_event(Some(sub), "owner", "stopped", &json!({"by": "owner"})).unwrap();
        assert_eq!(sub_status(&get(&db, top, real_now()), sub), "done");
    }

    #[test]
    fn thread_state_precedence() {
        let all = ["needs_you", "failed", "stalled", "waiting", "running", "planned", "done"];
        for i in 0..all.len() - 2 {
            assert_eq!(thread_state(&all[i..]), all[i]);
        }
        assert_eq!(thread_state(&["done", "planned"]), "planned");
        assert_eq!(thread_state(&["done", "planned", "done"]), "planned");
        assert_eq!(thread_state(&["done", "done"]), "done");
    }

    #[test]
    fn one_node_per_subtask_however_many_loops() {
        let (db, top, sub) = setup(TaskStatus::InProgress);
        for _ in 0..5 {
            db.increment_loops(sub).unwrap();
            db.add_event(Some(sub), "scheduler", "done_check", &json!({"passed": false, "output_tail": "boom"})).unwrap();
        }
        let th = get(&db, top, real_now());
        let subs: Vec<_> = th.nodes.iter().filter(|n| n.kind == "subtask").collect();
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].attempts.as_ref().unwrap().used, 5);
        assert_eq!(subs[0].event_ids.len(), 5);
        let out = subs[0].output.as_ref().unwrap();
        assert_eq!((out.kind, out.label.as_str()), ("test", "Tests fail"));
        assert_eq!(out.output_tail.as_deref(), Some("boom"));
    }

    #[test]
    fn thousands_of_assistant_events_make_no_nodes_and_stay_fast() {
        let (db, top, sub) = setup(TaskStatus::InProgress);
        let before = get(&db, top, real_now()).nodes.len();
        for _ in 0..5000 {
            db.add_event(Some(sub), "builder", "assistant", &json!({"text": "x"})).unwrap();
        }
        let start = Instant::now();
        let th = get(&db, top, real_now());
        assert!(start.elapsed().as_secs_f64() < 1.0);
        assert_eq!(th.nodes.len(), before);
        assert!(!kinds(&th).contains(&"assistant"));
    }

    #[test]
    fn task_deps_become_after() {
        let (db, top, first) = setup(TaskStatus::Open);
        let c = db.get_task(top).unwrap().unwrap().client_id;
        let second = add(&db, c, Some(top), TaskStatus::Open);
        db.add_task_dep(second, first).unwrap();
        let th = get(&db, top, real_now());
        let n = th.nodes.iter().find(|n| n.id == format!("task:{second}")).unwrap();
        assert_eq!(n.after, vec![format!("task:{first}")]);
    }

    #[test]
    fn forwarded_with_url_is_a_pr_and_without_is_a_diff_once_landed() {
        let (db, top, _) = setup(TaskStatus::Done);
        db.add_event(Some(top), "lead", "forwarded", &json!({"pr_url": "https://x.test/pr/1"})).unwrap();
        let th = get(&db, top, real_now());
        let pr = th.nodes.iter().find(|n| n.kind == "pr").unwrap();
        assert_eq!(pr.output.as_ref().unwrap().url.as_deref(), Some("https://x.test/pr/1"));
        let (db, top, _) = setup(TaskStatus::Done);
        db.add_event(Some(top), "lead", "forwarded", &json!({"pr_url": null})).unwrap();
        assert!(!kinds(&get(&db, top, real_now())).contains(&"diff"));
        db.add_event(Some(top), "owner", "landed", &json!({"branch": "b"})).unwrap();
        let th = get(&db, top, real_now());
        assert!(kinds(&th).contains(&"diff"));
        assert!(th.nodes.iter().all(|n| n.kind != "decision"));
    }

    #[test]
    fn instruction_answer_and_app_nodes() {
        let (db, top, sub) = setup(TaskStatus::InProgress);
        db.add_event(Some(top), "owner", "instruction", &json!({"text": "do more"})).unwrap();
        let q = db.create_ladder_question(sub, Role::Builder, "owner", &[], "Pick").unwrap();
        db.answer_question(q.id, "A", None).unwrap();
        let apps = json!({"apps": [{"port": 3000, "url": "http://localhost:3000"}]});
        db.add_event(Some(top), "scheduler", "apps_started", &apps).unwrap();
        let th = get(&db, top, real_now());
        let k = kinds(&th);
        assert!(["task_in", "instruction", "answer", "plan", "subtask", "app"].iter().all(|x| k.contains(x)));
        let app = th.nodes.iter().find(|n| n.kind == "app").unwrap();
        assert_eq!(app.output.as_ref().unwrap().label, "App running on :3000");
    }

    #[test]
    fn cost_sums_result_events_and_story_falls_back_to_log_decision() {
        let (db, top, sub) = setup(TaskStatus::InProgress);
        db.add_event(Some(top), "lead", "result", &json!({"total_cost_usd": 0.5})).unwrap();
        db.add_event(Some(sub), "builder", "result", &json!({"total_cost_usd": 0.25})).unwrap();
        db.add_event(Some(sub), "builder", "result", &json!({})).unwrap();
        let log = json!({"tool": "log_decision", "args": {"line": "chose sqlite"}});
        db.add_event(Some(top), "lead", "tool_call", &log).unwrap();
        let th = get(&db, top, real_now());
        assert!((th.cost_usd - 0.75).abs() < 1e-9);
        let node = th.nodes.iter().find(|n| n.subtask_id == Some(sub)).unwrap();
        assert!((node.cost_usd.unwrap() - 0.25).abs() < 1e-9);
        assert_eq!(th.story.as_deref(), Some("chose sqlite"));
        db.add_event(Some(top), "lead", "story", &json!({"line": "the story"})).unwrap();
        assert_eq!(get(&db, top, real_now()).story.as_deref(), Some("the story"));
        assert!(th.last_event_id > 0);
    }

    #[test]
    fn missing_or_subtask_id_has_no_thread() {
        let (db, _, sub) = setup(TaskStatus::Open);
        assert!(thread(&db, sub, real_now(), STALL_SECS).unwrap().is_none());
        assert!(thread(&db, 999, real_now(), STALL_SECS).unwrap().is_none());
    }

    #[test]
    fn fresh_split_is_not_started() {
        let (db, top, _sub) = setup(TaskStatus::Open);
        assert_eq!(get(&db, top, real_now()).state, "planned");
    }

    #[test]
    fn answer_text_instruction_is_not_a_second_node() {
        let (db, top, _sub) = setup(TaskStatus::Open);
        let say = |t: &str| db.add_event(Some(top), "owner", "instruction", &json!({"text": t})).unwrap();
        say("Answer to your question \"x\": yes");
        say("use v2");
        let th = get(&db, top, real_now());
        let titles: Vec<_> = th.nodes.iter().filter(|n| n.kind == "instruction").map(|n| n.title.clone()).collect();
        assert_eq!(titles, vec!["You said: use v2".to_string()]);
    }
}

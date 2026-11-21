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
const KINDS: [&str; 25] = [
    "instruction", "sent_back", "forwarded", "landed", "land_failed", "loop_limit", "apps_started", "done_check", "stopped",
    "dropped", "agent_error", "reviewed", "resumed", "escalated", "checkpoint_set", "checkpoint_cleared", "taken_over",
    "handed_back", "pr_merged", "pr_closed", "deploy_requested", "deploy_started", "deploy_finished", "team_asked", "team_reply",
];
const CHECKPOINT_REASON: &str = "Waiting for you to release this step";
const TAKEOVER_REASON: &str = "You're doing this step";

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
    #[serde(skip_serializing_if = "String::is_empty")]
    pub details: String,
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
    /// Why the node is in its status, when the owner holds the step.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Model picked for a subtask's runs (latest `run_settings`); None means the role or CLI default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Effort picked for a subtask's runs; None means the role or CLI default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
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
    pub hidden: bool,
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
    /// What each planning meeting's turns cost, by `team_asked` event id.
    meeting_cost: HashMap<i64, f64>,
    /// `run_settings` events of the subtasks, oldest first. Kept apart from `events`, so status logic never sees them.
    run_settings: Vec<Event>,
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
        after: vec![], decision: None, output: None, cost_usd: None, event_ids: vec![], reason: None, model: None,
        effort: None,
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

/// The top task's newest `forwarded` event while the owner hasn't answered it (approved or sent
/// back for changes) and it hasn't landed.
fn pending_review<'a>(ctx: &'a Ctx) -> Option<&'a Event> {
    let evs = ctx.evs(ctx.top.id);
    let fwd = latest(&evs, "forwarded")?;
    let answered = max_id(&evs, "reviewed");
    let settled = max_id(&evs, "landed").max(max_id(&evs, "pr_merged")).max(max_id(&evs, "pr_closed"));
    (fwd.id > answered && fwd.id > settled).then_some(fwd)
}

fn last_is_failure(evs: &[&Event]) -> bool {
    evs.last().is_some_and(|e| e.kind == "agent_error" || e.kind == "loop_limit")
}

fn running_or_stalled(ctx: &Ctx, t: &Task) -> &'static str {
    let last = ctx.last.get(&t.id).copied().unwrap_or(0);
    if ctx.now - last > ctx.stall { "stalled" } else { "running" }
}

/// "owner" for a step the owner took over, "waiting" for one held at a checkpoint.
fn held_status(ctx: &Ctx, t: &Task) -> Option<&'static str> {
    match (crate::scheduler::hold_of(ctx.evs(t.id)), t.status) {
        (Some("takeover"), s) if s != TaskStatus::Done => Some("owner"),
        (Some("checkpoint"), TaskStatus::New | TaskStatus::Open) => Some("waiting"),
        _ => None,
    }
}

fn task_status(ctx: &Ctx, t: &Task) -> &'static str {
    if is_halted(ctx, t) {
        return "done";
    }
    let review = t.parent_id.is_none() && pending_review(ctx).is_some();
    if ctx.owner_question_open(t.id) || pending_land(ctx, t).is_some() || review {
        return "needs_you";
    }
    if let Some(held) = held_status(ctx, t) {
        return held;
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

/// First match wins. `owner` means the owner took over a step ("You're doing a step").
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

/// An owner's answer forwarded as an instruction: tagged with `from_question`, or (older events) by its text.
fn is_answer_text(e: &Event) -> bool {
    if e.payload.get("from_question").is_some_and(|v| !v.is_null()) {
        return true;
    }
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
    // Questions closed by dropping the task weren't answered by the owner: leave them off the map.
    let dropped = |q: &&Question| q.answer.as_deref() == Some(crate::control::DROPPED_ANSWER);
    let answered = ctx.questions.iter().filter(|q| q.status == QuestionStatus::Answered && q.level == "owner").filter(|q| !dropped(q));
    let title = |q: &Question| format!("You answered: {}", q.answer.as_deref().unwrap_or_default());
    answered.map(|q| node(format!("q:{}", q.id), "in", "answer", "done", title(q), &q.created_at)).collect()
}

fn plan_node(ctx: &Ctx) -> Option<Node> {
    let n = ctx.subs.len();
    if n == 0 {
        return None;
    }
    let steps = if n == 1 { "step" } else { "steps" };
    let title = format!("Lead split it into {n} {steps}");
    let mut node = node(format!("plan:{}", ctx.top.id), "work", "plan", "done", title, &ctx.top.created_at);
    let list: Vec<String> = ctx.subs.iter().enumerate().map(|(i, t)| format!("{}. {}", i + 1, t.title)).collect();
    node.output = Some(Output { kind: "plan", label: format!("{n} {steps}"), url: None, preview: Some(list.join("\n")), output_tail: None });
    Some(node)
}

fn role_name(t: &Task) -> String {
    capitalize(t.agent.as_deref().unwrap_or(t.role.as_str()))
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
    n.reason = hold_reason(ctx, t, status).map(String::from);
    if let Some(e) = ctx.run_settings.iter().rev().find(|e| e.task_id == Some(t.id)) {
        let s = |k: &str| e.payload[k].as_str().map(String::from);
        (n.model, n.effort) = (s("model"), s("effort"));
    }
    n
}

/// The reason shown on a subtask the owner holds.
fn hold_reason(ctx: &Ctx, t: &Task, status: &str) -> Option<&'static str> {
    match (held_status(ctx, t)?, status) {
        ("waiting", "waiting") => Some(CHECKPOINT_REASON),
        ("owner", "owner") => Some(TAKEOVER_REASON),
        _ => None,
    }
}

/// One node per takeover; the newest is "owner" while the owner still has the step.
fn takeover_nodes(ctx: &Ctx) -> Vec<Node> {
    let taken = ctx.events.iter().filter(|e| e.kind == "taken_over");
    let build = |e: &Event| {
        let t = ctx.subs.iter().find(|s| Some(s.id) == e.task_id)?;
        let evs = ctx.evs(t.id);
        let held = held_status(ctx, t) == Some("owner") && latest(&evs, "taken_over").is_some_and(|l| l.id == e.id);
        let status = if held { "owner" } else { "done" };
        let mut n = node(ev_id(e.id), "work", "takeover", status, format!("You took over: {}", t.title), &e.created_at);
        n.subtask_id = Some(t.id);
        n.event_ids = vec![e.id];
        n.reason = held.then(|| TAKEOVER_REASON.to_string());
        Some(n)
    };
    taken.filter_map(build).collect()
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

/// One "meeting" node per planning meeting: running until every role replied, then done.
fn meeting_nodes(ctx: &Ctx) -> Vec<Node> {
    let asked = ctx.events.iter().filter(|e| e.kind == "team_asked" && e.task_id == Some(ctx.top.id));
    asked.map(|req| meeting_node(ctx, req)).collect()
}

fn meeting_node(ctx: &Ctx, req: &Event) -> Node {
    let mine = |e: &&Event| e.kind == "team_reply" && e.payload["roles_request_event_id"] == req.id;
    let replies: Vec<Event> = ctx.events.iter().filter(mine).cloned().collect();
    let roles = crate::team::roles_of(req);
    let open = !crate::team::waiting_roles(req, &replies).is_empty();
    let names: Vec<String> = roles.iter().map(|r| capitalize(r)).collect();
    let title = format!("Planning meeting: {}", names.join(", "));
    let mut n = node(ev_id(req.id), "work", "meeting", if open { "running" } else { "done" }, title, &req.created_at);
    n.ended_at = (!open).then(|| replies.last().map(|r| r.created_at.clone())).flatten();
    n.event_ids = std::iter::once(req.id).chain(replies.iter().map(|r| r.id)).collect();
    n.cost_usd = Some(ctx.meeting_cost.get(&req.id).copied().unwrap_or(0.0));
    let label = format!("{} of {} answered", replies.len(), roles.len());
    let lines: Vec<String> = replies.iter().map(|r| meeting_line(&r.payload)).collect();
    n.output = Some(Output { kind: "meeting", label, url: None, preview: Some(lines.join("\n")), output_tail: None });
    n
}

/// "Tester (S): approach. Proof: cargo test"
fn meeting_line(p: &serde_json::Value) -> String {
    let s = |k: &str| p[k].as_str().unwrap_or_default().to_string();
    format!("{} ({}): {} Proof: {}", capitalize(&s("role")), s("size"), s("approach"), s("proof"))
}

fn capitalize(name: &str) -> String {
    let mut chars = name.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// What each meeting's planning turns cost, read from their `result` events on the top task.
fn meeting_costs(db: &Db, top: i64, events: &[Event]) -> Result<HashMap<i64, f64>> {
    let mut out = HashMap::new();
    if !events.iter().any(|e| e.kind == "team_asked") {
        return Ok(out);
    }
    for e in db.events_for_tasks(&[top], &["result"], 0, i64::MAX)? {
        let Some(id) = e.agent.strip_prefix("team:").and_then(|r| r.split(':').next()?.parse::<i64>().ok()) else { continue };
        *out.entry(id).or_insert(0.0) += e.payload["total_cost_usd"].as_f64().unwrap_or(0.0);
    }
    Ok(out)
}

// ---- decisions ----

fn decision(source: &'static str, ask: String, actions: &[&str]) -> Decision {
    Decision {
        source, question_id: None, ask, details: String::new(), why: String::new(), ladder: vec![], risk: "unknown", reversible: None,
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
    if let Some(short) = short_question(payload) {
        d.details = std::mem::replace(&mut d.ask, short);
    }
    if let Some(opts) = payload["options"].as_array() {
        d.options = opts.iter().filter_map(|v| v.as_str().map(String::from)).collect();
    }
}

/// The newest `escalated` event that sent question `q` to the owner, among `events`.
pub fn owner_escalation<'a>(events: impl IntoIterator<Item = &'a Event>, q: &Question) -> Option<&'a Event> {
    let to_owner = |e: &&Event| e.kind == "escalated" && e.payload["question_id"] == q.id && e.payload["to"] == "owner";
    events.into_iter().filter(to_owner).max_by_key(|e| e.id)
}

/// The short question the escalating agent wrote for the owner, if any.
pub fn short_question(payload: &serde_json::Value) -> Option<String> {
    let s = payload["question"].as_str()?.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// The short owner question for `q`, read from its escalation event.
pub fn owner_short(db: &Db, q: &Question) -> Option<String> {
    let events = db.list_events(Some(q.task_id), 500).ok()?;
    short_question(&owner_escalation(&events, q)?.payload)
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
        d.undo_secs = crate::scheduler::UNDO_SECS;
        d.why = q.ladder.last().map(|r| r.why.clone()).unwrap_or_default();
        d.ladder = q.ladder.clone();
        let esc = owner_escalation(ctx.evs(q.task_id), q);
        if let Some(e) = esc {
            apply_meta(&mut d, &e.payload);
        }
        let title = esc.and_then(|e| short_question(&e.payload)).unwrap_or_else(|| first_line(&q.text));
        let mut n = decision_node(format!("q:{}", q.id), title, &q.created_at, d);
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

/// A merged PR is a done "pr" output; one closed without a merge is a failed one.
fn pr_outcome_nodes(ctx: &Ctx) -> Vec<Node> {
    let evs = ctx.evs(ctx.top.id);
    let outcome = |e: &&Event| e.kind == "pr_merged" || e.kind == "pr_closed";
    evs.into_iter().filter(outcome).map(pr_outcome_node).collect()
}

fn pr_outcome_node(e: &Event) -> Node {
    let merged = e.kind == "pr_merged";
    let label = if merged { "PR merged" } else { "PR closed without merge" };
    let url = e.payload["url"].as_str().filter(|u| !u.is_empty()).map(String::from);
    let out = Output { kind: "pr", label: label.into(), url, preview: None, output_tail: None };
    let mut n = out_node(e, "pr", label, out);
    n.status = if merged { "done" } else { "failed" };
    n
}

fn app_nodes(ctx: &Ctx) -> Vec<Node> {
    let started = ctx.events.iter().filter(|e| e.kind == "apps_started" && e.task_id == Some(ctx.top.id));
    started.filter_map(app_node).collect()
}

fn app_node(e: &Event) -> Option<Node> {
    let app = e.payload["apps"].as_array()?.first()?;
    let label = format!("App running on :{}", app["port"].as_u64().unwrap_or(0));
    let url = app["url"].as_str().filter(|u| !u.is_empty()).map(String::from);
    let out = Output { kind: "app", label: label.clone(), url, preview: None, output_tail: None };
    Some(out_node(e, "app", &label, out))
}

/// One "deploy" output per deploy the owner started: running, done or failed.
fn deploy_nodes(ctx: &Ctx) -> Vec<Node> {
    let evs = ctx.evs(ctx.top.id);
    let reqs = evs.iter().filter(|e| e.kind == "deploy_requested");
    reqs.filter_map(|r| deploy_node(r, &evs)).collect()
}

fn deploy_node(req: &Event, evs: &[&Event]) -> Option<Node> {
    let mine = |e: &&&Event| e.payload["event_id"].as_i64() == Some(req.id);
    let started = evs.iter().filter(|e| e.kind == "deploy_started").filter(mine).max_by_key(|e| e.id)?;
    let finished = evs.iter().filter(|e| e.kind == "deploy_finished" && e.id > started.id).find(mine);
    let status = match finished.map(|f| f.payload["ok"].as_bool() == Some(true)) {
        None => "running",
        Some(true) => "done",
        Some(false) => "failed",
    };
    let label = format!("Deploy to {}", req.payload["environment"].as_str().unwrap_or("?"));
    let output_tail = finished.and_then(|f| f.payload["output_tail"].as_str()).map(String::from);
    let mut n = out_node(started, "deploy", &label, Output { kind: "deploy", label: label.clone(), url: None, preview: None, output_tail });
    n.status = status;
    n.ended_at = finished.map(|f| f.created_at.clone());
    n.event_ids = std::iter::once(req.id).chain([started.id]).chain(finished.map(|f| f.id)).collect();
    Some(n)
}

// ---- assembly ----

fn nodes(ctx: &Ctx) -> Vec<Node> {
    let mut v = vec![task_in_node(ctx)];
    v.extend(instruction_nodes(ctx));
    v.extend(answer_nodes(ctx));
    v.extend(meeting_nodes(ctx));
    v.extend(plan_node(ctx));
    v.extend(ctx.subs.iter().map(|t| subtask_node(ctx, t)));
    v.extend(takeover_nodes(ctx));
    v.extend(review_nodes(ctx));
    v.extend(question_decisions(ctx));
    v.extend(loop_decisions(ctx));
    v.extend(land_decisions(ctx));
    v.extend(review_decision(ctx));
    v.extend(forward_node(ctx));
    v.extend(pr_outcome_nodes(ctx));
    v.extend(app_nodes(ctx));
    v.extend(deploy_nodes(ctx));
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
    let meeting_cost = meeting_costs(db, top.id, &events)?;
    let (questions, last, cost) = (open_questions(db, &ids)?, db.last_activity(&ids)?, db.cost_by_task(&ids)?);
    let run_settings = db.events_for_tasks(&ids, &[crate::run_settings::EVENT], 0, i64::MAX)?;
    let ctx = Ctx { top, subs: &subs, events, questions, last, cost, meeting_cost, run_settings, now, stall };
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

/// Done threads idle longer than this are left out of the thread list unless `all` is set.
pub const DONE_KEEP_SECS: i64 = 7 * 24 * 3600;

/// Every thread, optionally narrowed to one client and one state. Threads that need you come
/// first, then the most recently active. Old done threads are skipped unless `all` is set.
/// Which threads a listing shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Show {
    /// Visible threads, without old done ones.
    Recent,
    /// Every thread, hidden ones too.
    All,
    /// Only hidden threads that were not removed.
    Hidden,
}

/// True when the thread belongs in a listing of `show`, by its hidden state alone.
fn shown(h: &crate::hide::Hidden, id: i64, show: Show) -> bool {
    match show {
        Show::Recent => !h.is_hidden(id),
        Show::All => true,
        Show::Hidden => h.is_hidden(id) && !h.is_removed(id),
    }
}

pub fn summaries(db: &Db, client_id: Option<i64>, state: Option<&str>, show: Show, now: i64, stall: i64) -> Result<Vec<ThreadSummary>> {
    let hidden = crate::hide::hidden(db)?;
    let mut built = vec![];
    for t in db.list_tasks()?.into_iter().filter(|t| t.parent_id.is_none()) {
        if client_id.is_some_and(|c| c != t.client_id) || !shown(&hidden, t.id, show) {
            continue;
        }
        let th = build(db, &t, now, stall)?;
        if state.is_some_and(|s| s != th.state) || (show == Show::Recent && is_old_done(db, &th, now)?) {
            continue;
        }
        built.push((th, t.updated_at));
    }
    built.sort_by_key(|(th, _)| (th.state != "needs_you", std::cmp::Reverse(th.last_event_id)));
    Ok(built.into_iter().map(|(th, updated_at)| summary(th, updated_at, &hidden)).collect())
}

/// True when the thread is done and nothing in it has happened for DONE_KEEP_SECS.
fn is_old_done(db: &Db, th: &Thread, now: i64) -> Result<bool> {
    if th.state != "done" {
        return Ok(false);
    }
    let ids: Vec<i64> = std::iter::once(th.id).chain(db.subtasks(th.id)?.iter().map(|s| s.id)).collect();
    let last = db.last_activity(&ids)?.into_values().max().unwrap_or(now);
    Ok(now - last > DONE_KEEP_SECS)
}

fn summary(th: Thread, updated_at: String, h: &crate::hide::Hidden) -> ThreadSummary {
    let (count, hidden) = (th.needs_you.len(), h.is_hidden(th.id));
    ThreadSummary { id: th.id, client: th.client, title: th.title, state: th.state, story: th.story, needs_you_count: count, updated_at, hidden }
}

/// One node with its events, oldest first, one page at a time. A subtask also shows its tool calls.
pub fn inspect(db: &Db, th: &Thread, node_id: &str, after_id: i64) -> Result<Option<Inspector>> {
    let Some(node) = th.nodes.iter().find(|n| n.id == node_id) else { return Ok(None) };
    if node.kind == "plan" {
        return plan_inspect(db, th.id, node, after_id).map(Some);
    }
    let own = db.events_by_ids(&node.event_ids)?;
    let done_checks = done_checks(&own);
    let mut events: Vec<Event> = own.into_iter().filter(|e| e.id > after_id).collect();
    if let Some(sid) = node.subtask_id.filter(|_| node.kind == "subtask") {
        // The member's own turns and notes, so a running step shows what it is doing.
        let kinds = ["assistant", "tool_call", "progress", "result"];
        events.extend(db.events_for_tasks(&[sid], &kinds, after_id, PAGE as i64 + 1)?);
    }
    let cost_usd = node.cost_usd.unwrap_or(0.0);
    Ok(Some(page(node, events, cost_usd, done_checks)))
}

/// One page of a node's events, oldest first, with the id to continue after.
fn page(node: &Node, mut events: Vec<Event>, cost_usd: f64, done_checks: Vec<DoneCheck>) -> Inspector {
    events.sort_by_key(|e| e.id);
    events.dedup_by_key(|e| e.id);
    let more = events.len() > PAGE;
    events.truncate(PAGE);
    let next_after_id = if more { events.last().map(|e| e.id) } else { None };
    Inspector { node_id: node.id.clone(), events, cost_usd, done_checks, next_after_id }
}

/// The plan node's story: the Account Lead's turns on the top task up to the one that
/// created the last step, and what those turns cost.
fn plan_inspect(db: &Db, top: i64, node: &Node, after_id: i64) -> Result<Inspector> {
    let last_step = db.subtasks(top)?.into_iter().map(|t| t.created_at).max().unwrap_or_default();
    let kinds = ["assistant", "tool_call", "result"];
    let lead: Vec<Event> = db.events_for_tasks(&[top], &kinds, 0, i64::MAX)?.into_iter().filter(|e| e.agent == "lead").collect();
    let end = lead.iter().find(|e| e.kind == "result" && e.created_at >= last_step).map_or(i64::MAX, |e| e.id);
    let turn: Vec<Event> = lead.into_iter().filter(|e| e.id <= end).collect();
    let cost = turn.iter().filter(|e| e.kind == "result").filter_map(|e| e.payload["total_cost_usd"].as_f64()).sum();
    let events = turn.into_iter().filter(|e| e.id > after_id).collect();
    Ok(page(node, events, cost, vec![]))
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
    fn checkpoint_waits_and_takeover_is_owner() {
        let (db, top, sub) = setup(TaskStatus::Open);
        db.add_event(Some(sub), "owner", "checkpoint_set", &json!({})).unwrap();
        let th = get(&db, top, real_now());
        let n = th.nodes.iter().find(|n| n.id == format!("task:{sub}")).unwrap();
        assert_eq!((n.status, n.reason.as_deref()), ("waiting", Some("Waiting for you to release this step")));
        assert_eq!(th.state, "waiting");
        db.add_event(Some(sub), "owner", "taken_over", &json!({"worktree": null, "branch": "rui/2"})).unwrap();
        let th = get(&db, top, real_now());
        assert_eq!((sub_status(&th, sub), th.state), ("owner", "owner"));
        assert!(th.nodes.iter().any(|n| n.kind == "takeover" && n.status == "owner"));
        db.add_event(Some(sub), "owner", "handed_back", &json!({})).unwrap();
        let th = get(&db, top, real_now());
        assert_eq!(sub_status(&th, sub), "planned");
        assert!(th.nodes.iter().any(|n| n.kind == "takeover" && n.status == "done"));
    }

    #[test]
    fn meeting_node_runs_until_every_role_replied() {
        let (db, top, _) = setup(TaskStatus::Open);
        let req = db.add_event(Some(top), "lead", "team_asked", &json!({"roles": ["designer", "tester"], "question": "How?"})).unwrap();
        let reply = |role: &str| json!({"roles_request_event_id": req, "role": role, "approach": "do it", "risks": [], "needs": [], "size": "S", "proof": "cargo test"});
        db.add_event(Some(top), &crate::team::agent_name(req, "designer"), "team_reply", &reply("designer")).unwrap();
        db.add_event(Some(top), &crate::team::agent_name(req, "designer"), "result", &json!({"total_cost_usd": 0.25})).unwrap();
        let meeting = |th: &Thread| th.nodes.iter().find(|n| n.kind == "meeting").cloned().unwrap();
        let n = meeting(&get(&db, top, real_now()));
        assert_eq!((n.title.as_str(), n.status, n.lane), ("Planning meeting: Designer, Tester", "running", "work"));
        assert_eq!((n.cost_usd, n.event_ids.len()), (Some(0.25), 2));
        db.add_event(Some(top), &crate::team::agent_name(req, "tester"), "team_reply", &reply("tester")).unwrap();
        let n = meeting(&get(&db, top, real_now()));
        assert_eq!(n.status, "done");
        let out = n.output.unwrap();
        assert_eq!(out.label, "2 of 2 answered");
        assert!(out.preview.unwrap().contains("Tester (S): do it Proof: cargo test"));
    }

    #[test]
    fn subtask_nodes_show_the_latest_run_settings() {
        let (db, top, sub) = setup(TaskStatus::Open);
        let node = |th: &Thread| th.nodes.iter().find(|n| n.id == format!("task:{sub}")).cloned().unwrap();
        let n = node(&get(&db, top, real_now()));
        assert_eq!((n.model, n.effort), (None, None));
        let s = crate::run_settings::check(Some("haiku"), Some("low")).unwrap();
        crate::run_settings::record(&db, sub, &s, "lead", None).unwrap();
        let s = crate::run_settings::check(Some("opus"), Some("high")).unwrap();
        crate::run_settings::record(&db, sub, &s, "lead", Some("tricky")).unwrap();
        let n = node(&get(&db, top, real_now()));
        assert_eq!((n.model.as_deref(), n.effort.as_deref()), (Some("opus"), Some("high")));
        assert_eq!(sub_status(&get(&db, top, real_now()), sub), "planned");
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
    fn short_question_and_options_split_off_the_details() {
        let (db, top, sub) = setup(TaskStatus::WaitingAnswer);
        owner_question(&db, sub, "Long original text\nmore", json!({"question": "Use sqlite?", "options": ["Yes", "No"]}));
        let th = get(&db, top, real_now());
        let n = th.nodes.iter().find(|n| n.decision.is_some()).unwrap();
        let d = n.decision.as_ref().unwrap();
        assert_eq!((d.ask.as_str(), d.details.as_str()), ("Use sqlite?", "Long original text\nmore"));
        assert_eq!((d.options.clone(), n.title.as_str()), (vec!["Yes".to_string(), "No".to_string()], "Use sqlite?"));
        let (db, top, sub) = setup(TaskStatus::WaitingAnswer);
        owner_question(&db, sub, "Plain\nmore", json!({}));
        let n = get(&db, top, real_now()).nodes.into_iter().find(|n| n.decision.is_some()).unwrap();
        let d = n.decision.unwrap();
        assert_eq!((d.ask.as_str(), d.details.as_str(), d.options.len(), n.title.as_str()), ("Plain\nmore", "", 0, "Plain"));
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
    fn sending_work_back_answers_the_review_until_new_work_comes() {
        let (db, top, _) = setup(TaskStatus::Done);
        db.set_task_status(top, TaskStatus::Done).unwrap();
        db.add_event(Some(top), "lead", "forwarded", &json!({"remarks": "first"})).unwrap();
        db.add_event(Some(top), "owner", "reviewed", &json!({"verdict": "changes"})).unwrap();
        let reviews = |db: &Db| get(db, top, real_now()).nodes.iter().filter(|n| n.decision.as_ref().is_some_and(|d| d.source == "review")).count();
        assert_eq!(reviews(&db), 0, "sent back: nothing waits on the owner");
        db.add_event(Some(top), "lead", "forwarded", &json!({"remarks": "second"})).unwrap();
        assert_eq!(reviews(&db), 1, "new work asks again");
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
    fn pr_merged_and_closed_are_out_nodes() {
        let (db, top, _) = setup(TaskStatus::Done);
        db.add_event(Some(top), "lead", "forwarded", &json!({"remarks": "ok", "pr_url": "https://x.test/pr/1"})).unwrap();
        db.add_event(Some(top), "github", "pr_merged", &json!({"url": "https://x.test/pr/1", "merged_at": "t"})).unwrap();
        let th = get(&db, top, real_now());
        let merged = th.nodes.iter().find(|n| n.title == "PR merged").unwrap();
        assert_eq!((merged.lane, merged.kind, merged.status), ("out", "pr", "done"));
        assert!(th.nodes.iter().all(|n| n.kind != "decision"), "a merged PR needs no review");
        let (db, top, _) = setup(TaskStatus::Stuck);
        db.add_event(Some(top), "lead", "forwarded", &json!({"remarks": "ok", "pr_url": "https://x.test/pr/2"})).unwrap();
        db.add_event(Some(top), "github", "pr_closed", &json!({"url": "https://x.test/pr/2"})).unwrap();
        let th = get(&db, top, real_now());
        let closed = th.nodes.iter().find(|n| n.title == "PR closed without merge").unwrap();
        assert_eq!((closed.lane, closed.kind, closed.status), ("out", "pr", "failed"));
    }

    #[test]
    fn started_deploys_are_out_nodes() {
        let (db, top, _) = setup(TaskStatus::Done);
        let req = db.add_event(Some(top), "scheduler", "deploy_requested", &json!({"environment": "production", "command": "x"})).unwrap();
        let deploys = |db: &Db| get(db, top, real_now()).nodes.into_iter().filter(|n| n.kind == "deploy").collect::<Vec<_>>();
        assert!(deploys(&db).is_empty(), "an unstarted request is an Inbox item, not an output");
        db.add_event(Some(top), "owner", "deploy_started", &json!({"event_id": req})).unwrap();
        assert_eq!((deploys(&db)[0].lane, deploys(&db)[0].status), ("out", "running"));
        db.add_event(Some(top), "scheduler", "deploy_finished", &json!({"event_id": req, "ok": false, "output_tail": "boom"})).unwrap();
        let n = &deploys(&db)[0];
        assert_eq!((n.status, n.title.as_str()), ("failed", "Deploy to production"));
        assert_eq!(n.output.as_ref().unwrap().output_tail.as_deref(), Some("boom"));
        db.add_event(Some(top), "owner", "deploy_started", &json!({"event_id": req})).unwrap();
        db.add_event(Some(top), "scheduler", "deploy_finished", &json!({"event_id": req, "ok": true, "output_tail": "ok"})).unwrap();
        assert_eq!(deploys(&db)[0].status, "done", "a retry that passed wins");
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
    fn questions_closed_by_a_drop_are_not_shown_as_answers() {
        let (db, _top, sub) = setup(TaskStatus::InProgress);
        let q = db.create_ladder_question(sub, Role::Builder, "owner", &[], "Pick").unwrap();
        db.answer_question(q.id, crate::control::DROPPED_ANSWER, None).unwrap();
        let th = get(&db, _top, real_now());
        assert!(!kinds(&th).contains(&"answer"));
    }

    #[test]
    fn plan_node_shows_the_planning_turn_steps_and_cost() {
        let (db, top, _sub) = setup(TaskStatus::InProgress);
        let think = db.add_event(Some(top), "lead", "assistant", &json!({"text": "two parts"})).unwrap();
        let call = db.add_event(Some(top), "lead", "tool_call", &json!({"tool": "create_subtask"})).unwrap();
        let done = db.add_event(Some(top), "lead", "result", &json!({"total_cost_usd": 0.4})).unwrap();
        db.add_event(Some(top), "lead", "assistant", &json!({"text": "reviewing"})).unwrap();
        db.add_event(Some(top), "lead", "result", &json!({"total_cost_usd": 0.3})).unwrap();
        let th = get(&db, top, real_now());
        let plan = th.nodes.iter().find(|n| n.kind == "plan").unwrap();
        let out = plan.output.as_ref().expect("the plan lists its steps");
        assert_eq!((out.label.as_str(), out.preview.as_deref()), ("1 step", Some("1. work")));
        let seen = inspect(&db, &th, &plan.id, 0).unwrap().unwrap();
        let ids: Vec<i64> = seen.events.iter().map(|e| e.id).collect();
        assert_eq!(ids, [think, call, done], "only the planning turn");
        assert!((seen.cost_usd - 0.4).abs() < 1e-9);
    }

    #[test]
    fn running_step_shows_its_turns_and_notes_but_not_thinking_pings() {
        let (db, top, sub) = setup(TaskStatus::InProgress);
        let said = db.add_event(Some(sub), "builder", "assistant", &json!({"text": "scaffolding"})).unwrap();
        let note = db.add_event(Some(sub), "builder", "progress", &json!({"note": "next: mock data"})).unwrap();
        db.add_event(Some(sub), "builder", "other", &json!({"subtype": "thinking_tokens"})).unwrap();
        db.add_event(Some(sub), "builder", "toolresult", &json!({})).unwrap();
        let th = get(&db, top, real_now());
        let step = th.nodes.iter().find(|n| n.subtask_id == Some(sub) && n.kind == "subtask").unwrap();
        let seen = inspect(&db, &th, &step.id, 0).unwrap().unwrap();
        let ids: Vec<i64> = seen.events.iter().map(|e| e.id).filter(|id| [said, note].contains(id)).collect();
        assert_eq!(ids, [said, note]);
        assert!(seen.events.iter().all(|e| e.kind != "other" && e.kind != "toolresult"));
        let later = inspect(&db, &th, &step.id, note).unwrap().unwrap();
        assert!(later.events.iter().all(|e| e.id > note), "follow-up pages only bring newer events");
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

    #[test]
    fn stuck_answer_shows_once_as_an_answer() {
        let (db, top, sub) = setup(TaskStatus::Stuck);
        db.set_task_status(top, TaskStatus::Stuck).unwrap();
        db.add_event(Some(sub), "scheduler", "agent_error", &json!({"error": "boom"})).unwrap();
        crate::scheduler::raise_stuck_question(&db, top, "boom").unwrap();
        let q = db.questions_for_task(top).unwrap().pop().unwrap();
        crate::scheduler::answer_question(&db, q.id, "why we use astro?", None, "owner").unwrap();
        let inst = db.pending_instructions(top).unwrap();
        assert_eq!(inst[0].1, "why we use astro?");
        let th = get(&db, top, real_now());
        let ins: Vec<_> = th.nodes.iter().filter(|n| n.kind == "instruction").collect();
        let ans: Vec<_> = th.nodes.iter().filter(|n| n.kind == "answer").map(|n| n.title.clone()).collect();
        assert!(ins.is_empty(), "{ins:?}");
        assert_eq!(ans, vec!["You answered: why we use astro?".to_string()]);
    }

    fn listed(db: &Db, all: bool, now: i64) -> usize {
        let show = if all { Show::All } else { Show::Recent };
        summaries(db, None, None, show, now, STALL_SECS).unwrap().len()
    }

    /// A done thread and the epoch second of its last activity.
    fn done_thread() -> (Db, i64) {
        let (db, top, sub) = setup(TaskStatus::Done);
        db.set_task_status(top, TaskStatus::Done).unwrap();
        let last = *db.last_activity(&[top, sub]).unwrap().values().max().unwrap();
        (db, last)
    }

    #[test]
    fn old_done_thread_hidden_unless_all() {
        let (db, last) = done_thread();
        let now = last + DONE_KEEP_SECS + 1;
        assert_eq!(listed(&db, false, now), 0);
        assert_eq!(listed(&db, true, now), 1);
    }

    #[test]
    fn recent_done_thread_shown() {
        let (db, last) = done_thread();
        assert_eq!(listed(&db, false, last + 60), 1);
    }

    #[test]
    fn old_open_thread_shown() {
        let (db, _, sub) = setup(TaskStatus::InProgress);
        let last = db.last_activity(&[sub]).unwrap()[&sub];
        assert_eq!(listed(&db, false, last + DONE_KEEP_SECS + 1), 1);
    }
}

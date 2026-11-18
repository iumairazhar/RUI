//! Blocking HTTP client for the RUI daemon.

use anyhow::Result;
use serde::Deserialize;
use rui_core::{Chat, ChatAsk, ChatMessage, Client, Event, Question, SeatInfo, Task, TaskRun};
use std::collections::HashMap;
use std::time::Duration;

mod costs;
mod support;
pub use costs::*;
pub use support::*;

/// Base URL of the daemon (`RUI_URL` or the default local address).
pub fn base_url() -> String {
    std::env::var("RUI_URL").unwrap_or_else(|_| "http://127.0.0.1:7878".into())
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(2)))
        .timeout_recv_response(Some(Duration::from_secs(5)))
        .timeout_recv_body(Some(Duration::from_secs(5)))
        .build()
        .into()
}

fn get(path: &str) -> Result<serde_json::Value> {
    let url = format!("{}{}", base_url(), path);
    Ok(agent().get(&url).call()?.body_mut().read_json()?)
}

fn post(path: &str, body: serde_json::Value) -> Result<()> {
    let url = format!("{}{}", base_url(), path);
    agent().post(&url).send_json(body)?;
    Ok(())
}

fn post_value(path: &str, body: serde_json::Value) -> Result<serde_json::Value> {
    let url = format!("{}{}", base_url(), path);
    Ok(agent().post(&url).send_json(body)?.body_mut().read_json()?)
}

/// Fetch all clients.
pub fn get_clients() -> Result<Vec<Client>> {
    Ok(serde_json::from_value(get("/clients")?)?)
}

/// Fetch all tasks.
pub fn get_tasks() -> Result<Vec<Task>> {
    Ok(serde_json::from_value(get("/tasks")?)?)
}

/// Fetch open questions.
pub fn get_questions_open() -> Result<Vec<Question>> {
    Ok(serde_json::from_value(get("/questions?status=open")?)?)
}

/// Fetch all chats, newest updated first.
pub fn get_chats() -> Result<Vec<Chat>> {
    Ok(serde_json::from_value(get("/chats")?)?)
}

/// Create a chat; the daemon seeds it with Rui's first message.
pub fn create_chat() -> Result<Chat> {
    let url = format!("{}/chats", base_url());
    let body = agent().post(&url).send_json(serde_json::json!({}))?.body_mut().read_json()?;
    Ok(serde_json::from_value(body)?)
}

/// Fetch the latest 200 messages of one chat, oldest first.
pub fn get_chat(chat_id: i64) -> Result<Vec<ChatMessage>> {
    Ok(serde_json::from_value(get(&format!("/chat?chat_id={chat_id}&limit=200"))?)?)
}

/// Fetch the signed-in seat info.
pub fn get_seat() -> Result<SeatInfo> {
    Ok(serde_json::from_value(get("/seat")?)?)
}

/// One item that needs the owner, from `GET /inbox`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct InboxItem {
    /// "question", "ask", "stuck" or "review".
    pub kind: String,
    pub id: i64,
    #[serde(default)]
    pub task_id: Option<i64>,
    pub title: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub actions: Vec<String>,
    #[serde(default)]
    pub created_at: String,
    /// Longer text, like a Scout report or the text a Coach proposal adds.
    #[serde(default)]
    pub detail: Option<String>,
    /// Where to act outside RUI, like a PR link.
    #[serde(default)]
    pub link: Option<String>,
}

/// A task's owner-facing status word and reason, from `GET /tasks/state`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct OwnerState {
    pub task_id: i64,
    /// "Needs you", "Working", "Waiting", "Stopped" or "Done".
    pub word: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub since: String,
}

/// Fetch everything that needs the owner, newest first.
pub fn get_inbox() -> Result<Vec<InboxItem>> {
    Ok(serde_json::from_value(get("/inbox")?)?)
}

/// Fetch every task's status word and reason.
pub fn get_task_states() -> Result<Vec<OwnerState>> {
    Ok(serde_json::from_value(get("/tasks/state")?)?)
}

/// Fetch the latest run (model and seat) per task.
pub fn get_task_runs() -> Result<Vec<TaskRun>> {
    Ok(serde_json::from_value(get("/tasks/runs")?)?)
}

/// Fetch events oldest first: those after `after_id`, or the newest `limit` if `None`.
pub fn get_events_after(after_id: Option<i64>, limit: usize) -> Result<Vec<Event>> {
    let path = match after_id {
        Some(id) => format!("/events?after_id={id}&limit={limit}"),
        None => format!("/events?limit={limit}"),
    };
    Ok(serde_json::from_value(get(&path)?)?)
}

/// Fetch one task's newest events, oldest first.
pub fn task_events(task_id: i64) -> Result<Vec<Event>> {
    Ok(serde_json::from_value(get(&format!("/events?task_id={task_id}&limit=400"))?)?)
}

/// One file changed by a task.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Change {
    pub file: String,
    pub added: i64,
    pub removed: i64,
    /// Unified diff text; empty when the daemon does not send one.
    #[serde(default)]
    pub diff: String,
}

/// A file's content from a task's worktree.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TaskFile {
    pub path: String,
    pub content: String,
    #[serde(default)]
    pub truncated: bool,
}

/// What one running agent turn is doing right now.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct LiveState {
    /// Streaming assistant text (tail-capped by the daemon).
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub thinking: bool,
    /// Tool currently being called.
    #[serde(default)]
    pub tool: Option<String>,
    /// Partial JSON of the tool input; may be incomplete.
    #[serde(default)]
    pub tool_input: String,
    #[serde(default)]
    pub at: String,
}

/// Live state of every running task and chat turn, keyed by id.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct LiveSnapshot {
    #[serde(default)]
    pub tasks: HashMap<String, LiveState>,
    #[serde(default)]
    pub chats: HashMap<String, LiveState>,
}

impl LiveSnapshot {
    /// Live state of a task, if its turn is running.
    pub fn task(&self, id: i64) -> Option<&LiveState> {
        self.tasks.get(&id.to_string())
    }

    /// Live state of a chat, if its turn is running.
    pub fn chat(&self, id: i64) -> Option<&LiveState> {
        self.chats.get(&id.to_string())
    }
}

/// Fetch what running tasks and chats are doing right now.
pub fn get_live() -> Result<LiveSnapshot> {
    Ok(serde_json::from_value(get("/live")?)?)
}

/// Percent-encode everything except unreserved characters and '/'.
fn encode_path(path: &str) -> String {
    let keep = |b: u8| b.is_ascii_alphanumeric() || b"-._~/".contains(&b);
    path.bytes().map(|b| if keep(b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// Fetch one file of a task's worktree.
pub fn task_file(id: i64, path: &str) -> Result<TaskFile> {
    let url = format!("/tasks/{id}/file?path={}", encode_path(path));
    Ok(serde_json::from_value(get(&url)?)?)
}

/// A file a task (or its team) produced in the project repo.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ResultFile {
    pub path: String,
    pub abs_path: String,
    #[serde(default)]
    pub size: u64,
    pub task_id: i64,
    #[serde(default)]
    pub agent: Option<String>,
}

/// Fetch the files a task and its team produced.
pub fn get_results(task_id: i64) -> Result<Vec<ResultFile>> {
    Ok(serde_json::from_value(get(&format!("/tasks/{task_id}/results"))?)?)
}

/// Fetch one produced file from the project repo.
pub fn get_result_file(task_id: i64, path: &str) -> Result<TaskFile> {
    let url = format!("/tasks/{task_id}/result-file?path={}", encode_path(path));
    Ok(serde_json::from_value(get(&url)?)?)
}

/// One app a team started for a project.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AppInfo {
    pub project: String,
    pub name: String,
    pub port: u16,
    pub url: String,
    pub status: String,
    #[serde(default)]
    pub log: Vec<String>,
}

/// One history line of a skill or role.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Hist {
    pub when: String,
    pub who: String,
    pub what: String,
    #[serde(default)]
    pub why: Option<String>,
    #[serde(default)]
    pub from: Option<String>,
}

/// An older saved version of a skill or role; `ts` is unix milliseconds, newest first.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Version {
    pub ts: u64,
}

/// Where a skill is attached: roles by name, clients by name.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Attach {
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub clients: Vec<String>,
}

fn status_on() -> String {
    "on".into()
}

/// One skill (SKILL.md) from the daemon. `text` and `versions` only come with `GET /skills/{name}`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub bash: Vec<String>,
    #[serde(default)]
    pub run_hint: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    pub body: String,
    #[serde(default = "status_on")]
    pub status: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub attach: Attach,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub hist: Vec<Hist>,
    #[serde(default)]
    pub used_7d: i64,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub versions: Vec<Version>,
}

/// One agent role from the roster. `text` and `versions` only come with `GET /roles/{name}`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct RoleInfo {
    pub name: String,
    pub code: String,
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub max_loops: i64,
    #[serde(default = "status_on")]
    pub status: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub owns_checks: bool,
    #[serde(default)]
    pub hist: Vec<Hist>,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub versions: Vec<Version>,
}

/// Fetch all apps, sorted by project then name.
pub fn get_apps() -> Result<Vec<AppInfo>> {
    Ok(serde_json::from_value(get("/apps")?)?)
}

/// Fetch all skills.
pub fn get_skills() -> Result<Vec<SkillInfo>> {
    Ok(serde_json::from_value(get("/skills")?)?)
}

/// Fetch all roles, sorted by name.
pub fn get_roles() -> Result<Vec<RoleInfo>> {
    Ok(serde_json::from_value(get("/roles")?)?)
}

/// Fetch the asks Rui has open.
pub fn get_asks_open() -> Result<Vec<ChatAsk>> {
    Ok(serde_json::from_value(get("/asks?status=open")?)?)
}

/// Fetch the asks already answered.
pub fn get_asks_answered() -> Result<Vec<ChatAsk>> {
    Ok(serde_json::from_value(get("/asks?status=answered")?)?)
}

/// Answer one of Rui's asks.
pub fn post_ask_answer(ask_id: i64, answer: &str) -> Result<()> {
    post(&format!("/asks/{ask_id}/answer"), serde_json::json!({ "answer": answer }))
}

/// The `error` text of a daemon error body, else the status line.
fn error_text(status: u16, body: &serde_json::Value) -> String {
    match body["error"].as_str() {
        Some(e) => e.to_string(),
        None => format!("Request failed ({status})"),
    }
}

/// A library call (skills and roles). Errors carry the daemon's `{error}` text.
pub fn library(method: &str, path: &str, body: Option<serde_json::Value>) -> Result<serde_json::Value> {
    let cfg = ureq::Agent::config_builder().http_status_as_error(false);
    let agent: ureq::Agent = cfg.timeout_connect(Some(Duration::from_secs(2))).timeout_global(Some(Duration::from_secs(60))).build().into();
    let url = format!("{}{}", base_url(), path);
    let body = body.unwrap_or_else(|| serde_json::json!({}));
    let mut res = match method {
        "GET" => agent.get(&url).call()?,
        "DELETE" => agent.delete(&url).call()?,
        "PUT" => agent.put(&url).send_json(body)?,
        _ => agent.post(&url).send_json(body)?,
    };
    let json: serde_json::Value = res.body_mut().read_json().unwrap_or(serde_json::Value::Null);
    let status = res.status();
    if status.is_success() {
        return Ok(json);
    }
    Err(anyhow::anyhow!(error_text(status.as_u16(), &json)))
}

/// One file inside a skill folder. `text` is null for binary or oversized files.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SkillFile {
    pub path: String,
    pub size: u64,
    pub text: Option<String>,
}

/// Fetch the files of a skill (SKILL.md first).
pub fn skill_files(name: &str) -> Result<serde_json::Value> {
    library("GET", &format!("/skills/{}/files", encode_path(name)), None)
}

/// Create a client from a repo folder. Errors carry the daemon's `{error}` text.
pub fn create_client(name: &str, repo_path: &str) -> Result<serde_json::Value> {
    library("POST", "/clients", Some(serde_json::json!({ "name": name, "repo_path": repo_path })))
}

/// What the owner wrote down about a client, as `{text}`.
pub fn get_knowledge(client_id: i64) -> Result<serde_json::Value> {
    library("GET", &format!("/clients/{client_id}/knowledge"), None)
}

/// Append one dated bullet to a client's knowledge; returns the new `{text}`.
pub fn add_knowledge(client_id: i64, text: &str) -> Result<serde_json::Value> {
    library("POST", &format!("/clients/{client_id}/knowledge"), Some(serde_json::json!({ "text": text })))
}

/// A client trigger: start a task when `on` happens ("pr_merged", "email" or "schedule").
#[derive(Debug, Clone, PartialEq, serde::Serialize, Deserialize)]
pub struct Trigger {
    pub id: String,
    pub on: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron: Option<String>,
    pub title: String,
    #[serde(default)]
    pub body: String,
}

/// A client's triggers.
pub fn get_triggers(client_id: i64) -> Result<Vec<Trigger>> {
    Ok(serde_json::from_value(library("GET", &format!("/clients/{client_id}/triggers"), None)?)?)
}

/// Replace a client's whole trigger list. Errors carry the daemon's `{error}` text.
pub fn set_triggers(client_id: i64, list: &[Trigger]) -> Result<Vec<Trigger>> {
    let body = serde_json::to_value(list)?;
    Ok(serde_json::from_value(library("POST", &format!("/clients/{client_id}/triggers"), Some(body))?)?)
}

/// Restart a project's apps.
pub fn restart_apps(project: &str) -> Result<()> {
    post(&format!("/apps/{}/restart", encode_path(project)), serde_json::json!({}))
}

/// Stop a project's apps.
pub fn stop_apps(project: &str) -> Result<()> {
    post(&format!("/apps/{}/stop", encode_path(project)), serde_json::json!({}))
}

/// Ids in a `{stopped: [ids]}` reply.
fn stopped_ids(v: &serde_json::Value) -> Vec<i64> {
    v["stopped"].as_array().map(|a| a.iter().filter_map(|x| x.as_i64()).collect()).unwrap_or_default()
}

/// Body of a stop call: where it came from and an optional reason.
fn stop_body(source: &str, reason: Option<&str>) -> serde_json::Value {
    serde_json::json!({ "source": source, "reason": reason })
}

/// Stop a task and its unfinished subtasks; returns every id stopped.
pub fn stop(id: i64, source: &str, reason: Option<&str>) -> Result<Vec<i64>> {
    Ok(stopped_ids(&post_value(&format!("/tasks/{id}/stop"), stop_body(source, reason))?))
}

/// Stop every running team; returns every id stopped.
pub fn stop_all(source: &str) -> Result<Vec<i64>> {
    Ok(stopped_ids(&post_value("/stop-all", stop_body(source, None))?))
}

/// Pass new instructions down to a task; returns the daemon's note about what happens next.
pub fn instruct(id: i64, text: &str) -> Result<Option<String>> {
    let v = post_value(&format!("/tasks/{id}/instruct"), serde_json::json!({ "text": text }))?;
    Ok(v["note"].as_str().map(str::to_string))
}

/// Review landed work: verdict "ok" or "changes" (with the owner's text).
pub fn review(id: i64, verdict: &str, text: Option<&str>) -> Result<()> {
    post(&format!("/tasks/{id}/review"), serde_json::json!({ "verdict": verdict, "text": text }))
}

/// Drop a stuck task.
pub fn drop_task(id: i64) -> Result<()> {
    post(&format!("/tasks/{id}/drop"), serde_json::json!({}))
}

/// Retry a stuck task.
pub fn retry_task(id: i64) -> Result<()> {
    post(&format!("/tasks/{id}/retry"), serde_json::json!({}))
}

/// Fetch the files a task changed.
pub fn changes(id: i64) -> Result<Vec<Change>> {
    Ok(serde_json::from_value(get(&format!("/tasks/{id}/changes"))?)?)
}

/// Delete a chat. Errors carry the daemon's `{error}` text (409 while Rui is answering).
pub fn delete_chat(id: i64) -> Result<()> {
    library("DELETE", &format!("/chats/{id}"), None).map(|_| ())
}

/// Post a user message to a chat.
pub fn post_chat(chat_id: i64, content: &str) -> Result<()> {
    post("/chat", serde_json::json!({ "chat_id": chat_id, "content": content }))
}

/// How to undo a held owner answer before `run_at` (unix seconds).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Undo {
    pub event_id: i64,
    #[serde(default)]
    pub run_at: i64,
}

/// Answer a question by id, with the owner's reason when given; returns the undo window if any.
pub fn post_answer(question_id: i64, answer: &str, why: Option<&str>) -> Result<Option<Undo>> {
    let path = format!("/questions/{question_id}/answer");
    let v = post_value(&path, serde_json::json!({ "answer": answer, "why": why }))?;
    Ok(serde_json::from_value(v["undo"].clone()).ok())
}

// ---- threads: mirrors of crates/rui-daemon/src/thread.rs ----

/// A thread's client: id and name.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ClientRef {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub name: String,
}

/// One thread on the Work board, from `GET /threads`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ThreadSummary {
    pub id: i64,
    #[serde(default)]
    pub client: ClientRef,
    #[serde(default)]
    pub title: String,
    /// needs_you, failed, stalled, owner, waiting, running, planned or done.
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub story: Option<String>,
    #[serde(default)]
    pub needs_you_count: usize,
    #[serde(default)]
    pub updated_at: String,
}

/// Loops a subtask used out of its maximum.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Attempts {
    #[serde(default)]
    pub used: i64,
    #[serde(default)]
    pub max: i64,
}

/// Where to review the change behind a decision.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ReviewRef {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub task_id: i64,
    #[serde(default)]
    pub url: Option<String>,
}

/// What a node asks the owner to decide.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Decision {
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub question_id: Option<i64>,
    #[serde(default)]
    pub ask: String,
    #[serde(default)]
    pub why: String,
    #[serde(default)]
    pub ladder: Vec<rui_core::Rung>,
    #[serde(default)]
    pub risk: String,
    #[serde(default)]
    pub reversible: Option<bool>,
    #[serde(default)]
    pub if_approved: Vec<String>,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub review: Option<ReviewRef>,
    #[serde(default)]
    pub actions: Vec<String>,
    #[serde(default)]
    pub undo_secs: i64,
}

/// What a node produced: tests, a pull request, a file.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Output {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub preview: Option<String>,
    #[serde(default)]
    pub output_tail: Option<String>,
}

/// One step of a thread, in lane `in`, `work` or `out`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Node {
    pub id: String,
    #[serde(default)]
    pub lane: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub subtask_id: Option<i64>,
    #[serde(default)]
    pub attempts: Option<Attempts>,
    #[serde(default)]
    pub started_at: String,
    #[serde(default)]
    pub ended_at: Option<String>,
    #[serde(default)]
    pub after: Vec<String>,
    #[serde(default)]
    pub decision: Option<Decision>,
    #[serde(default)]
    pub output: Option<Output>,
    #[serde(default)]
    pub cost_usd: Option<f64>,
    #[serde(default)]
    pub event_ids: Vec<i64>,
    #[serde(default)]
    pub reason: Option<String>,
}

/// One thread with all its nodes, from `GET /threads/{id}`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Thread {
    pub id: i64,
    #[serde(default)]
    pub client: ClientRef,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub story: Option<String>,
    #[serde(default)]
    pub deadline: Option<String>,
    #[serde(default)]
    pub needs_you: Vec<String>,
    #[serde(default)]
    pub cost_usd: f64,
    #[serde(default)]
    pub last_event_id: i64,
    #[serde(default)]
    pub nodes: Vec<Node>,
}

/// One run of a done check.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct DoneCheck {
    #[serde(default)]
    pub passed: bool,
    #[serde(default)]
    pub output_tail: String,
}

/// One page of a node's events, its cost and done checks.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Inspector {
    #[serde(default)]
    pub node_id: String,
    #[serde(default)]
    pub events: Vec<Event>,
    #[serde(default)]
    pub cost_usd: f64,
    #[serde(default)]
    pub done_checks: Vec<DoneCheck>,
    #[serde(default)]
    pub next_after_id: Option<i64>,
}

/// Reply to a take-over: the worktree and branch the owner works in.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct TakeOver {
    #[serde(default)]
    pub worktree: Option<String>,
    #[serde(default)]
    pub branch: String,
}

/// Reply to a hand-back: the check result and the step's new status.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct HandBack {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub passed: bool,
    #[serde(default)]
    pub output_tail: String,
}

/// Fetch the threads for the board, needs-you first.
pub fn get_threads() -> Result<Vec<ThreadSummary>> {
    Ok(serde_json::from_value(get("/threads")?)?)
}

/// Fetch one thread with its nodes.
pub fn get_thread(id: i64) -> Result<Thread> {
    Ok(serde_json::from_value(get(&format!("/threads/{id}"))?)?)
}

/// The path of one node's inspector page, after event `after_id` when given.
fn node_path(id: i64, node_id: &str, after_id: Option<i64>) -> String {
    let base = format!("/threads/{id}/nodes/{}", encode_path(node_id));
    match after_id {
        Some(a) => format!("{base}?after_id={a}"),
        None => base,
    }
}

/// Fetch one page of a node's events, cost and done checks.
pub fn get_thread_node(id: i64, node_id: &str, after_id: Option<i64>) -> Result<Inspector> {
    Ok(serde_json::from_value(get(&node_path(id, node_id, after_id))?)?)
}

/// Hold a subtask that has not started (`on`), or release it. Errors carry the daemon's text.
pub fn set_checkpoint(id: i64, on: bool) -> Result<()> {
    library("POST", &format!("/tasks/{id}/checkpoint"), Some(serde_json::json!({ "on": on }))).map(|_| ())
}

/// Stop a subtask's agent so the owner can do the step by hand.
pub fn take_over(id: i64) -> Result<TakeOver> {
    Ok(serde_json::from_value(library("POST", &format!("/tasks/{id}/take-over"), None)?)?)
}

/// Give a taken-over step back with a note; `force` lands it even if its check fails.
pub fn hand_back(id: i64, note: &str, force: bool) -> Result<HandBack> {
    let body = serde_json::json!({ "note": note, "force": force });
    Ok(serde_json::from_value(library("POST", &format!("/tasks/{id}/hand-back"), Some(body))?)?)
}

/// Undo a held owner answer inside its window; returns the reopened question.
pub fn cancel_approval(event_id: i64) -> Result<Question> {
    Ok(serde_json::from_value(library("POST", &format!("/approvals/{event_id}/cancel"), None)?)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rui_core::{ChatRole, QuestionStatus, TaskStatus};

    #[test]
    fn parses_live() {
        let j = r#"{"tasks":{"7":{"text":"hi","thinking":false,"tool":"Bash",
            "tool_input":"{\"command\":\"ls","at":"2026-11-15T10:20:30Z"}},"chats":{"2":{}}}"#;
        let v: LiveSnapshot = serde_json::from_str(j).unwrap();
        let t = v.task(7).unwrap();
        assert_eq!(t.tool.as_deref(), Some("Bash"));
        assert_eq!(t.text, "hi");
        assert_eq!(v.chat(2), Some(&LiveState::default()));
        assert!(v.task(8).is_none() && v.chat(7).is_none());
        let empty: LiveSnapshot = serde_json::from_str(r#"{"tasks":{},"chats":{}}"#).unwrap();
        assert_eq!(empty, LiveSnapshot::default());
    }

    #[test]
    fn parses_client() {
        let j = r#"[{"id":1,"name":"acme","repo_path":"/repo","test_command":null,
            "allowed_commands":["cargo test"],"created_at":"2026-01-01T00:00:00Z"}]"#;
        let v: Vec<Client> = serde_json::from_str(j).unwrap();
        assert_eq!(v[0].name, "acme");
        assert_eq!(v[0].allowed_commands, vec!["cargo test"]);
    }

    #[test]
    fn parses_task() {
        let j = r#"[{"id":2,"client_id":1,"parent_id":null,"title":"t","body":"",
            "status":"in_progress","done_command":null,"max_loops":5,"loops_used":1,
            "role":"lead","branch":null,"source_kind":"user","source_ref":null,
            "depends_on":[1],"created_at":"a","updated_at":"b"}]"#;
        let v: Vec<Task> = serde_json::from_str(j).unwrap();
        assert_eq!(v[0].status, TaskStatus::InProgress);
        assert_eq!(v[0].depends_on, vec![1]);
    }

    #[test]
    fn parses_changes() {
        let j = r#"[{"file":"src/a.rs","added":3,"removed":1}]"#;
        let v: Vec<Change> = serde_json::from_str(j).unwrap();
        assert_eq!(v[0], Change { file: "src/a.rs".into(), added: 3, removed: 1, diff: String::new() });
        let j = r#"[{"file":"a","added":0,"removed":0,"diff":"@@ -1 +1 @@"}]"#;
        let v: Vec<Change> = serde_json::from_str(j).unwrap();
        assert_eq!(v[0].diff, "@@ -1 +1 @@");
    }

    #[test]
    fn parses_result_files() {
        let j = r#"[{"path":"d/a.md","abs_path":"/r/d/a.md","size":12,"modified":5,"task_id":3,"agent":null}]"#;
        let v: Vec<ResultFile> = serde_json::from_str(j).unwrap();
        assert_eq!((v[0].path.as_str(), v[0].size, v[0].task_id, v[0].agent.clone()), ("d/a.md", 12, 3, None));
    }

    #[test]
    fn parses_apps_and_skills() {
        let a = r#"[{"project":"p","name":"api","port":8101,"url":"http://localhost:8101","status":"running","log":["hi"]}]"#;
        let v: Vec<AppInfo> = serde_json::from_str(a).unwrap();
        assert_eq!((v[0].port, v[0].log.len()), (8101, 1));
        let s = r#"[{"name":"x","description":"d","bash":["a *"],"run_hint":null,"body":"b"}]"#;
        let v: Vec<SkillInfo> = serde_json::from_str(s).unwrap();
        assert_eq!((v[0].bash.len(), v[0].run_hint.clone(), v[0].license.clone()), (1, None, None));
        let s = r#"[{"name":"x","description":"d","license":"MIT","source":"https://a.b/c","body":"b"}]"#;
        let v: Vec<SkillInfo> = serde_json::from_str(s).unwrap();
        assert_eq!((v[0].license.as_deref(), v[0].source.as_deref()), (Some("MIT"), Some("https://a.b/c")));
    }

    #[test]
    fn parses_library_fields() {
        let s = r#"{"name":"x","description":"d","body":"b","status":"review","author":"imported",
            "attach":{"roles":["lead"],"clients":["acme"]},"tools":["Read"],"used_7d":4,
            "hist":[{"when":"2026-11-16 10:00","who":"You","what":"imported it","from":"https://a.b"}],
            "text":"t","versions":[{"ts":1700000000000}]}"#;
        let v: SkillInfo = serde_json::from_str(s).unwrap();
        assert_eq!((v.status.as_str(), v.attach.roles.len(), v.used_7d, v.versions[0].ts), ("review", 1, 4, 1_700_000_000_000));
        assert_eq!((v.hist[0].why.clone(), v.hist[0].from.as_deref()), (None, Some("https://a.b")));
        let bare: SkillInfo = serde_json::from_str(r#"{"name":"x","description":"d","body":"b"}"#).unwrap();
        assert_eq!(bare.status, "on");
    }

    #[test]
    fn error_text_prefers_the_error_field() {
        assert_eq!(error_text(400, &serde_json::json!({"error": "bad name"})), "bad name");
        assert_eq!(error_text(500, &serde_json::Value::Null), "Request failed (500)");
    }

    #[test]
    fn parses_asks() {
        let j = r#"[{"id":1,"chat_id":2,"chat_title":"Setup","text":"When?","options":["Today"],
            "answer":null,"status":"open","created_at":"a"}]"#;
        let v: Vec<ChatAsk> = serde_json::from_str(j).unwrap();
        assert_eq!((v[0].options.len(), v[0].chat_title.as_str()), (1, "Setup"));
    }

    #[test]
    fn parses_roles() {
        let j = r#"[{"name":"backend","code":"BE","title":"Backend Engineer","description":"d",
            "tools":["Read"],"skills":["fastapi"],"max_loops":6}]"#;
        let v: Vec<RoleInfo> = serde_json::from_str(j).unwrap();
        assert_eq!((v[0].code.as_str(), v[0].max_loops, v[0].skills.len()), ("BE", 6, 1));
    }

    #[test]
    fn parses_inbox_and_states() {
        let j = r#"[{"kind":"stuck","id":4,"task_id":null,"title":"t","reason":"r","actions":["Retry"],"created_at":"a"}]"#;
        let v: Vec<InboxItem> = serde_json::from_str(j).unwrap();
        assert_eq!((v[0].kind.as_str(), v[0].task_id, v[0].actions.len()), ("stuck", None, 1));
        let s = r#"[{"task_id":2,"word":"Needs you","reason":"Asks: x","since":"a"}]"#;
        let v: Vec<OwnerState> = serde_json::from_str(s).unwrap();
        assert_eq!((v[0].task_id, v[0].word.as_str()), (2, "Needs you"));
    }

    #[test]
    fn stop_body_carries_source_and_reason() {
        let b = stop_body("Work", Some("why"));
        assert_eq!((b["source"].as_str(), b["reason"].as_str()), (Some("Work"), Some("why")));
        assert!(stop_body("Work", None)["reason"].is_null());
    }

    #[test]
    fn encodes_paths() {
        assert_eq!(encode_path("src/a b&c.rs"), "src/a%20b%26c.rs");
    }

    #[test]
    fn parses_question_and_chat() {
        let q = r#"[{"id":3,"task_id":2,"asked_by":"lead","text":"why?","answer":null,
            "status":"open","created_at":"a"}]"#;
        let v: Vec<Question> = serde_json::from_str(q).unwrap();
        assert_eq!(v[0].status, QuestionStatus::Open);
        let c = r#"[{"id":1,"chat_id":4,"role":"user","content":"hi","created_at":"a"}]"#;
        let v: Vec<ChatMessage> = serde_json::from_str(c).unwrap();
        assert_eq!(v[0].role, ChatRole::User);
        assert_eq!(v[0].chat_id, 4);
    }

    #[test]
    fn parses_thread_summaries() {
        let j = r#"[{"id":7,"client":{"id":1,"name":"acme"},"title":"Add login","state":"needs_you",
            "story":"Lead asks: which provider?","needs_you_count":2,"updated_at":"2026-11-18 10:00:00"},
            {"id":8,"title":"bare"}]"#;
        let v: Vec<ThreadSummary> = serde_json::from_str(j).unwrap();
        assert_eq!((v[0].client.name.as_str(), v[0].state.as_str(), v[0].needs_you_count), ("acme", "needs_you", 2));
        assert_eq!(v[0].story.as_deref(), Some("Lead asks: which provider?"));
        assert_eq!((v[1].state.as_str(), v[1].story.clone(), v[1].client.id), ("", None, 0));
    }

    #[test]
    fn parses_thread_with_nodes() {
        let j = r#"{"id":7,"client":{"id":1,"name":"acme"},"title":"t","state":"running","story":null,
            "deadline":null,"needs_you":["task:9"],"cost_usd":1.5,"last_event_id":40,"nodes":[
            {"id":"task:7","lane":"in","kind":"task_in","status":"done","title":"You asked: t","agent":null,
             "subtask_id":null,"attempts":null,"started_at":"a","ended_at":null,"after":[],"decision":null,
             "output":null,"cost_usd":null,"event_ids":[]},
            {"id":"task:9","lane":"work","kind":"subtask","status":"needs_you","title":"Backend: x","agent":"backend",
             "subtask_id":9,"attempts":{"used":1,"max":5},"started_at":"b","ended_at":null,"after":["task:8"],
             "decision":{"source":"question","question_id":3,"ask":"Which db?","why":"","ladder":[{"who":"Lead","why":"y"}],
               "risk":"high","reversible":false,"if_approved":["migrate"],"options":["pg"],"review":{"kind":"pr","task_id":7,"url":null},
               "actions":["Answer"],"undo_secs":10},
             "output":{"kind":"test","label":"Tests fail","url":null,"preview":null,"output_tail":"boom"},
             "cost_usd":0.4,"event_ids":[12],"reason":"You're doing this step"}]}"#;
        let t: Thread = serde_json::from_str(j).unwrap();
        assert_eq!((t.nodes.len(), t.needs_you.len(), t.last_event_id), (2, 1, 40));
        let n = &t.nodes[1];
        assert_eq!((n.attempts.clone().unwrap().max, n.after.len(), n.reason.is_some()), (5, 1, true));
        let d = n.decision.as_ref().unwrap();
        assert_eq!((d.question_id, d.risk.as_str(), d.reversible, d.ladder[0].who.as_str()), (Some(3), "high", Some(false), "Lead"));
        assert_eq!(n.output.as_ref().unwrap().output_tail.as_deref(), Some("boom"));
        assert_eq!(t.nodes[0].reason, None);
    }

    #[test]
    fn parses_inspector_and_replies() {
        let j = r#"{"node_id":"task:9","events":[{"id":1,"task_id":9,"agent":"backend","kind":"done_check",
            "payload":{"passed":true},"created_at":"a"}],"cost_usd":0.2,
            "done_checks":[{"passed":true,"output_tail":"ok"}],"next_after_id":null}"#;
        let v: Inspector = serde_json::from_str(j).unwrap();
        assert_eq!((v.events.len(), v.done_checks[0].passed, v.next_after_id), (1, true, None));
        let h: HandBack = serde_json::from_str(r#"{"status":"done","passed":false,"output_tail":"x"}"#).unwrap();
        assert_eq!((h.status.as_str(), h.passed), ("done", false));
        let u: Undo = serde_json::from_value(serde_json::json!({"event_id":5,"run_at":100})).unwrap();
        assert_eq!(u, Undo { event_id: 5, run_at: 100 });
    }

    #[test]
    fn node_paths_encode_the_node_id() {
        assert_eq!(node_path(7, "task:9", None), "/threads/7/nodes/task%3A9");
        assert_eq!(node_path(7, "ev:3", Some(40)), "/threads/7/nodes/ev%3A3?after_id=40");
    }

    #[test]
    fn parses_chats() {
        let j = r#"[{"id":2,"title":"New chat","created_at":"a","updated_at":"b","last":null}]"#;
        let v: Vec<Chat> = serde_json::from_str(j).unwrap();
        assert_eq!(v[0].last, None);
    }
}

//! JSON HTTP API over the database.

use crate::db::{Db, NewTask};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use crate::apps::{self, AppInfo, Apps};
use crate::live::{self, Live};
use crate::scheduler::Runs;
use crate::results::ResultFile;
use crate::seat::SeatCache;
use crate::worktree::{self, Change};
use rui_core::{
    Chat, ChatMessage, ChatRole, Client, Event, Question, QuestionStatus, Role, SeatInfo, SourceKind, Task, TaskRun,
    TaskStatus,
};
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;

/// Shared state for all handlers.
#[derive(Clone)]
pub struct ApiState {
    pub db: Arc<Db>,
    pub chat_tx: UnboundedSender<i64>,
    pub seat: SeatCache,
    pub runs: Runs,
    pub live: Live,
    pub data_dir: std::path::PathBuf,
    pub apps: Apps,
    /// Owner token required in the `x-rui-owner` header.
    pub owner_token: Arc<str>,
}

/// Error that renders as `{ "error": msg }` with a status code.
pub struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    }
}

type ApiResult<T> = Result<T, ApiError>;

fn bad_request(msg: &str) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, msg.into())
}

fn not_found(msg: &str) -> ApiError {
    ApiError(StatusCode::NOT_FOUND, msg.into())
}

fn require_text(s: &str, field: &str) -> ApiResult<()> {
    if s.trim().is_empty() {
        return Err(bad_request(&format!("{field} must not be empty")));
    }
    Ok(())
}

/// Build the router with all JSON routes.
pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/clients", get(list_clients).post(create_client_http))
        .route("/clients/{id}", get(get_client_http).patch(update_client_http).delete(delete_client_http))
        .route("/tasks", get(list_tasks).post(create_task))
        .route("/questions", get(list_questions))
        .route("/questions/{id}/answer", post(answer_question))
        .route("/approvals/{event_id}/cancel", post(cancel_approval))
        .route("/chats", get(list_chats).post(create_chat))
        .route("/chats/{id}", delete(delete_chat))
        .route("/chat", get(list_chat).post(post_chat))
        .route("/asks", get(list_asks))
        .route("/asks/{id}/answer", post(answer_ask))
        .route("/events", get(list_events))
        .route("/seat", get(get_seat))
        .route("/live", get(get_live))
        .route("/tasks/runs", get(list_task_runs))
        .route("/tasks/stopped", get(list_stopped))
        .route("/tasks/state", get(list_states))
        .route("/threads", get(list_threads))
        .route("/threads/{id}", get(get_thread))
        .route("/threads/{id}/nodes/{node_id}", get(get_thread_node))
        .route("/threads/{id}/hide", post(hide_thread))
        .route("/threads/{id}/unhide", post(unhide_thread))
        .route("/threads/{id}/remove", post(remove_thread))
        .route("/inbox", get(get_inbox))
        .route("/tasks/{id}/changes", get(task_changes))
        .route("/tasks/{id}/file", get(task_file))
        .route("/tasks/{id}/results", get(task_results))
        .route("/tasks/{id}/result-file", get(result_file))
        .route("/tasks/{id}/stop", post(stop_task))
        .route("/stop-all", post(stop_all))
        .route("/tasks/{id}/instruct", post(instruct_task))
        .route("/tasks/{id}/review", post(review_task))
        .route("/tasks/{id}/drop", post(drop_task))
        .route("/tasks/{id}/retry", post(retry_task))
        .route("/tasks/{id}/checkpoint", post(checkpoint_task))
        .route("/tasks/{id}/take-over", post(take_over_task))
        .route("/tasks/{id}/hand-back", post(hand_back_task))
        .route("/maintenance/clean-worktrees", post(clean_worktrees))
        .route("/apps", get(list_apps))
        .route("/apps/{project}/restart", post(restart_apps))
        .route("/apps/{project}/stop", post(stop_apps))
        .route("/apps/{project}/{name}/stop", post(stop_app))
        .merge(crate::library_api::routes())
        .merge(crate::seats_api::routes())
        .merge(crate::support_api::routes())
        .merge(crate::inputs_api::routes())
        .merge(crate::costs::routes())
        .route_layer(axum::middleware::from_fn_with_state(state.owner_token.clone(), crate::owner_auth::require))
        .with_state(state)
}

async fn list_apps(State(s): State<ApiState>) -> Json<Vec<AppInfo>> {
    Json(apps::list(&s.apps))
}

/// Re-reads the project's apps.json and starts them in the background.
async fn restart_apps(State(s): State<ApiState>, Path(project): Path<String>) -> ApiResult<StatusCode> {
    let client = s.db.get_client_by_name(&project)?.ok_or_else(|| not_found("project not found"))?;
    let (apps, db, data_dir) = (s.apps.clone(), s.db.clone(), s.data_dir.clone());
    tokio::spawn(async move {
        crate::scheduler::start_client_apps(&db, &data_dir, &apps, &client).await;
    });
    Ok(StatusCode::ACCEPTED)
}

async fn stop_apps(State(s): State<ApiState>, Path(project): Path<String>) -> Json<Vec<AppInfo>> {
    apps::stop_project(&s.apps, &project);
    Json(apps::list(&s.apps))
}

async fn stop_app(State(s): State<ApiState>, Path((project, name)): Path<(String, String)>) -> Json<Vec<AppInfo>> {
    apps::stop_one(&s.apps, &project, &name);
    Json(apps::list(&s.apps))
}

async fn get_live(State(s): State<ApiState>) -> Json<serde_json::Value> {
    Json(live::snapshot(&s.live))
}

async fn list_clients(State(s): State<ApiState>) -> ApiResult<Json<Vec<Client>>> {
    Ok(Json(s.db.list_clients()?))
}

#[derive(Deserialize)]
struct NewClient {
    name: String,
    repo_path: String,
}

async fn create_client_http(State(s): State<ApiState>, Json(b): Json<NewClient>) -> ApiResult<(StatusCode, Json<Client>)> {
    let name = b.name.trim();
    if !name.is_empty() && s.db.get_client_by_name(name)?.is_some() {
        return Err(conflict(&format!("client '{name}' already exists")));
    }
    let client = crate::mcp::logic::register_client(&s.db, &s.data_dir, name, &b.repo_path, None, &[]).map_err(|e| bad_request(&e.to_string()))?;
    Ok((StatusCode::CREATED, Json(client)))
}

#[derive(serde::Serialize)]
struct ClientDetail {
    #[serde(flatten)]
    client: Client,
    hosts: Vec<String>,
}

fn client_hosts(s: &ApiState, name: &str) -> Vec<String> {
    crate::client_state::read_client(&s.data_dir, name).ok().flatten().map(|c| c.network_hosts).unwrap_or_default()
}

/// One client with its extra allowed web hosts (from its client.json).
async fn get_client_http(State(s): State<ApiState>, Path(id): Path<i64>) -> ApiResult<Json<ClientDetail>> {
    let client = s.db.get_client(id)?.ok_or_else(|| not_found("client not found"))?;
    let hosts = client_hosts(&s, &client.name);
    Ok(Json(ClientDetail { client, hosts }))
}

#[derive(Deserialize)]
struct ClientPatch {
    name: Option<String>,
    repo_path: Option<String>,
    hosts: Option<Vec<String>>,
}

/// Trims hosts, drops empty and repeated ones.
fn clean_hosts(hosts: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for h in hosts.iter().map(|h| h.trim()).filter(|h| !h.is_empty()) {
        if !out.iter().any(|o| o == h) {
            out.push(h.to_string());
        }
    }
    out
}

/// Checks a new name: not empty, usable as a folder name, and not taken by another client.
fn check_new_name(s: &ApiState, old: &Client, name: &str) -> ApiResult<()> {
    require_text(name, "name")?;
    if crate::projects::slug(name).is_empty() {
        return Err(bad_request("name needs at least one letter or digit"));
    }
    if s.db.get_client_by_name(name)?.is_some_and(|c| c.id != old.id) {
        return Err(conflict(&format!("client '{name}' already exists")));
    }
    let (from, to) = (crate::client_state::dir(&s.data_dir, &old.name), crate::client_state::dir(&s.data_dir, name));
    if from != to && to.exists() {
        return Err(conflict("a folder for that name already exists"));
    }
    Ok(())
}

/// Moves the client's state folder to the new name.
fn rename_state_dir(s: &ApiState, old: &str, new: &str) -> ApiResult<()> {
    let (from, to) = (crate::client_state::dir(&s.data_dir, old), crate::client_state::dir(&s.data_dir, new));
    if from != to && from.exists() {
        std::fs::rename(from, to).map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }
    Ok(())
}

/// Changes a client's name, repo folder and allowed web hosts.
async fn update_client_http(State(s): State<ApiState>, Path(id): Path<i64>, Json(b): Json<ClientPatch>) -> ApiResult<Json<ClientDetail>> {
    let old = s.db.get_client(id)?.ok_or_else(|| not_found("client not found"))?;
    let name = b.name.as_deref().map(str::trim).unwrap_or(&old.name).to_string();
    check_new_name(&s, &old, &name)?;
    let repo = b.repo_path.as_deref().map(str::trim).unwrap_or(&old.repo_path);
    let path = crate::scheduler::expand_home(repo);
    if !path.is_dir() {
        return Err(bad_request(&format!("repo_path {} does not exist", path.display())));
    }
    let hosts = b.hosts.as_deref().map(clean_hosts).unwrap_or_else(|| client_hosts(&s, &old.name));
    rename_state_dir(&s, &old.name, &name)?;
    let client = s.db.update_client(id, &name, &path.to_string_lossy())?;
    let mut file = crate::client_state::ensure(&s.data_dir, &client.name, &client.repo_path, None)?;
    file.network_hosts = hosts.clone();
    let json = serde_json::to_string_pretty(&file).map_err(anyhow::Error::from)?;
    std::fs::write(crate::client_state::dir(&s.data_dir, &client.name).join("client.json"), json).map_err(anyhow::Error::from)?;
    if old.name != client.name {
        apps::stop_project(&s.apps, &old.name);
    }
    Ok(Json(ClientDetail { client, hosts }))
}

/// Stops the client's running work and deletes it with its tasks and history. The repo folder and
/// task worktrees stay on disk.
async fn delete_client_http(State(s): State<ApiState>, Path(id): Path<i64>) -> ApiResult<Json<serde_json::Value>> {
    let client = s.db.get_client(id)?.ok_or_else(|| not_found("client not found"))?;
    let open: Vec<i64> = (s.db.list_tasks()?.iter())
        .filter(|t| t.client_id == id && t.parent_id.is_none() && !matches!(t.status, TaskStatus::Done | TaskStatus::Stuck))
        .map(|t| t.id)
        .collect();
    for task in open {
        crate::control::stop_tree(&s.db, &s.runs, task, "owner", Some("Clients"), Some("client deleted"))?;
    }
    apps::stop_project(&s.apps, &client.name);
    s.db.delete_client_cascade(id)?;
    Ok(Json(serde_json::json!({ "deleted": id })))
}

async fn list_tasks(State(s): State<ApiState>) -> ApiResult<Json<Vec<Task>>> {
    Ok(Json(s.db.list_tasks()?))
}

#[derive(Deserialize)]
struct QuestionsQuery {
    status: Option<String>,
}

async fn list_questions(
    State(s): State<ApiState>,
    Query(q): Query<QuestionsQuery>,
) -> ApiResult<Json<Vec<Question>>> {
    let status = match q.status {
        None => None,
        Some(v) => Some(QuestionStatus::parse(&v).ok_or_else(|| bad_request("invalid status"))?),
    };
    let owner_only = s.db.list_questions(status)?.into_iter().filter(|q| q.level == "owner").collect();
    Ok(Json(owner_only))
}

/// Builds Rui's opening line for a new chat from the time of day and what is going on.
pub fn greeting(hour: u32, in_loop: usize, needs_you: usize, open: usize) -> String {
    let opener = match hour {
        0..12 => "Good morning.",
        12..18 => "Good afternoon.",
        _ => "Good evening.",
    };
    let close = match needs_you {
        0 => "What do you want to set up?",
        _ => "Want to start with those, or set up something new?",
    };
    format!("{opener} {} {close}", state_line(in_loop, needs_you, open))
}

/// The middle sentence of the greeting: what needs the owner, runs, or is open.
fn state_line(in_loop: usize, needs_you: usize, open: usize) -> String {
    match (needs_you, in_loop, open) {
        (1, _, _) => "1 thing waits for you in the Inbox.".into(),
        (n, _, _) if n > 1 => format!("{n} things wait for you in the Inbox."),
        (_, 1, _) => "1 thread is in the loop.".into(),
        (_, n, _) if n > 1 => format!("{n} threads are in the loop."),
        (_, _, 1) => "1 thread is open, none running.".into(),
        (_, _, n) if n > 1 => format!("{n} threads are open, none running."),
        _ => "All quiet.".into(),
    }
}

/// Counts (in the loop, needs the owner, open) for the greeting.
fn greeting_counts(db: &Db) -> anyhow::Result<(usize, usize, usize)> {
    let tasks = db.list_tasks()?;
    let top = || tasks.iter().filter(|t| t.parent_id.is_none());
    let open = top().filter(|t| t.status != TaskStatus::Done).count();
    let in_loop = top().filter(|t| t.status == TaskStatus::InProgress).count();
    let hidden = crate::hide::hidden(db)?;
    let top_of = |id: i64| tasks.iter().find(|t| t.id == id).map_or(id, |t| t.parent_id.unwrap_or(t.id));
    let questions = db.list_questions(Some(QuestionStatus::Open))?;
    let owner_qs = questions.iter().filter(|q| q.level == "owner" && !hidden.is_hidden(top_of(q.task_id))).count();
    let needs_you = owner_qs + db.list_chat_asks(Some("open"))?.len();
    Ok((in_loop, needs_you, open))
}

async fn list_chats(State(s): State<ApiState>) -> ApiResult<Json<Vec<Chat>>> {
    Ok(Json(s.db.list_chats()?))
}

/// Creates a chat that opens with a Rui greeting.
/// Optional body of `POST /chats`. The app sends the Inbox count it shows, because only the
/// app knows which items the owner snoozed or dismissed.
#[derive(Deserialize, Default)]
struct NewChat {
    needs_you: Option<usize>,
}

async fn create_chat(State(s): State<ApiState>, body: Option<Json<NewChat>>) -> ApiResult<(StatusCode, Json<Chat>)> {
    let chat = s.db.create_chat()?;
    let (in_loop, counted, open) = greeting_counts(&s.db)?;
    let needs_you = body.and_then(|b| b.0.needs_you).unwrap_or(counted);
    let hour = chrono::Timelike::hour(&chrono::Local::now());
    s.db.add_chat_message(chat.id, ChatRole::Rui, &greeting(hour, in_loop, needs_you, open))?;
    let chat = s.db.get_chat(chat.id)?.ok_or_else(|| not_found("chat not found"))?;
    Ok((StatusCode::CREATED, Json(chat)))
}

#[derive(Deserialize)]
struct ChatQuery {
    chat_id: Option<i64>,
    limit: Option<i64>,
}

/// Deletes a chat; refused while Rui is mid-turn in it.
async fn delete_chat(State(s): State<ApiState>, Path(id): Path<i64>) -> ApiResult<StatusCode> {
    let busy = s.live.lock().map(|m| m.contains_key(&live::LiveKey::Chat(id))).unwrap_or(false);
    if busy {
        return Err(conflict("Rui is still answering in this chat. Stop it or wait, then delete."));
    }
    if !s.db.delete_chat(id)? {
        return Err(not_found("chat not found"));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn list_chat(
    State(s): State<ApiState>,
    Query(q): Query<ChatQuery>,
) -> ApiResult<Json<Vec<ChatMessage>>> {
    let chat_id = q.chat_id.ok_or_else(|| bad_request("chat_id is required"))?;
    if s.db.get_chat(chat_id)?.is_none() {
        return Err(not_found("chat not found"));
    }
    Ok(Json(s.db.list_chat(chat_id, q.limit.unwrap_or(100))?))
}

#[derive(Deserialize)]
struct EventsQuery {
    task_id: Option<i64>,
    limit: Option<i64>,
    after_id: Option<i64>,
}

async fn list_events(
    State(s): State<ApiState>,
    Query(q): Query<EventsQuery>,
) -> ApiResult<Json<Vec<Event>>> {
    let limit = q.limit.unwrap_or(200);
    let events = match q.after_id {
        Some(after) => s.db.list_events_after(q.task_id, after, limit)?,
        None => s.db.list_events(q.task_id, limit)?,
    };
    Ok(Json(events))
}

async fn get_seat(State(s): State<ApiState>) -> ApiResult<Json<SeatInfo>> {
    let mut info = s.seat.read().await.clone();
    info.last_model = s.db.latest_model()?;
    (info.limits, info.limits_seen_at) = crate::seats::current_limits(&s.db)?;
    Ok(Json(info))
}

fn seat_kind(payload: &serde_json::Value) -> &'static str {
    match payload.get("apiKeySource").and_then(|v| v.as_str()) {
        Some("none") => "subscription",
        Some(_) => "api_key",
        None => "unknown",
    }
}

/// Ids of tasks whose state word is "Stopped" right now.
async fn list_stopped(State(s): State<ApiState>) -> ApiResult<Json<Vec<i64>>> {
    let states = crate::owner_state::all_states(&s.db)?;
    Ok(Json(states.into_iter().filter(|o| o.word == "Stopped").map(|o| o.task_id).collect()))
}

/// The owner-facing word and reason of every task.
async fn list_states(State(s): State<ApiState>) -> ApiResult<Json<Vec<crate::owner_state::OwnerState>>> {
    Ok(Json(crate::owner_state::all_states(&s.db)?))
}

#[derive(Deserialize)]
struct ThreadsQuery {
    client_id: Option<i64>,
    state: Option<String>,
    all: Option<String>,
    hidden: Option<String>,
}

fn flag(v: Option<&str>) -> bool {
    matches!(v, Some("1" | "true"))
}

/// Threads (top-level tasks as conversations), needs-you first.
async fn list_threads(State(s): State<ApiState>, Query(q): Query<ThreadsQuery>) -> ApiResult<Json<Vec<crate::thread::ThreadSummary>>> {
    use crate::thread::Show;
    let show = match (flag(q.hidden.as_deref()), flag(q.all.as_deref())) {
        (true, _) => Show::Hidden,
        (_, true) => Show::All,
        _ => Show::Recent,
    };
    let list = crate::thread::summaries(&s.db, q.client_id, q.state.as_deref(), show, crate::thread::now_secs(), crate::thread::STALL_SECS)?;
    Ok(Json(list))
}

/// Hides a thread from the board, the Inbox and the needs-you counts.
async fn hide_thread(State(s): State<ApiState>, Path(id): Path<i64>) -> ApiResult<Json<serde_json::Value>> {
    crate::hide::hide(&s.db, id)?;
    Ok(Json(serde_json::json!({ "id": id, "hidden": true })))
}

/// Brings a hidden thread back to the board.
async fn unhide_thread(State(s): State<ApiState>, Path(id): Path<i64>) -> ApiResult<Json<serde_json::Value>> {
    crate::hide::unhide(&s.db, id)?;
    Ok(Json(serde_json::json!({ "id": id, "hidden": false })))
}

/// Drops an unfinished thread with its team and hides it for good.
async fn remove_thread(State(s): State<ApiState>, Path(id): Path<i64>) -> ApiResult<Json<serde_json::Value>> {
    let dropped = crate::hide::remove(&s.db, &s.runs, id)?;
    Ok(Json(serde_json::json!({ "id": id, "dropped": dropped })))
}

async fn get_thread(State(s): State<ApiState>, Path(id): Path<i64>) -> ApiResult<Json<crate::thread::Thread>> {
    let th = crate::thread::thread(&s.db, id, crate::thread::now_secs(), crate::thread::STALL_SECS)?;
    th.map(Json).ok_or_else(|| not_found("thread not found"))
}

#[derive(Deserialize)]
struct NodeQuery {
    after_id: Option<i64>,
}

/// One node's events, cost and done checks, a page at a time.
async fn get_thread_node(
    State(s): State<ApiState>,
    Path((id, node_id)): Path<(i64, String)>,
    Query(q): Query<NodeQuery>,
) -> ApiResult<Json<crate::thread::Inspector>> {
    let th = crate::thread::thread(&s.db, id, crate::thread::now_secs(), crate::thread::STALL_SECS)?;
    let th = th.ok_or_else(|| not_found("thread not found"))?;
    let seen = crate::thread::inspect(&s.db, &th, &node_id, q.after_id.unwrap_or(0))?;
    seen.map(Json).ok_or_else(|| not_found("node not found"))
}

/// Everything that needs the owner, newest first.
async fn get_inbox(State(s): State<ApiState>) -> ApiResult<Json<Vec<crate::inbox::InboxItem>>> {
    Ok(Json(crate::inbox::inbox(&s.db)?))
}

async fn list_task_runs(State(s): State<ApiState>) -> ApiResult<Json<Vec<TaskRun>>> {
    let runs = s
        .db
        .latest_init_per_task()?
        .into_iter()
        .filter_map(|e| {
            Some(TaskRun {
                task_id: e.task_id?,
                agent: e.agent,
                model: e.payload.get("model").and_then(|v| v.as_str()).map(String::from),
                seat: seat_kind(&e.payload).into(),
                at: e.created_at,
            })
        })
        .collect();
    Ok(Json(runs))
}

#[derive(Deserialize)]
struct ChatBody {
    chat_id: i64,
    content: String,
    #[serde(default)]
    client: Option<String>,
}

async fn post_chat(
    State(s): State<ApiState>,
    Json(b): Json<ChatBody>,
) -> ApiResult<(StatusCode, Json<ChatMessage>)> {
    require_text(&b.content, "content")?;
    if s.db.get_chat(b.chat_id)?.is_none() {
        return Err(not_found("chat not found"));
    }
    s.db.close_chat_asks(b.chat_id)?;
    let msg = s.db.add_chat_message(b.chat_id, ChatRole::User, &b.content)?;
    if let Some(c) = b.client.as_deref().and_then(|n| s.db.get_client_by_name(n).ok().flatten()) {
        crate::chat::pick_client(msg.id, &c.name);
    }
    let _ = s.chat_tx.send(msg.id);
    Ok((StatusCode::CREATED, Json(msg)))
}

#[derive(Deserialize)]
struct AsksQuery {
    #[serde(default)]
    status: Option<String>,
}

/// Rui's questions to the owner, optionally only `?status=open` or `answered`.
async fn list_asks(State(s): State<ApiState>, Query(q): Query<AsksQuery>) -> ApiResult<Json<Vec<rui_core::ChatAsk>>> {
    if q.status.as_deref().is_some_and(|st| st != "open" && st != "answered") {
        return Err(bad_request("status must be open or answered"));
    }
    Ok(Json(s.db.list_chat_asks(q.status.as_deref())?))
}

#[derive(Deserialize)]
struct AskAnswer {
    answer: String,
}

/// Answers an ask; once its whole turn is answered Rui gets one combined message in the chat.
async fn answer_ask(State(s): State<ApiState>, Path(id): Path<i64>, Json(b): Json<AskAnswer>) -> ApiResult<Json<rui_core::ChatAsk>> {
    use crate::asks::AskError;
    require_text(&b.answer, "answer")?;
    match crate::asks::answer(&s.db, &s.chat_tx, id, b.answer.trim()) {
        Ok(a) => Ok(Json(a)),
        Err(AskError::NotFound) => Err(not_found("ask not found")),
        Err(AskError::Answered) => Err(conflict("ask is already answered")),
        Err(AskError::Other(e)) => Err(e.into()),
    }
}

#[derive(Deserialize)]
struct AnswerBody {
    answer: String,
    #[serde(default)]
    why: Option<String>,
}

/// The answered question plus how to undo it before it takes effect.
#[derive(serde::Serialize)]
struct AnswerReply {
    #[serde(flatten)]
    question: Question,
    undo: Undo,
}

#[derive(serde::Serialize)]
struct Undo {
    event_id: i64,
    run_at: i64,
}

/// Stores the owner's answer and holds it for the undo window; the scheduler delivers it after.
async fn answer_question(
    State(s): State<ApiState>,
    Path(id): Path<i64>,
    Json(b): Json<AnswerBody>,
) -> ApiResult<Json<AnswerReply>> {
    require_text(&b.answer, "answer")?;
    let q = s.db.get_question(id)?.ok_or_else(|| not_found("question not found"))?;
    if q.status == QuestionStatus::Answered {
        return Err(conflict("question is already answered"));
    }
    let why = b.why.as_deref().map(str::trim).filter(|w| !w.is_empty());
    let now = crate::thread::now_secs();
    let held = crate::scheduler::answer_with_undo(&s.db, id, &b.answer, why, now)?;
    let undo = Undo { event_id: held.event_id, run_at: held.run_at };
    Ok(Json(AnswerReply { question: held.question, undo }))
}

/// Undoes a held owner answer inside its window and returns the reopened question.
async fn cancel_approval(State(s): State<ApiState>, Path(event_id): Path<i64>) -> ApiResult<Json<Question>> {
    let now = crate::thread::now_secs();
    match crate::scheduler::cancel_held(&s.db, event_id, now)? {
        Some(q) => Ok(Json(q)),
        None => Err(conflict("too late: the answer was already delivered or cancelled")),
    }
}

#[derive(Deserialize)]
struct TaskBody {
    client_id: i64,
    title: String,
    body: String,
}

async fn create_task(
    State(s): State<ApiState>,
    Json(b): Json<TaskBody>,
) -> ApiResult<(StatusCode, Json<Task>)> {
    require_text(&b.title, "title")?;
    if s.db.get_client(b.client_id)?.is_none() {
        return Err(not_found("client not found"));
    }
    let task = s.db.create_task(&NewTask {
        client_id: b.client_id,
        parent_id: None,
        title: b.title,
        body: b.body,
        status: TaskStatus::New,
        done_command: None,
        max_loops: 5,
        role: Role::Lead,
        source_kind: SourceKind::User,
        source_ref: None,
        agent: None,
        pod: vec![],
    })?;
    Ok((StatusCode::CREATED, Json(task)))
}

fn conflict(msg: &str) -> ApiError {
    ApiError(StatusCode::CONFLICT, msg.into())
}

/// Removes and aborts the running turn of a task, if there is one.
#[derive(Deserialize)]
struct InstructBody {
    text: String,
}

impl From<crate::control::ControlError> for ApiError {
    fn from(e: crate::control::ControlError) -> Self {
        use crate::control::ControlError as C;
        match e {
            C::NotFound(m) => not_found(&m),
            C::Conflict(m) => conflict(&m),
            C::Other(e) => e.into(),
        }
    }
}

/// A task, plus a note saying what was resumed so it could continue.
#[derive(serde::Serialize)]
struct InstructReply {
    #[serde(flatten)]
    task: Task,
    note: Option<String>,
}

/// Saves an owner instruction, aborts any run and puts the task back in the queue.
async fn instruct_task(
    State(s): State<ApiState>,
    Path(id): Path<i64>,
    Json(b): Json<InstructBody>,
) -> ApiResult<Json<InstructReply>> {
    require_text(&b.text, "text")?;
    let (task, note) = crate::control::instruct(&s.db, &s.runs, id, &b.text, "owner")?;
    Ok(Json(InstructReply { task, note }))
}

/// Optional body of a stop call: where it came from and why.
#[derive(Deserialize, Default)]
struct StopBody {
    source: Option<String>,
    reason: Option<String>,
}

/// Reads a stop body; an empty body means no source and no reason.
fn parse_stop_body(bytes: &[u8]) -> ApiResult<StopBody> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(StopBody::default());
    }
    serde_json::from_slice(bytes).map_err(|e| bad_request(&format!("invalid body: {e}")))
}

/// Stops a task and its unfinished subtasks, aborting their runs.
async fn stop_task(State(s): State<ApiState>, Path(id): Path<i64>, body: axum::body::Bytes) -> ApiResult<Json<serde_json::Value>> {
    let b = parse_stop_body(&body)?;
    let ids = crate::control::stop_tree(&s.db, &s.runs, id, "owner", b.source.as_deref(), b.reason.as_deref())?;
    Ok(Json(serde_json::json!({ "stopped": ids })))
}

/// Stops every top-level task that is not done or stuck, with its subtasks.
async fn stop_all(State(s): State<ApiState>, body: axum::body::Bytes) -> ApiResult<Json<serde_json::Value>> {
    let b = parse_stop_body(&body)?;
    let open: Vec<i64> = (s.db.list_tasks()?.iter())
        .filter(|t| t.parent_id.is_none() && !matches!(t.status, TaskStatus::Done | TaskStatus::Stuck))
        .map(|t| t.id)
        .collect();
    let mut stopped = Vec::new();
    for id in open {
        stopped.extend(crate::control::stop_tree(&s.db, &s.runs, id, "owner", b.source.as_deref(), b.reason.as_deref())?);
    }
    Ok(Json(serde_json::json!({ "stopped": stopped })))
}

#[derive(Deserialize)]
struct ReviewBody {
    verdict: String,
    text: Option<String>,
}

/// The owner's verdict on finished work: "ok", or "changes" with a text for the Lead.
async fn review_task(State(s): State<ApiState>, Path(id): Path<i64>, Json(b): Json<ReviewBody>) -> ApiResult<Json<Task>> {
    if !["ok", "changes"].contains(&b.verdict.as_str()) {
        return Err(bad_request("verdict must be ok or changes"));
    }
    Ok(Json(crate::control::review(&s.db, &s.runs, &s.data_dir, id, &b.verdict, b.text.as_deref())?))
}

/// Drops a task: stops it and its team for good.
async fn drop_task(State(s): State<ApiState>, Path(id): Path<i64>) -> ApiResult<Json<serde_json::Value>> {
    let ids = crate::control::drop_task(&s.db, &s.runs, id)?;
    Ok(Json(serde_json::json!({ "stopped": ids })))
}

/// Puts a stuck task back to work.
async fn retry_task(State(s): State<ApiState>, Path(id): Path<i64>) -> ApiResult<Json<serde_json::Value>> {
    let task = s.db.get_task(id)?.ok_or_else(|| not_found("task not found"))?;
    if task.status != TaskStatus::Stuck {
        return Err(conflict("task is not stuck"));
    }
    let ids = crate::scheduler::retry_stuck(&s.db, id, "owner")?;
    Ok(Json(serde_json::json!({ "resumed": ids })))
}

#[derive(Deserialize)]
struct CheckpointBody {
    on: bool,
}

/// Holds a subtask that has not started ("wait for me before this step"), or releases it.
async fn checkpoint_task(State(s): State<ApiState>, Path(id): Path<i64>, Json(b): Json<CheckpointBody>) -> ApiResult<Json<Task>> {
    let task = s.db.get_task(id)?.ok_or_else(|| not_found("task not found"))?;
    if task.parent_id.is_none() || !matches!(task.status, TaskStatus::New | TaskStatus::Open) {
        return Err(bad_request("only a subtask that has not started can be held"));
    }
    if crate::scheduler::owner_hold(&s.db, id)? == Some("takeover") {
        return Err(conflict("this step is taken over"));
    }
    let kind = if b.on { "checkpoint_set" } else { "checkpoint_cleared" };
    s.db.add_event(Some(id), "owner", kind, &serde_json::json!({}))?;
    Ok(Json(task))
}

/// Stops a subtask's agent so the owner can do the step by hand.
async fn take_over_task(State(s): State<ApiState>, Path(id): Path<i64>) -> ApiResult<Json<crate::control::TakeOver>> {
    Ok(Json(crate::control::take_over(&s.db, &s.runs, &s.data_dir, id)?))
}

#[derive(Deserialize)]
struct HandBackBody {
    note: String,
    #[serde(default)]
    force: bool,
}

/// Gives a taken-over step back: runs its check, then lands it or sends the note to the agent.
async fn hand_back_task(
    State(s): State<ApiState>,
    Path(id): Path<i64>,
    Json(b): Json<HandBackBody>,
) -> ApiResult<Json<crate::scheduler::HandBack>> {
    let task = s.db.get_task(id)?.ok_or_else(|| not_found("task not found"))?;
    if crate::scheduler::owner_hold(&s.db, id)? != Some("takeover") {
        return Err(conflict("this step is not taken over"));
    }
    if !crate::scheduler::subtask_worktree(&s.data_dir, id).exists() {
        return Err(conflict("this step has no worktree to check"));
    }
    Ok(Json(crate::scheduler::hand_back(&s.db, &s.data_dir, &task, &b.note, b.force).await?))
}

/// Files changed by a task: committed diff when done, else the live worktree diff.
async fn task_changes(State(s): State<ApiState>, Path(id): Path<i64>) -> ApiResult<Json<Vec<Change>>> {
    let task = s.db.get_task(id)?.ok_or_else(|| not_found("task not found"))?;
    let top = task.parent_id.is_none();
    let name = if top { format!("task-{id}") } else { id.to_string() };
    let dir = s.data_dir.join("worktrees").join(name);
    if !dir.exists() {
        return Ok(Json(branch_changes(&s.db, &task).await?));
    }
    let done = task.status == TaskStatus::Done;
    let list = tokio::task::spawn_blocking(move || {
        if top { worktree::changes_vs_base(&dir) } else { worktree::changes(&dir, done) }
    })
        .await
        .map_err(anyhow::Error::from)??;
    Ok(Json(list))
}

/// The task's branch: the one it recorded, else the name RUI gives it.
fn task_branch(task: &Task) -> String {
    let fallback = || match task.parent_id {
        None => worktree::task_branch_name(task.id),
        Some(_) => worktree::branch_name(task.id),
    };
    task.branch.clone().unwrap_or_else(fallback)
}

/// Changes read from the task's branch when its worktree is gone (never made, or cleaned up).
/// An unfinished subtask's work lived only in its worktree, so it has none to show.
async fn branch_changes(db: &Db, task: &Task) -> ApiResult<Vec<Change>> {
    let top = task.parent_id.is_none();
    if !top && task.status != TaskStatus::Done {
        return Ok(vec![]);
    }
    let Some(client) = db.get_client(task.client_id)? else { return Ok(vec![]) };
    let repo = crate::scheduler::expand_home(&client.repo_path);
    let branch = task_branch(task);
    let list = tokio::task::spawn_blocking(move || worktree::branch_changes(&repo, &branch, top))
        .await
        .map_err(anyhow::Error::from)??;
    Ok(list)
}

/// True once the sweep removed one of the task's worktrees.
fn cleaned_up(db: &Db, id: i64) -> ApiResult<bool> {
    Ok(!db.events_for_tasks(&[id], &["worktree_removed"], 0, 1)?.is_empty())
}

/// Removes the worktrees RUI no longer needs, now, and says which were removed or kept.
async fn clean_worktrees(State(s): State<ApiState>) -> ApiResult<Json<crate::sweep::Sweep>> {
    let running = s.runs.lock().map(|r| r.keys().copied().collect()).unwrap_or_default();
    let (db, dir) = (s.db.clone(), s.data_dir.clone());
    let now = crate::thread::now_secs();
    let out = tokio::task::spawn_blocking(move || crate::sweep::sweep(&db, &dir, &running, now))
        .await
        .map_err(anyhow::Error::from)??;
    Ok(Json(out))
}

const MAX_FILE_BYTES: u64 = 256 * 1024;

#[derive(Deserialize)]
struct FileQuery {
    path: String,
}

#[derive(serde::Serialize)]
struct FileView {
    path: String,
    content: String,
    truncated: bool,
}

fn forbidden(msg: &str) -> ApiError {
    ApiError(StatusCode::FORBIDDEN, msg.into())
}

/// Reads up to 256 KB of a file inside the worktree; 403 when the path escapes it.
fn read_worktree_file(dir: &std::path::Path, rel: &str) -> ApiResult<FileView> {
    use std::io::Read;
    if std::path::Path::new(rel).is_absolute() {
        return Err(forbidden("path must be relative"));
    }
    let root = dir.canonicalize().map_err(|_| not_found("worktree not found"))?;
    let full = root.join(rel).canonicalize().map_err(|_| not_found("file not found"))?;
    if !full.starts_with(&root) {
        return Err(forbidden("path is outside the worktree"));
    }
    if !full.is_file() {
        return Err(not_found("file not found"));
    }
    let file = std::fs::File::open(&full).map_err(|_| not_found("file not found"))?;
    let mut buf = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut buf).map_err(anyhow::Error::from)?;
    let truncated = buf.len() as u64 > MAX_FILE_BYTES;
    buf.truncate(MAX_FILE_BYTES as usize);
    let content = String::from_utf8_lossy(&buf).into_owned();
    Ok(FileView { path: rel.to_string(), content, truncated })
}

/// Contents of one file in a task's worktree (read-only, capped at 256 KB).
async fn task_file(
    State(s): State<ApiState>,
    Path(id): Path<i64>,
    Query(q): Query<FileQuery>,
) -> ApiResult<Json<FileView>> {
    let task = s.db.get_task(id)?.ok_or_else(|| not_found("task not found"))?;
    let dir = s.data_dir.join("worktrees").join(id.to_string());
    if !dir.exists() && cleaned_up(&s.db, id)? {
        let msg = format!("This task's files were cleaned up; its branch {} still has them", task_branch(&task));
        return Err(not_found(&msg));
    }
    let view = tokio::task::spawn_blocking(move || read_worktree_file(&dir, &q.path))
        .await
        .map_err(anyhow::Error::from)??;
    Ok(Json(view))
}

/// Files the task and its team produced, as found in the project repo.
async fn task_results(State(s): State<ApiState>, Path(id): Path<i64>) -> ApiResult<Json<Vec<ResultFile>>> {
    let task = s.db.get_task(id)?.ok_or_else(|| not_found("task not found"))?;
    let store = s.db.clone();
    let list = tokio::task::spawn_blocking(move || crate::results::collect(&store, &task))
        .await
        .map_err(anyhow::Error::from)??;
    Ok(Json(list))
}

/// Contents of one produced file in the project repo (read-only, capped at 256 KB).
async fn result_file(
    State(s): State<ApiState>,
    Path(id): Path<i64>,
    Query(q): Query<FileQuery>,
) -> ApiResult<Json<FileView>> {
    let task = s.db.get_task(id)?.ok_or_else(|| not_found("task not found"))?;
    let client = s.db.get_client(task.client_id)?.ok_or_else(|| not_found("project not found"))?;
    let dir = crate::scheduler::expand_home(&client.repo_path);
    let view = tokio::task::spawn_blocking(move || read_worktree_file(&dir, &q.path))
        .await
        .map_err(anyhow::Error::from)??;
    Ok(Json(view))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use tokio::sync::mpsc::UnboundedReceiver;
    use tower::ServiceExt;

    fn setup() -> (Router, Arc<Db>, UnboundedReceiver<i64>) {
        let db = Arc::new(Db::open_test());
        let (chat_tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (router(ApiState { db: db.clone(), chat_tx, seat: SeatCache::default(), runs: Runs::default(), live: Live::default(), data_dir: std::env::temp_dir().join("rui-api-test-none"), apps: Apps::default(), owner_token: crate::owner_auth::test_token() }), db, rx)
    }

    async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(uri).header(crate::owner_auth::HEADER, "test-owner-token");
        let body = match body {
            Some(b) => {
                req = req.header("content-type", "application/json");
                Body::from(b.to_string())
            }
            None => Body::empty(),
        };
        let resp = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    fn seed_task(db: &Db) -> (i64, i64) {
        let c = db.create_client("acme", "/repo", None, &[]).unwrap();
        let t = db
            .create_task(&NewTask {
                client_id: c.id,
                parent_id: None,
                title: "t".into(),
                body: String::new(),
                status: TaskStatus::New,
                done_command: None,
                max_loops: 5,
                role: Role::Lead,
                source_kind: SourceKind::User,
                source_ref: None,
                agent: None,
                pod: vec![],
            })
            .unwrap();
        (c.id, t.id)
    }

    async fn status_with(app: &Router, uri: &str, token: Option<&str>) -> StatusCode {
        let mut req = Request::builder().method("GET").uri(uri);
        if let Some(t) = token {
            req = req.header(crate::owner_auth::HEADER, t);
        }
        app.clone().oneshot(req.body(Body::empty()).unwrap()).await.unwrap().status()
    }

    #[tokio::test]
    async fn owner_routes_need_the_token_but_mcp_uses_run_tokens() {
        let (app, db, _rx) = setup();
        let app = app.merge(crate::mcp::mcp_router(db, std::env::temp_dir(), Apps::default(), Runs::default()));
        assert_eq!(status_with(&app, "/tasks", None).await, StatusCode::UNAUTHORIZED);
        assert_eq!(status_with(&app, "/tasks", Some("wrong")).await, StatusCode::UNAUTHORIZED);
        assert_eq!(status_with(&app, "/skills", None).await, StatusCode::UNAUTHORIZED);
        assert_eq!(status_with(&app, "/tasks", Some("test-owner-token")).await, StatusCode::OK);
        let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#;
        let mcp = |token: Option<&str>| {
            let mut req = Request::post("/mcp/rui").header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream").header("host", "localhost");
            if let Some(t) = token {
                req = req.header(crate::run_tokens::HEADER, t);
            }
            req.body(Body::from(init)).unwrap()
        };
        assert_eq!(app.clone().oneshot(mcp(Some("test-owner-token"))).await.unwrap().status(), StatusCode::UNAUTHORIZED);
        let grant = crate::run_tokens::RunGrant::new(crate::run_tokens::Server::Rui, None);
        let run = crate::run_tokens::global().issue(grant).unwrap();
        assert_eq!(app.oneshot(mcp(Some(run.value()))).await.unwrap().status(), StatusCode::OK);
    }

    fn seed_task_for(db: &Db, client_id: i64) -> i64 {
        seed_child(db, client_id, None)
    }

    fn seed_child(db: &Db, client_id: i64, parent_id: Option<i64>) -> i64 {
        db.create_task(&NewTask {
            client_id,
            parent_id,
            title: "t".into(),
            body: String::new(),
            status: TaskStatus::New,
            done_command: None,
            max_loops: 5,
            role: Role::Lead,
            source_kind: SourceKind::User,
            source_ref: None,
            agent: None,
            pod: vec![],
        })
        .unwrap()
        .id
    }

    #[tokio::test]
    async fn apps_list_empty_and_restart_unknown_404() {
        let (app, _db, _rx) = setup();
        let (code, v) = call(&app, "GET", "/apps", None).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(v, serde_json::json!([]));
        let (code, _) = call(&app, "POST", "/apps/nope/restart", None).await;
        assert_eq!(code, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn skills_lists_data_dir_skills() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("skills/demo");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "---\nname: demo\ndescription: d\nbash: [\"a *\"]\n---\nBody\n").unwrap();
        let (app, _tid) = setup_with_dir(dir.path());
        let (code, v) = call(&app, "GET", "/skills", None).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(v[0]["name"], "demo");
        assert_eq!(v[0]["bash"], json!(["a *"]));
        assert_eq!(v[0]["body"], "Body");
    }

    #[tokio::test]
    async fn roles_lists_data_dir_roles_without_body() {
        let dir = tempfile::tempdir().unwrap();
        let role_dir = dir.path().join("roles/backend");
        std::fs::create_dir_all(&role_dir).unwrap();
        let text = "---\nname: backend\ncode: BE\ntitle: Backend Engineer\ndescription: Builds APIs\ntools: [\"Read\"]\nskills: [\"fastapi\"]\nmax_loops: 6\n---\nYou build APIs.\n";
        std::fs::write(role_dir.join("AGENT.md"), text).unwrap();
        let (app, _tid) = setup_with_dir(dir.path());
        let (code, v) = call(&app, "GET", "/roles", None).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!((v[0]["code"].as_str(), v[0]["title"].as_str()), (Some("BE"), Some("Backend Engineer")));
        assert_eq!((v[0]["skills"].clone(), v[0]["max_loops"].as_i64()), (json!(["fastapi"]), Some(6)));
        assert!(v[0].get("body").is_none());
    }

    #[tokio::test]
    async fn get_clients_and_tasks() {
        let (app, db, _rx) = setup();
        seed_task(&db);
        let (st, v) = call(&app, "GET", "/clients", None).await;
        assert_eq!((st, v[0]["name"].as_str()), (StatusCode::OK, Some("acme")));
        let (st, v) = call(&app, "GET", "/tasks", None).await;
        assert_eq!((st, v[0]["status"].as_str()), (StatusCode::OK, Some("new")));
    }

    #[tokio::test]
    async fn post_clients_validates_and_conflicts() {
        let (app, _db, _rx) = setup();
        let repo = tempfile::tempdir().unwrap();
        let ok = std::process::Command::new("git").arg("-C").arg(repo.path()).arg("init").output().unwrap();
        assert!(ok.status.success());
        let body = json!({"name": "newco", "repo_path": repo.path().to_string_lossy()});
        let (st, v) = call(&app, "POST", "/clients", Some(body.clone())).await;
        assert_eq!((st, v["name"].as_str()), (StatusCode::CREATED, Some("newco")));
        let (_, list) = call(&app, "GET", "/clients", None).await;
        assert_eq!(list[0]["name"].as_str(), Some("newco"));
        assert_eq!(call(&app, "POST", "/clients", Some(body)).await.0, StatusCode::CONFLICT);
        let plain = tempfile::tempdir().unwrap();
        let bad = json!({"name": "other", "repo_path": plain.path().to_string_lossy()});
        assert_eq!(call(&app, "POST", "/clients", Some(bad)).await.0, StatusCode::BAD_REQUEST);
        let empty = json!({"name": " ", "repo_path": repo.path().to_string_lossy()});
        assert_eq!(call(&app, "POST", "/clients", Some(empty)).await.0, StatusCode::BAD_REQUEST);
    }

    fn git_repo() -> tempfile::TempDir {
        let repo = tempfile::tempdir().unwrap();
        let ok = std::process::Command::new("git").arg("-C").arg(repo.path()).arg("init").output().unwrap();
        assert!(ok.status.success());
        repo
    }

    #[tokio::test]
    async fn patch_client_renames_and_validates() {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::open_test());
        let (chat_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let state = ApiState { db: db.clone(), chat_tx, seat: SeatCache::default(), runs: Runs::default(), live: Live::default(), data_dir: dir.path().to_path_buf(), apps: Apps::default(), owner_token: crate::owner_auth::test_token() };
        let app = router(state);
        let repo = git_repo();
        let other = db.create_client("taken", "/repo", None, &[]).unwrap();
        let body = json!({"name": "newco", "repo_path": repo.path().to_string_lossy()});
        let (_, c) = call(&app, "POST", "/clients", Some(body)).await;
        let id = c["id"].as_i64().unwrap();
        let url = format!("/clients/{id}");
        let patch = json!({"name": "renamed", "hosts": ["a.example.com", " a.example.com ", "b.example.com"]});
        let (st, v) = call(&app, "PATCH", &url, Some(patch)).await;
        assert_eq!((st, v["name"].as_str()), (StatusCode::OK, Some("renamed")));
        assert_eq!(v["hosts"], json!(["a.example.com", "b.example.com"]));
        let (_, got) = call(&app, "GET", &url, None).await;
        assert_eq!(got["hosts"], json!(["a.example.com", "b.example.com"]));
        let dup = json!({"name": "taken"});
        assert_eq!(call(&app, "PATCH", &url, Some(dup)).await.0, StatusCode::CONFLICT);
        let bad = json!({"repo_path": "/no/such/folder/here"});
        assert_eq!(call(&app, "PATCH", &url, Some(bad)).await.0, StatusCode::BAD_REQUEST);
        let empty = json!({"name": " "});
        assert_eq!(call(&app, "PATCH", &url, Some(empty)).await.0, StatusCode::BAD_REQUEST);
        let missing = json!({"name": "x"});
        assert_eq!(call(&app, "PATCH", "/clients/9999", Some(missing)).await.0, StatusCode::NOT_FOUND);
        assert!(db.get_client(other.id).unwrap().is_some());
    }

    #[tokio::test]
    async fn delete_client_cascades_and_404s() {
        let (app, db, _rx) = setup();
        let (client, task) = seed_task(&db);
        let (other, keep) = {
            let o = db.create_client("keeper", "/repo", None, &[]).unwrap();
            (o.id, seed_task_for(&db, o.id))
        };
        let (st, _) = call(&app, "DELETE", &format!("/clients/{client}"), None).await;
        assert_eq!(st, StatusCode::OK);
        assert!(db.get_client(client).unwrap().is_none());
        assert!(db.get_task(task).unwrap().is_none());
        assert!(db.get_client(other).unwrap().is_some());
        assert!(db.get_task(keep).unwrap().is_some());
        assert_eq!(call(&app, "DELETE", &format!("/clients/{client}"), None).await.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn owner_list_hides_lower_levels_and_answer_flows_to_member() {
        let (app, db, _rx) = setup();
        let (client, parent) = seed_task(&db);
        let member = db
            .create_task(&NewTask {
                client_id: client,
                parent_id: Some(parent),
                title: "m".into(),
                body: String::new(),
                status: TaskStatus::WaitingAnswer,
                done_command: Some("true".into()),
                max_loops: 5,
                role: Role::Builder,
                source_kind: SourceKind::User,
                source_ref: None,
                agent: Some("backend".into()),
                pod: vec![],
            })
            .unwrap();
        let ask = |level: &str| db.create_ladder_question(member.id, Role::Builder, level, &[], "q").unwrap();
        let (lead_q, owner_q) = (ask("lead"), ask("owner"));
        let (_, v) = call(&app, "GET", "/questions?status=open", None).await;
        let ids: Vec<i64> = v.as_array().unwrap().iter().map(|q| q["id"].as_i64().unwrap()).collect();
        assert_eq!(ids, vec![owner_q.id]);
        let uri = format!("/questions/{}/answer", owner_q.id);
        let (st, v) = call(&app, "POST", &uri, Some(json!({"answer": "go", "why": "scope"}))).await;
        assert_eq!((st, v["why"].as_str()), (StatusCode::OK, Some("scope")));
        assert_eq!(db.get_task(member.id).unwrap().unwrap().status, TaskStatus::WaitingAnswer);
        assert!(db.pending_instructions(member.id).unwrap().is_empty());
        let event_id = v["undo"]["event_id"].as_i64().unwrap();
        let data = tempfile::tempdir().unwrap();
        crate::scheduler::deliver_held(&db, data.path(), event_id).unwrap();
        assert_eq!(db.get_task(member.id).unwrap().unwrap().status, TaskStatus::Open);
        assert_eq!(db.pending_instructions(member.id).unwrap().len(), 1);
        let cancel = format!("/approvals/{event_id}/cancel");
        assert_eq!(call(&app, "POST", &cancel, None).await.0, StatusCode::CONFLICT);
        let (st, _) = call(&app, "POST", &uri, Some(json!({"answer": "again"}))).await;
        assert_eq!(st, StatusCode::CONFLICT);
        db.set_question_level(lead_q.id, "owner", &[]).unwrap();
        let (_, v) = call(&app, "GET", "/questions?status=open", None).await;
        assert_eq!(v.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn questions_filter_and_answer() {
        let (app, db, _rx) = setup();
        let (_, tid) = seed_task(&db);
        let q = db.create_question(tid, Role::Lead, "why?").unwrap();
        let (_, v) = call(&app, "GET", "/questions?status=open", None).await;
        assert_eq!(v.as_array().unwrap().len(), 1);
        let uri = format!("/questions/{}/answer", q.id);
        let (st, v) = call(&app, "POST", &uri, Some(json!({"answer": "because"}))).await;
        assert_eq!((st, v["status"].as_str()), (StatusCode::OK, Some("answered")));
        let (_, v) = call(&app, "GET", "/questions?status=open", None).await;
        assert!(v.as_array().unwrap().is_empty());
        let (st, _) = call(&app, "GET", "/questions?status=bogus", None).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn answer_errors() {
        let (app, _db, _rx) = setup();
        let (st, _) = call(&app, "POST", "/questions/42/answer", Some(json!({"answer": "x"}))).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        let (st, v) = call(&app, "POST", "/questions/42/answer", Some(json!({"answer": " "}))).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert!(v["error"].is_string());
    }

    #[tokio::test]
    async fn chat_post_notifies_and_lists() {
        let (app, db, mut rx) = setup();
        let cid = db.create_chat().unwrap().id;
        let (st, v) = call(&app, "POST", "/chat", Some(json!({"chat_id": cid, "content": "hi"}))).await;
        assert_eq!((st, v["role"].as_str()), (StatusCode::CREATED, Some("user")));
        assert_eq!(rx.try_recv().unwrap(), v["id"].as_i64().unwrap());
        let (_, v) = call(&app, "GET", &format!("/chat?chat_id={cid}&limit=10"), None).await;
        assert_eq!(v[0]["content"].as_str(), Some("hi"));
        assert_eq!(v[0]["chat_id"].as_i64(), Some(cid));
        let (st, _) = call(&app, "POST", "/chat", Some(json!({"chat_id": cid, "content": ""}))).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn chats_delete_removes_chat_and_404s_after() {
        let (app, _db, _rx) = setup();
        let (_, c) = call(&app, "POST", "/chats", None).await;
        let cid = c["id"].as_i64().unwrap();
        call(&app, "POST", "/chat", Some(json!({"chat_id": cid, "content": "hi"}))).await;
        assert_eq!(call(&app, "DELETE", &format!("/chats/{cid}"), None).await.0, StatusCode::NO_CONTENT);
        let (_, list) = call(&app, "GET", "/chats", None).await;
        assert!(list.as_array().unwrap().is_empty());
        assert_eq!(call(&app, "DELETE", &format!("/chats/{cid}"), None).await.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn chats_delete_conflicts_while_rui_is_answering() {
        let (app, db, _rx) = setup();
        let cid = db.create_chat().unwrap().id;
        let (chat_tx, _rx2) = tokio::sync::mpsc::unbounded_channel();
        let live = Live::default();
        live.lock().unwrap().insert(crate::live::LiveKey::Chat(cid), Default::default());
        let state = ApiState { db: db.clone(), chat_tx, seat: SeatCache::default(), runs: Runs::default(), live, data_dir: std::env::temp_dir(), apps: Apps::default(), owner_token: crate::owner_auth::test_token() };
        let busy_app = router(state);
        assert_eq!(call(&busy_app, "DELETE", &format!("/chats/{cid}"), None).await.0, StatusCode::CONFLICT);
        assert!(db.get_chat(cid).unwrap().is_some());
        let _ = app;
    }

    #[tokio::test]
    async fn chat_errors_for_missing_or_unknown_chat() {
        let (app, _db, mut rx) = setup();
        assert_eq!(call(&app, "GET", "/chat", None).await.0, StatusCode::BAD_REQUEST);
        assert_eq!(call(&app, "GET", "/chat?chat_id=99", None).await.0, StatusCode::NOT_FOUND);
        let body = json!({"chat_id": 99, "content": "hi"});
        assert_eq!(call(&app, "POST", "/chat", Some(body)).await.0, StatusCode::NOT_FOUND);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn greeting_covers_each_state() {
        assert_eq!(greeting(8, 0, 0, 0), "Good morning. All quiet. What do you want to set up?");
        assert_eq!(greeting(12, 2, 0, 3), "Good afternoon. 2 threads are in the loop. What do you want to set up?");
        assert_eq!(greeting(17, 1, 0, 1), "Good afternoon. 1 thread is in the loop. What do you want to set up?");
        assert_eq!(greeting(18, 0, 0, 1), "Good evening. 1 thread is open, none running. What do you want to set up?");
        assert_eq!(greeting(23, 0, 0, 4), "Good evening. 4 threads are open, none running. What do you want to set up?");
        let ask = "Want to start with those, or set up something new?";
        assert_eq!(greeting(9, 3, 1, 3), format!("Good morning. 1 thing waits for you in the Inbox. {ask}"));
        assert_eq!(greeting(9, 0, 2, 0), format!("Good morning. 2 things wait for you in the Inbox. {ask}"));
    }

    #[tokio::test]
    async fn chats_create_list_and_title_from_first_message() {
        let (app, _db, _rx) = setup();
        let (st, c) = call(&app, "POST", "/chats", None).await;
        assert_eq!(st, StatusCode::CREATED);
        assert_eq!(c["title"].as_str(), Some("New chat"));
        let last = c["last"].as_str().unwrap();
        assert!(last.starts_with("Good ") && last.ends_with('?'), "{last}");
        let cid = c["id"].as_i64().unwrap();
        let long = "x".repeat(60);
        call(&app, "POST", "/chat", Some(json!({"chat_id": cid, "content": long}))).await;
        let (_, list) = call(&app, "GET", "/chats", None).await;
        assert_eq!(list.as_array().unwrap().len(), 1);
        assert_eq!(list[0]["title"].as_str(), Some(format!("{}…", "x".repeat(44)).as_str()));
        let (_, msgs) = call(&app, "GET", &format!("/chat?chat_id={cid}"), None).await;
        assert_eq!(msgs.as_array().unwrap().len(), 2);
        assert_eq!(msgs[0]["role"].as_str(), Some("rui"));
    }

    #[tokio::test]
    async fn chat_greeting_uses_the_inbox_count_the_app_sends() {
        let (app, db, _rx) = setup();
        let (_, tid) = seed_task(&db);
        db.create_ladder_question(tid, Role::Lead, "owner", &[], "Which provider?").unwrap();
        let (_, c) = call(&app, "POST", "/chats", Some(json!({"needs_you": 0}))).await;
        assert!(!c["last"].as_str().unwrap().contains("Inbox"), "{}", c["last"]);
        let (_, c) = call(&app, "POST", "/chats", None).await;
        assert!(c["last"].as_str().unwrap().contains("1 thing waits for you in the Inbox"), "{}", c["last"]);
    }

    #[tokio::test]
    async fn events_filter() {
        let (app, db, _rx) = setup();
        let (_, tid) = seed_task(&db);
        db.add_event(Some(tid), "lead", "a", &json!({})).unwrap();
        db.add_event(None, "rui", "b", &json!({})).unwrap();
        let (_, v) = call(&app, "GET", "/events", None).await;
        assert_eq!(v.as_array().unwrap().len(), 2);
        let (_, v) = call(&app, "GET", &format!("/events?task_id={tid}&limit=5"), None).await;
        assert_eq!(v.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn seat_returns_cache_and_last_model() {
        let (app, db, _rx) = setup();
        let (_, v) = call(&app, "GET", "/seat", None).await;
        assert!(v["last_model"].is_null());
        db.add_event(None, "lead", "init", &json!({"model": "claude-old"})).unwrap();
        db.add_event(None, "lead", "init", &json!({"session_id": "x"})).unwrap();
        db.add_event(None, "lead", "init", &json!({"model": "claude-new"})).unwrap();
        let (st, v) = call(&app, "GET", "/seat", None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["last_model"].as_str(), Some("claude-new"));
        assert_eq!(v["logged_in"].as_bool(), Some(false));
    }

    #[tokio::test]
    async fn task_runs_newest_init_per_task() {
        let (app, db, _rx) = setup();
        let (cid, t1) = seed_task(&db);
        let t2 = seed_task_for(&db, cid);
        let t3 = seed_task_for(&db, cid);
        let _t4 = seed_task_for(&db, cid);
        db.add_event(Some(t1), "lead", "init", &json!({"model": "old", "apiKeySource": "ANTHROPIC_API_KEY"})).unwrap();
        db.add_event(Some(t1), "builder", "init", &json!({"model": "m1", "apiKeySource": "none"})).unwrap();
        db.add_event(Some(t2), "lead", "init", &json!({"model": "m2", "apiKeySource": "env"})).unwrap();
        db.add_event(Some(t3), "lead", "init", &json!({"session_id": "s"})).unwrap();
        db.add_event(Some(t3), "lead", "assistant", &json!({})).unwrap();
        let (st, v) = call(&app, "GET", "/tasks/runs", None).await;
        assert_eq!(st, StatusCode::OK);
        let runs = v.as_array().unwrap();
        assert_eq!(runs.len(), 3);
        assert_eq!((runs[0]["agent"].as_str(), runs[0]["model"].as_str()), (Some("builder"), Some("m1")));
        assert_eq!(runs[0]["seat"].as_str(), Some("subscription"));
        assert_eq!(runs[1]["seat"].as_str(), Some("api_key"));
        assert_eq!(runs[2]["seat"].as_str(), Some("unknown"));
        assert!(runs[2]["model"].is_null());
    }

    #[tokio::test]
    async fn events_after_id_oldest_first_with_limit() {
        let (app, db, _rx) = setup();
        let ids: Vec<i64> = (0..5).map(|i| db.add_event(None, "rui", "k", &json!({"i": i})).unwrap()).collect();
        let (_, v) = call(&app, "GET", &format!("/events?after_id={}", ids[1]), None).await;
        let got: Vec<i64> = v.as_array().unwrap().iter().map(|e| e["id"].as_i64().unwrap()).collect();
        assert_eq!(got, ids[2..]);
        let (_, v) = call(&app, "GET", &format!("/events?after_id={}&limit=2", ids[0]), None).await;
        let got: Vec<i64> = v.as_array().unwrap().iter().map(|e| e["id"].as_i64().unwrap()).collect();
        assert_eq!(got, ids[1..3]);
    }

    #[tokio::test]
    async fn create_task_cases() {
        let (app, db, _rx) = setup();
        let (cid, _) = seed_task(&db);
        let ok = json!({"client_id": cid, "title": "new", "body": "b"});
        let (st, v) = call(&app, "POST", "/tasks", Some(ok)).await;
        assert_eq!(st, StatusCode::CREATED);
        assert_eq!((v["role"].as_str(), v["max_loops"].as_i64()), (Some("lead"), Some(5)));
        assert_eq!(v["source_kind"].as_str(), Some("user"));
        let missing = json!({"client_id": 999, "title": "x", "body": ""});
        assert_eq!(call(&app, "POST", "/tasks", Some(missing)).await.0, StatusCode::NOT_FOUND);
        let empty = json!({"client_id": cid, "title": "", "body": ""});
        assert_eq!(call(&app, "POST", "/tasks", Some(empty)).await.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn stop_open_task_marks_stuck_and_logs() {
        let (app, db, _rx) = setup();
        let (_, tid) = seed_task(&db);
        db.set_task_status(tid, TaskStatus::InProgress).unwrap();
        let (st, v) = call(&app, "POST", &format!("/tasks/{tid}/stop"), None).await;
        assert_eq!((st, v["stopped"].clone()), (StatusCode::OK, json!([tid])));
        assert_eq!(db.get_task(tid).unwrap().unwrap().status, TaskStatus::Stuck);
        let events = db.list_events(Some(tid), 10).unwrap();
        assert!(events.iter().any(|e| e.kind == "stopped" && e.agent == "owner"));
    }

    #[tokio::test]
    async fn stop_takes_optional_source_and_reason() {
        let (app, db, _rx) = setup();
        let (_, a) = seed_task(&db);
        let uri = format!("/tasks/{a}/stop");
        let body = Some(json!({"source": "Task page", "reason": "wrong plan"}));
        assert_eq!(call(&app, "POST", &uri, body).await.0, StatusCode::OK);
        let (_, v) = call(&app, "GET", "/tasks/state", None).await;
        assert_eq!(v[0]["reason"], "Stopped by you from Task page: wrong plan");
        let (_, v) = call(&app, "GET", "/tasks/stopped", None).await;
        assert_eq!(v, json!([a]));
        db.set_task_status(a, TaskStatus::Open).unwrap();
        assert_eq!(call(&app, "POST", &uri, Some(json!({}))).await.0, StatusCode::OK);
        db.set_task_status(a, TaskStatus::Open).unwrap();
        assert_eq!(call(&app, "POST", &uri, None).await.0, StatusCode::OK);
        let stops = db.events_of_kinds(&["stopped"]).unwrap();
        assert_eq!((stops.len(), stops[2].payload["source"].clone()), (3, json!(null)));
    }

    #[tokio::test]
    async fn stop_all_accepts_body_and_empty_body() {
        let (app, db, _rx) = setup();
        let (_, a) = seed_task(&db);
        let (st, _) = call(&app, "POST", "/stop-all", Some(json!({"source": "Work stop all"}))).await;
        assert_eq!(st, StatusCode::OK);
        let e = db.list_events(Some(a), 10).unwrap().into_iter().find(|e| e.kind == "stopped").unwrap();
        assert_eq!(e.payload["source"], "Work stop all");
        assert_eq!(call(&app, "POST", "/stop-all", None).await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn retry_drop_and_review_routes() {
        let (app, db, _rx) = setup();
        let (_, tid) = seed_task(&db);
        assert_eq!(call(&app, "POST", &format!("/tasks/{tid}/retry"), None).await.0, StatusCode::CONFLICT);
        db.set_task_status(tid, TaskStatus::Stuck).unwrap();
        let (st, v) = call(&app, "POST", &format!("/tasks/{tid}/retry"), None).await;
        assert_eq!((st, v["resumed"].clone()), (StatusCode::OK, json!([tid])));
        assert_eq!(db.get_task(tid).unwrap().unwrap().status, TaskStatus::New);
        let (st, _) = call(&app, "POST", &format!("/tasks/{tid}/drop"), None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(db.events_of_kinds(&["dropped"]).unwrap().len(), 1);
        let uri = format!("/tasks/{tid}/review");
        let review = |v: Value| call(&app, "POST", &uri, Some(v));
        assert_eq!(review(json!({"verdict": "ok"})).await.0, StatusCode::CONFLICT);
        assert_eq!(review(json!({"verdict": "maybe"})).await.0, StatusCode::BAD_REQUEST);
        db.set_task_status(tid, TaskStatus::Done).unwrap();
        assert_eq!(review(json!({"verdict": "changes"})).await.0, StatusCode::CONFLICT);
        assert_eq!(review(json!({"verdict": "changes", "text": "shorter"})).await.0, StatusCode::OK);
        assert_eq!(db.get_task(tid).unwrap().unwrap().status, TaskStatus::New);
        db.set_task_status(tid, TaskStatus::Done).unwrap();
        assert_eq!(review(json!({"verdict": "ok"})).await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn checkpoint_take_over_and_hand_back_guards() {
        let (app, db, _rx) = setup();
        let (cid, parent) = seed_task(&db);
        let kid = seed_child(&db, cid, Some(parent));
        let cp_uri = format!("/tasks/{kid}/checkpoint");
        let cp = |on: bool| call(&app, "POST", &cp_uri, Some(json!({"on": on})));
        assert_eq!(cp(true).await.0, StatusCode::OK);
        assert_eq!(crate::scheduler::owner_hold(&db, kid).unwrap(), Some("checkpoint"));
        assert_eq!(cp(false).await.0, StatusCode::OK);
        assert_eq!(crate::scheduler::owner_hold(&db, kid).unwrap(), None);
        let top = call(&app, "POST", &format!("/tasks/{parent}/checkpoint"), Some(json!({"on": true}))).await.0;
        assert_eq!(top, StatusCode::BAD_REQUEST);
        let hb_uri = format!("/tasks/{kid}/hand-back");
        assert_eq!(call(&app, "POST", &hb_uri, Some(json!({"note": "x"}))).await.0, StatusCode::CONFLICT);
        db.set_task_status(kid, TaskStatus::InProgress).unwrap();
        let (st, v) = call(&app, "POST", &format!("/tasks/{kid}/take-over"), None).await;
        assert_eq!((st, v["branch"].clone(), v["worktree"].clone()), (StatusCode::OK, json!(format!("rui/{kid}")), Value::Null));
        assert_eq!(db.get_task(kid).unwrap().unwrap().status, TaskStatus::Open);
        assert_eq!(crate::scheduler::owner_hold(&db, kid).unwrap(), Some("takeover"));
        db.set_task_status(kid, TaskStatus::Done).unwrap();
        assert_eq!(cp(true).await.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn inbox_lists_stuck_task_and_instruct_reply_has_note() {
        let (app, db, _rx) = setup();
        let (_, tid) = seed_task(&db);
        db.set_task_status(tid, TaskStatus::Stuck).unwrap();
        db.add_event(Some(tid), "scheduler", "agent_error", &json!({"error": "boom"})).unwrap();
        let (st, v) = call(&app, "GET", "/inbox", None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!((v[0]["kind"].clone(), v[0]["id"].clone(), v[0]["reason"].clone()), (json!("stuck"), json!(tid), json!("Failed: boom")));
        let (_, v) = call(&app, "POST", &format!("/tasks/{tid}/instruct"), Some(json!({"text": "go"}))).await;
        assert!(v["note"].is_null() && v["id"] == tid);
    }

    #[tokio::test]
    async fn stop_parent_stops_running_children() {
        let (app, db, _rx) = setup();
        let (cid, parent) = seed_task(&db);
        let kid = seed_child(&db, cid, Some(parent));
        db.set_task_status(kid, TaskStatus::InProgress).unwrap();
        let (st, v) = call(&app, "POST", &format!("/tasks/{parent}/stop"), None).await;
        assert_eq!((st, v["stopped"].clone()), (StatusCode::OK, json!([parent, kid])));
        assert_eq!(db.get_task(kid).unwrap().unwrap().status, TaskStatus::Stuck);
    }

    #[tokio::test]
    async fn stop_all_stops_every_open_team() {
        let (app, db, _rx) = setup();
        let (cid, a) = seed_task(&db);
        let ka = seed_child(&db, cid, Some(a));
        let b = seed_task_for(&db, cid);
        let done = seed_task_for(&db, cid);
        db.set_task_status(done, TaskStatus::Done).unwrap();
        let (st, v) = call(&app, "POST", "/stop-all", None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["stopped"], json!([a, ka, b]));
        assert_eq!(db.get_task(done).unwrap().unwrap().status, TaskStatus::Done);
        assert_eq!(db.get_task(b).unwrap().unwrap().status, TaskStatus::Stuck);
    }

    #[tokio::test]
    async fn stop_done_is_conflict_and_unknown_is_not_found() {
        let (app, db, _rx) = setup();
        let (_, tid) = seed_task(&db);
        db.set_task_status(tid, TaskStatus::Done).unwrap();
        assert_eq!(call(&app, "POST", &format!("/tasks/{tid}/stop"), None).await.0, StatusCode::CONFLICT);
        assert_eq!(call(&app, "POST", "/tasks/999/stop", None).await.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn instruct_stuck_builder_reopens_it_and_parent() {
        let (app, db, _rx) = setup();
        let (cid, parent) = seed_task(&db);
        db.set_task_status(parent, TaskStatus::Stuck).unwrap();
        let sub = db
            .create_task(&NewTask {
                client_id: cid,
                parent_id: Some(parent),
                title: "s".into(),
                body: String::new(),
                status: TaskStatus::Stuck,
                done_command: Some("true".into()),
                max_loops: 5,
                role: Role::Builder,
                source_kind: SourceKind::User,
                source_ref: None,
                agent: None,
                pod: vec![],
            })
            .unwrap();
        let uri = format!("/tasks/{}/instruct", sub.id);
        let (st, v) = call(&app, "POST", &uri, Some(json!({"text": "try x"}))).await;
        assert_eq!((st, v["status"].as_str()), (StatusCode::OK, Some("open")));
        assert_eq!(db.get_task(parent).unwrap().unwrap().status, TaskStatus::Split);
        let events = db.list_events(Some(sub.id), 10).unwrap();
        assert!(events.iter().any(|e| e.kind == "instruction" && e.agent == "owner"));
    }

    #[tokio::test]
    async fn stopped_lists_only_stopped_stuck_tasks() {
        let (app, db, _rx) = setup();
        let (cid, stopped) = seed_task(&db);
        let stuck = seed_task_for(&db, cid);
        db.set_task_status(stopped, TaskStatus::Stuck).unwrap();
        db.set_task_status(stuck, TaskStatus::Stuck).unwrap();
        db.add_event(Some(stopped), "owner", "stopped", &json!({"by": "owner"})).unwrap();
        let (st, v) = call(&app, "GET", "/tasks/stopped", None).await;
        assert_eq!((st, v), (StatusCode::OK, json!([stopped])));
    }

    #[tokio::test]
    async fn instruct_lead_goes_to_new() {
        let (app, db, _rx) = setup();
        let (_, tid) = seed_task(&db);
        db.set_task_status(tid, TaskStatus::Stuck).unwrap();
        let (st, v) = call(&app, "POST", &format!("/tasks/{tid}/instruct"), Some(json!({"text": "go"}))).await;
        assert_eq!((st, v["status"].as_str()), (StatusCode::OK, Some("new")));
    }

    #[tokio::test]
    async fn instruct_errors() {
        let (app, db, _rx) = setup();
        let (_, tid) = seed_task(&db);
        let uri = format!("/tasks/{tid}/instruct");
        db.set_task_status(tid, TaskStatus::WaitingAnswer).unwrap();
        assert_eq!(call(&app, "POST", &uri, Some(json!({"text": "x"}))).await.0, StatusCode::CONFLICT);
        assert_eq!(call(&app, "POST", &uri, Some(json!({"text": "  "}))).await.0, StatusCode::BAD_REQUEST);
        let missing = call(&app, "POST", "/tasks/999/instruct", Some(json!({"text": "x"}))).await;
        assert_eq!(missing.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn asks_list_answer_and_chat_reply() {
        let (app, db, mut rx) = setup();
        let chat = db.create_chat().unwrap();
        let msg = db.add_chat_message(chat.id, ChatRole::User, "plan my exam").unwrap();
        let a = db.create_chat_ask(chat.id, Some(msg.id), "When is it?", &["Thursday".into()]).unwrap();
        let (st, v) = call(&app, "GET", "/asks?status=open", None).await;
        assert_eq!((st, v[0]["text"].as_str(), v[0]["options"][0].as_str(), v[0]["chat_title"].as_str()), (StatusCode::OK, Some("When is it?"), Some("Thursday"), Some("plan my exam")));
        assert_eq!(call(&app, "GET", "/asks?status=weird", None).await.0, StatusCode::BAD_REQUEST);
        let uri = format!("/asks/{}/answer", a.id);
        assert_eq!(call(&app, "POST", &uri, Some(json!({"answer": " "}))).await.0, StatusCode::BAD_REQUEST);
        let (st, v) = call(&app, "POST", &uri, Some(json!({"answer": "Thursday"}))).await;
        assert_eq!((st, v["status"].as_str()), (StatusCode::OK, Some("answered")));
        let last = db.list_chat(chat.id, 5).unwrap().pop().unwrap();
        assert_eq!((last.role, last.content.as_str()), (ChatRole::User, "Re: When is it? — Thursday"));
        assert_eq!(rx.try_recv().unwrap(), last.id);
        assert_eq!(call(&app, "POST", &uri, Some(json!({"answer": "x"}))).await.0, StatusCode::CONFLICT);
        assert_eq!(call(&app, "POST", "/asks/999/answer", Some(json!({"answer": "x"}))).await.0, StatusCode::NOT_FOUND);
        let b = db.create_chat_ask(chat.id, None, "More?", &[]).unwrap();
        call(&app, "POST", "/chat", Some(json!({"chat_id": chat.id, "content": "just do it"}))).await;
        assert_eq!(db.get_chat_ask(b.id).unwrap().unwrap().status, "answered");
        assert!(call(&app, "GET", "/asks?status=open", None).await.1.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn stop_aborts_registered_handle() {
        let db = Arc::new(Db::open_test());
        let (chat_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let runs = Runs::default();
        let state = ApiState { db: db.clone(), chat_tx, seat: SeatCache::default(), runs: runs.clone(), live: Live::default(), data_dir: std::env::temp_dir().join("rui-api-test-none"), apps: Apps::default(), owner_token: crate::owner_auth::test_token() };
        let app = router(state);
        let (_, tid) = seed_task(&db);
        let join = tokio::spawn(tokio::time::sleep(std::time::Duration::from_secs(3600)));
        runs.lock().unwrap().insert(tid, join.abort_handle());
        let (st, _) = call(&app, "POST", &format!("/tasks/{tid}/stop"), None).await;
        assert_eq!(st, StatusCode::OK);
        assert!(join.await.unwrap_err().is_cancelled());
        assert!(!runs.lock().unwrap().contains_key(&tid));
    }

    #[tokio::test]
    async fn changes_unknown_404_and_no_worktree_empty() {
        let (app, db, _rx) = setup();
        let missing = call(&app, "GET", "/tasks/999/changes", None).await;
        assert_eq!(missing.0, StatusCode::NOT_FOUND);
        let (_, tid) = seed_task(&db);
        let (st, body) = call(&app, "GET", &format!("/tasks/{tid}/changes"), None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body, json!([]));
    }

    fn setup_with_dir(dir: &std::path::Path) -> (Router, i64) {
        let db = Arc::new(Db::open_test());
        let (chat_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (_, tid) = seed_task(&db);
        let state = ApiState { db, chat_tx, seat: SeatCache::default(), runs: Runs::default(), live: Live::default(), data_dir: dir.to_path_buf(), apps: Apps::default(), owner_token: crate::owner_auth::test_token() };
        (router(state), tid)
    }

    #[tokio::test]
    async fn live_returns_task_and_chat_state() {
        let db = Arc::new(Db::open_test());
        let (chat_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let live = Live::default();
        let st = crate::live::LiveState { text: "hi".into(), tool: Some("Bash".into()), ..Default::default() };
        live.lock().unwrap().insert(crate::live::LiveKey::Task(5), st);
        live.lock().unwrap().insert(crate::live::LiveKey::Chat(2), Default::default());
        let state = ApiState { db, chat_tx, seat: SeatCache::default(), runs: Runs::default(), live, data_dir: std::env::temp_dir(), apps: Apps::default(), owner_token: crate::owner_auth::test_token() };
        let (code, v) = call(&router(state), "GET", "/live", None).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(v["tasks"]["5"]["text"], "hi");
        assert_eq!(v["tasks"]["5"]["tool"], "Bash");
        assert_eq!(v["chats"]["2"], json!({"text":"","thinking":false,"tool":null,"tool_input":"","at":""}));
    }

    #[tokio::test]
    async fn cleaned_task_reads_changes_from_its_branch() {
        let repo = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| assert!(std::process::Command::new("git").arg("-C").arg(repo.path()).args(args).status().unwrap().success());
        git(&["init"]);
        std::fs::write(repo.path().join("a.txt"), "a").unwrap();
        git(&["add", "."]);
        git(&["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "init"]);
        let db = Arc::new(Db::open_test());
        let c = db.create_client("acme", &repo.path().to_string_lossy(), None, &[]).unwrap();
        let new = |parent: Option<i64>| NewTask { client_id: c.id, parent_id: parent, title: "t".into(), body: String::new(), status: TaskStatus::Done, done_command: None, max_loops: 2, role: Role::Lead, source_kind: SourceKind::User, source_ref: None, agent: None, pod: vec![] };
        let top = db.create_task(&new(None)).unwrap();
        let sub = db.create_task(&new(Some(top.id))).unwrap();
        let wts = data.path().join("worktrees");
        let (task_wt, tb) = worktree::ensure_task_worktree(repo.path(), &wts, top.id, &worktree::base_branch(repo.path()).unwrap()).unwrap();
        let (sub_wt, _) = worktree::create_worktree(repo.path(), &wts, sub.id, Some(&tb)).unwrap();
        std::fs::write(task_wt.join("b.txt"), "b\n").unwrap();
        worktree::commit_all(&task_wt, "add b").unwrap();
        db.add_event(Some(top.id), "owner", "landed", &json!({})).unwrap();
        let (chat_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let state = ApiState { db: db.clone(), chat_tx, seat: SeatCache::default(), runs: Runs::default(), live: Live::default(), data_dir: data.path().to_path_buf(), apps: Apps::default(), owner_token: crate::owner_auth::test_token() };
        let app = router(state);
        let (st, v) = call(&app, "POST", "/maintenance/clean-worktrees", None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["removed"].as_array().unwrap().len(), 2, "{v}");
        assert!(!task_wt.exists() && !sub_wt.exists());
        let (st, v) = call(&app, "GET", &format!("/tasks/{}/changes", top.id), None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v[0]["file"], "b.txt", "{v}");
        let (st, v) = call(&app, "GET", &format!("/tasks/{}/file?path=a.txt", sub.id), None).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        assert!(v.to_string().contains(&format!("rui/{}", sub.id)), "{v}");
    }

    #[tokio::test]
    async fn file_reads_guards_and_truncates() {
        let data = tempfile::tempdir().unwrap();
        let (app, tid) = setup_with_dir(data.path());
        let wt = data.path().join("worktrees").join(tid.to_string());
        std::fs::create_dir_all(wt.join("src")).unwrap();
        std::fs::write(wt.join("src/a.txt"), "hello").unwrap();
        std::fs::write(data.path().join("secret.txt"), "nope").unwrap();
        std::fs::write(wt.join("big.txt"), "z".repeat(300 * 1024)).unwrap();
        let get = |p: String| {
            let (app, uri) = (app.clone(), format!("/tasks/{tid}/file?path={p}"));
            async move { call(&app, "GET", &uri, None).await }
        };
        let (st, v) = get("src/a.txt".into()).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v, json!({"path": "src/a.txt", "content": "hello", "truncated": false}));
        assert_eq!(get("../../secret.txt".into()).await.0, StatusCode::FORBIDDEN);
        let abs = data.path().join("secret.txt").to_string_lossy().into_owned();
        assert_eq!(get(abs).await.0, StatusCode::FORBIDDEN);
        assert_eq!(get("missing.txt".into()).await.0, StatusCode::NOT_FOUND);
        let (_, big) = get("big.txt".into()).await;
        assert_eq!(big["truncated"], json!(true));
        assert_eq!(big["content"].as_str().unwrap().len(), 256 * 1024);
        let missing_task = call(&app, "GET", "/tasks/999/file?path=a", None).await;
        assert_eq!(missing_task.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn result_file_reads_repo_and_refuses_traversal() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::write(repo.path().join("plan.md"), "# hi").unwrap();
        std::fs::write(repo.path().parent().unwrap().join("outside-rui.txt"), "x").unwrap();
        let (app, db, _rx) = setup();
        let c = db.create_client("res", &repo.path().to_string_lossy(), None, &[]).unwrap();
        let tid = seed_task_for(&db, c.id);
        let get = |p: &str| {
            let (app, uri) = (app.clone(), format!("/tasks/{tid}/result-file?path={p}"));
            async move { call(&app, "GET", &uri, None).await }
        };
        let (st, v) = get("plan.md").await;
        assert_eq!((st, v["content"].as_str(), v["truncated"].as_bool()), (StatusCode::OK, Some("# hi"), Some(false)));
        assert_eq!(get("../outside-rui.txt").await.0, StatusCode::FORBIDDEN);
        assert_eq!(get("/etc/hosts").await.0, StatusCode::FORBIDDEN);
        assert_eq!(call(&app, "GET", "/tasks/999/result-file?path=a", None).await.0, StatusCode::NOT_FOUND);
        let (st, v) = call(&app, "GET", "/tasks/999/results", None).await;
        assert_eq!((st, v), (StatusCode::NOT_FOUND, json!({"error": "task not found"})));
        let (st, v) = call(&app, "GET", &format!("/tasks/{tid}/results"), None).await;
        assert_eq!((st, v), (StatusCode::OK, json!([])));
    }

    #[tokio::test]
    async fn threads_list_puts_needs_you_first() {
        let (app, db, _rx) = setup();
        let (client, first) = seed_task(&db);
        let second = seed_task_for(&db, client);
        db.create_ladder_question(second, Role::Lead, "owner", &[], "Pick one").unwrap();
        let (code, v) = call(&app, "GET", "/threads", None).await;
        assert_eq!(code, StatusCode::OK);
        let ids: Vec<i64> = v.as_array().unwrap().iter().map(|t| t["id"].as_i64().unwrap()).collect();
        assert_eq!(ids, vec![second, first]);
        assert_eq!(v[0]["state"], "needs_you");
        assert_eq!(v[0]["needs_you_count"], 1);
        let (_, only) = call(&app, "GET", "/threads?state=planned", None).await;
        assert_eq!(only.as_array().unwrap().len(), 1);
    }

    fn thread_ids(v: &Value) -> Vec<i64> {
        v.as_array().unwrap().iter().map(|t| t["id"].as_i64().unwrap()).collect()
    }

    #[tokio::test]
    async fn hiding_a_thread_takes_it_off_the_list_and_inbox() {
        let (app, db, _rx) = setup();
        let (_, top) = seed_task(&db);
        db.create_ladder_question(top, Role::Lead, "owner", &[], "Pick one").unwrap();
        assert_eq!(call(&app, "GET", "/inbox", None).await.1.as_array().unwrap().len(), 1);
        assert_eq!(call(&app, "POST", &format!("/threads/{top}/hide"), None).await.0, StatusCode::OK);
        assert!(thread_ids(&call(&app, "GET", "/threads", None).await.1).is_empty());
        assert!(call(&app, "GET", "/inbox", None).await.1.as_array().unwrap().is_empty());
        let (_, hidden) = call(&app, "GET", "/threads?hidden=1", None).await;
        assert_eq!((thread_ids(&hidden), hidden[0]["hidden"].as_bool()), (vec![top], Some(true)));
        assert_eq!(thread_ids(&call(&app, "GET", "/threads?all=1", None).await.1), vec![top]);
        assert_eq!(call(&app, "POST", &format!("/threads/{top}/unhide"), None).await.0, StatusCode::OK);
        let (_, back) = call(&app, "GET", "/threads", None).await;
        assert_eq!((thread_ids(&back), back[0]["needs_you_count"].as_i64()), (vec![top], Some(1)));
        assert!(thread_ids(&call(&app, "GET", "/threads?hidden=1", None).await.1).is_empty());
    }

    #[tokio::test]
    async fn removing_a_running_thread_drops_and_hides_it() {
        let (app, db, _rx) = setup();
        let (client, top) = seed_task(&db);
        let sub = seed_child(&db, client, Some(top));
        db.set_task_status(top, TaskStatus::InProgress).unwrap();
        let q = db.create_ladder_question(sub, Role::Lead, "owner", &[], "Pick one").unwrap();
        let (code, v) = call(&app, "POST", &format!("/threads/{top}/remove"), None).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(v["dropped"], json!([top, sub]));
        assert_eq!(db.get_task(top).unwrap().unwrap().status, TaskStatus::Stuck);
        assert_eq!(db.get_question(q.id).unwrap().unwrap().status, QuestionStatus::Answered);
        assert!(db.list_events(Some(top), 50).unwrap().iter().any(|e| e.kind == "dropped"));
        assert!(thread_ids(&call(&app, "GET", "/threads", None).await.1).is_empty());
        assert!(thread_ids(&call(&app, "GET", "/threads?hidden=1", None).await.1).is_empty());
        let (code, v) = call(&app, "POST", &format!("/threads/{top}/unhide"), None).await;
        assert_eq!((code, v["error"].as_str()), (StatusCode::CONFLICT, Some("removed threads stay hidden")));
    }

    #[tokio::test]
    async fn removing_a_done_thread_only_hides_it() {
        let (app, db, _rx) = setup();
        let (_, top) = seed_task(&db);
        db.set_task_status(top, TaskStatus::Done).unwrap();
        let (code, v) = call(&app, "POST", &format!("/threads/{top}/remove"), None).await;
        assert_eq!((code, v["dropped"].clone()), (StatusCode::OK, json!([])));
        assert_eq!(db.get_task(top).unwrap().unwrap().status, TaskStatus::Done);
        assert!(!db.list_events(Some(top), 50).unwrap().iter().any(|e| e.kind == "dropped"));
        assert!(thread_ids(&call(&app, "GET", "/threads?all=1", None).await.1) == vec![top]);
        assert!(thread_ids(&call(&app, "GET", "/threads", None).await.1).is_empty());
    }

    #[tokio::test]
    async fn hide_rejects_subtasks_and_unknown_ids() {
        let (app, db, _rx) = setup();
        let (client, top) = seed_task(&db);
        let sub = seed_child(&db, client, Some(top));
        for action in ["hide", "unhide", "remove"] {
            assert_eq!(call(&app, "POST", &format!("/threads/{sub}/{action}"), None).await.0, StatusCode::CONFLICT);
            assert_eq!(call(&app, "POST", &format!("/threads/9999/{action}"), None).await.0, StatusCode::NOT_FOUND);
        }
    }

    #[tokio::test]
    async fn thread_get_404s_for_a_subtask_id() {
        let (app, db, _rx) = setup();
        let (client, top) = seed_task(&db);
        let sub = seed_child(&db, client, Some(top));
        let (code, v) = call(&app, "GET", &format!("/threads/{top}"), None).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(v["id"], top);
        let (code, _) = call(&app, "GET", &format!("/threads/{sub}"), None).await;
        assert_eq!(code, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn thread_node_inspector_pages_events() {
        let (app, db, _rx) = setup();
        let (client, top) = seed_task(&db);
        let sub = seed_child(&db, client, Some(top));
        for _ in 0..250 {
            db.add_event(Some(sub), "builder", "tool_call", &json!({"tool": "x"})).unwrap();
        }
        let uri = format!("/threads/{top}/nodes/task:{sub}");
        let (code, page) = call(&app, "GET", &uri, None).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(page["events"].as_array().unwrap().len(), 200);
        let next = page["next_after_id"].as_i64().unwrap();
        let (_, rest) = call(&app, "GET", &format!("{uri}?after_id={next}"), None).await;
        assert_eq!(rest["events"].as_array().unwrap().len(), 50);
        assert!(rest["next_after_id"].is_null());
        let (code, _) = call(&app, "GET", &format!("/threads/{top}/nodes/task:9999"), None).await;
        assert_eq!(code, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn owner_answer_is_logged_for_the_account_lead() {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::open_test());
        let (chat_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let state = ApiState { db: db.clone(), chat_tx, seat: SeatCache::default(), runs: Runs::default(), live: Live::default(), data_dir: dir.path().to_path_buf(), apps: Apps::default(), owner_token: crate::owner_auth::test_token() };
        let app = router(state);
        let (_, tid) = seed_task(&db);
        crate::client_state::ensure(dir.path(), "acme", "/repo", None).unwrap();
        let q = db.create_question(tid, Role::Lead, "why?").unwrap();
        let uri = format!("/questions/{}/answer", q.id);
        let (st, v) = call(&app, "POST", &uri, Some(json!({"answer": "because"}))).await;
        assert_eq!(st, StatusCode::OK);
        let log_path = dir.path().join("clients/acme/decisions.log");
        assert!(std::fs::read_to_string(&log_path).unwrap_or_default().is_empty());
        crate::scheduler::deliver_held(&db, dir.path(), v["undo"]["event_id"].as_i64().unwrap()).unwrap();
        let log = std::fs::read_to_string(&log_path).unwrap();
        assert!(log.contains(&format!("Owner answered Q{} (task {tid}): because", q.id)), "{log}");
    }

    #[tokio::test]
    async fn owner_answer_can_be_undone_inside_the_window() {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Db::open_test());
        let (chat_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let state = ApiState { db: db.clone(), chat_tx, seat: SeatCache::default(), runs: Runs::default(), live: Live::default(), data_dir: dir.path().to_path_buf(), apps: Apps::default(), owner_token: crate::owner_auth::test_token() };
        let app = router(state);
        let (_, tid) = seed_task(&db);
        let q = db.create_question(tid, Role::Lead, "why?").unwrap();
        let uri = format!("/questions/{}/answer", q.id);
        let (_, v) = call(&app, "POST", &uri, Some(json!({"answer": "because"}))).await;
        assert_eq!(v["status"], "answered");
        let cancel = format!("/approvals/{}/cancel", v["undo"]["event_id"]);
        let (st, back) = call(&app, "POST", &cancel, None).await;
        assert_eq!((st, back["status"].as_str(), back["answer"].is_null()), (StatusCode::OK, Some("open"), true));
        assert_eq!(call(&app, "POST", &cancel, None).await.0, StatusCode::CONFLICT);
    }
}

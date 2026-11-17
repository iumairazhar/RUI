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
        .route("/clients", get(list_clients))
        .route("/tasks", get(list_tasks).post(create_task))
        .route("/questions", get(list_questions))
        .route("/questions/{id}/answer", post(answer_question))
        .route("/chats", get(list_chats).post(create_chat))
        .route("/chats/{id}", delete(delete_chat))
        .route("/chat", get(list_chat).post(post_chat))
        .route("/asks", get(list_asks))
        .route("/asks/{id}/answer", post(answer_ask))
        .route("/events", get(list_events))
        .route("/seat", get(get_seat))
        .route("/live", get(get_live))
        .route("/tasks/runs", get(list_task_runs))
        .route("/tasks/{id}/changes", get(task_changes))
        .route("/tasks/{id}/file", get(task_file))
        .route("/tasks/{id}/results", get(task_results))
        .route("/tasks/{id}/result-file", get(result_file))
        .route("/tasks/{id}/stop", post(stop_task))
        .route("/stop-all", post(stop_all))
        .route("/tasks/{id}/instruct", post(instruct_task))
        .route("/apps", get(list_apps))
        .route("/apps/{project}/restart", post(restart_apps))
        .route("/apps/{project}/stop", post(stop_apps))
        .route("/apps/{project}/{name}/stop", post(stop_app))
        .merge(crate::library_api::routes())
        .with_state(state)
}

async fn list_apps(State(s): State<ApiState>) -> Json<Vec<AppInfo>> {
    Json(apps::list(&s.apps))
}

/// Re-reads the project's apps.json and starts them in the background.
async fn restart_apps(State(s): State<ApiState>, Path(project): Path<String>) -> ApiResult<StatusCode> {
    let client = s.db.get_client_by_name(&project)?.ok_or_else(|| not_found("project not found"))?;
    let repo = crate::scheduler::expand_home(&client.repo_path);
    let apps = s.apps.clone();
    tokio::spawn(async move {
        apps::start_project(&apps, &client.name, &repo).await;
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

const CHAT_GREETING: &str = "What do you want set up?";

async fn list_chats(State(s): State<ApiState>) -> ApiResult<Json<Vec<Chat>>> {
    Ok(Json(s.db.list_chats()?))
}

/// Creates a chat that opens with a Rui greeting.
async fn create_chat(State(s): State<ApiState>) -> ApiResult<(StatusCode, Json<Chat>)> {
    let chat = s.db.create_chat()?;
    s.db.add_chat_message(chat.id, ChatRole::Rui, CHAT_GREETING)?;
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
    Ok(Json(info))
}

fn seat_kind(payload: &serde_json::Value) -> &'static str {
    match payload.get("apiKeySource").and_then(|v| v.as_str()) {
        Some("none") => "subscription",
        Some(_) => "api_key",
        None => "unknown",
    }
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

async fn answer_question(
    State(s): State<ApiState>,
    Path(id): Path<i64>,
    Json(b): Json<AnswerBody>,
) -> ApiResult<Json<Question>> {
    require_text(&b.answer, "answer")?;
    let q = s.db.get_question(id)?.ok_or_else(|| not_found("question not found"))?;
    if q.status == QuestionStatus::Answered {
        return Err(conflict("question is already answered"));
    }
    let why = b.why.as_deref().map(str::trim).filter(|w| !w.is_empty());
    Ok(Json(crate::scheduler::answer_question(&s.db, id, &b.answer, why, "owner")?))
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

/// Saves an owner instruction, aborts any run and puts the task back in the queue.
async fn instruct_task(
    State(s): State<ApiState>,
    Path(id): Path<i64>,
    Json(b): Json<InstructBody>,
) -> ApiResult<Json<Task>> {
    require_text(&b.text, "text")?;
    Ok(Json(crate::control::instruct(&s.db, &s.runs, id, &b.text, "owner")?))
}

/// Stops a task and its unfinished subtasks, aborting their runs.
async fn stop_task(State(s): State<ApiState>, Path(id): Path<i64>) -> ApiResult<Json<serde_json::Value>> {
    let ids = crate::control::stop_tree(&s.db, &s.runs, id, "owner", None)?;
    Ok(Json(serde_json::json!({ "stopped": ids })))
}

/// Stops every top-level task that is not done or stuck, with its subtasks.
async fn stop_all(State(s): State<ApiState>) -> ApiResult<Json<serde_json::Value>> {
    let open: Vec<i64> = (s.db.list_tasks()?.iter())
        .filter(|t| t.parent_id.is_none() && !matches!(t.status, TaskStatus::Done | TaskStatus::Stuck))
        .map(|t| t.id)
        .collect();
    let mut stopped = Vec::new();
    for id in open {
        stopped.extend(crate::control::stop_tree(&s.db, &s.runs, id, "owner", None)?);
    }
    Ok(Json(serde_json::json!({ "stopped": stopped })))
}

/// Files changed by a task: committed diff when done, else the live worktree diff.
async fn task_changes(State(s): State<ApiState>, Path(id): Path<i64>) -> ApiResult<Json<Vec<Change>>> {
    let task = s.db.get_task(id)?.ok_or_else(|| not_found("task not found"))?;
    let dir = s.data_dir.join("worktrees").join(id.to_string());
    if !dir.exists() {
        return Ok(Json(vec![]));
    }
    let done = task.status == TaskStatus::Done;
    let list = tokio::task::spawn_blocking(move || worktree::changes(&dir, done))
        .await
        .map_err(anyhow::Error::from)??;
    Ok(Json(list))
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
    s.db.get_task(id)?.ok_or_else(|| not_found("task not found"))?;
    let dir = s.data_dir.join("worktrees").join(id.to_string());
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
        (router(ApiState { db: db.clone(), chat_tx, seat: SeatCache::default(), runs: Runs::default(), live: Live::default(), data_dir: std::env::temp_dir().join("rui-api-test-none"), apps: Apps::default() }), db, rx)
    }

    async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(uri);
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
        assert_eq!(db.get_task(member.id).unwrap().unwrap().status, TaskStatus::Open);
        assert_eq!(db.pending_instructions(member.id).unwrap().len(), 1);
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
        let state = ApiState { db: db.clone(), chat_tx, seat: SeatCache::default(), runs: Runs::default(), live, data_dir: std::env::temp_dir(), apps: Apps::default() };
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

    #[tokio::test]
    async fn chats_create_list_and_title_from_first_message() {
        let (app, _db, _rx) = setup();
        let (st, c) = call(&app, "POST", "/chats", None).await;
        assert_eq!(st, StatusCode::CREATED);
        assert_eq!(c["title"].as_str(), Some("New chat"));
        assert_eq!(c["last"].as_str(), Some("What do you want set up?"));
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
        let state = ApiState { db: db.clone(), chat_tx, seat: SeatCache::default(), runs: runs.clone(), live: Live::default(), data_dir: std::env::temp_dir().join("rui-api-test-none"), apps: Apps::default() };
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
        let state = ApiState { db, chat_tx, seat: SeatCache::default(), runs: Runs::default(), live: Live::default(), data_dir: dir.to_path_buf(), apps: Apps::default() };
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
        let state = ApiState { db, chat_tx, seat: SeatCache::default(), runs: Runs::default(), live, data_dir: std::env::temp_dir(), apps: Apps::default() };
        let (code, v) = call(&router(state), "GET", "/live", None).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(v["tasks"]["5"]["text"], "hi");
        assert_eq!(v["tasks"]["5"]["tool"], "Bash");
        assert_eq!(v["chats"]["2"], json!({"text":"","thinking":false,"tool":null,"tool_input":"","at":""}));
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
}

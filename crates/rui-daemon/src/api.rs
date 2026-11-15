//! JSON HTTP API over the database.

use crate::db::{Db, NewTask};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use crate::seat::SeatCache;
use rui_core::{
    ChatMessage, ChatRole, Client, Event, Question, QuestionStatus, Role, SeatInfo, SourceKind, Task, TaskRun,
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
        .route("/chat", get(list_chat).post(post_chat))
        .route("/events", get(list_events))
        .route("/seat", get(get_seat))
        .route("/tasks/runs", get(list_task_runs))
        .with_state(state)
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
    Ok(Json(s.db.list_questions(status)?))
}

#[derive(Deserialize)]
struct ChatQuery {
    limit: Option<i64>,
}

async fn list_chat(
    State(s): State<ApiState>,
    Query(q): Query<ChatQuery>,
) -> ApiResult<Json<Vec<ChatMessage>>> {
    Ok(Json(s.db.list_chat(q.limit.unwrap_or(100))?))
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
    content: String,
}

async fn post_chat(
    State(s): State<ApiState>,
    Json(b): Json<ChatBody>,
) -> ApiResult<(StatusCode, Json<ChatMessage>)> {
    require_text(&b.content, "content")?;
    let msg = s.db.add_chat_message(ChatRole::User, &b.content)?;
    let _ = s.chat_tx.send(msg.id);
    Ok((StatusCode::CREATED, Json(msg)))
}

#[derive(Deserialize)]
struct AnswerBody {
    answer: String,
}

async fn answer_question(
    State(s): State<ApiState>,
    Path(id): Path<i64>,
    Json(b): Json<AnswerBody>,
) -> ApiResult<Json<Question>> {
    require_text(&b.answer, "answer")?;
    let q = s.db.answer_question(id, &b.answer)?;
    Ok(Json(q.ok_or_else(|| not_found("question not found"))?))
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
    })?;
    Ok((StatusCode::CREATED, Json(task)))
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
        (router(ApiState { db: db.clone(), chat_tx, seat: SeatCache::default() }), db, rx)
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
            })
            .unwrap();
        (c.id, t.id)
    }

    fn seed_task_for(db: &Db, client_id: i64) -> i64 {
        db.create_task(&NewTask {
            client_id,
            parent_id: None,
            title: "t".into(),
            body: String::new(),
            status: TaskStatus::New,
            done_command: None,
            max_loops: 5,
            role: Role::Lead,
            source_kind: SourceKind::User,
            source_ref: None,
        })
        .unwrap()
        .id
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
        let (app, _db, mut rx) = setup();
        let (st, v) = call(&app, "POST", "/chat", Some(json!({"content": "hi"}))).await;
        assert_eq!((st, v["role"].as_str()), (StatusCode::CREATED, Some("user")));
        assert_eq!(rx.try_recv().unwrap(), v["id"].as_i64().unwrap());
        let (_, v) = call(&app, "GET", "/chat?limit=10", None).await;
        assert_eq!(v[0]["content"].as_str(), Some("hi"));
        let (st, _) = call(&app, "POST", "/chat", Some(json!({"content": ""}))).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert!(rx.try_recv().is_err());
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
}

//! HTTP routes for the skill and role library. Errors render as `{error}` with 400, 404, 409 or 500.

use crate::api::ApiState;
use crate::library::{self as lib, Kind, LibError, LibResult};
use crate::{library_roles as roles, library_skills as skills};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};

impl IntoResponse for LibError {
    fn into_response(self) -> Response {
        let (code, msg) = match self {
            LibError::Bad(m) => (StatusCode::BAD_REQUEST, m),
            LibError::NotFound(m) => (StatusCode::NOT_FOUND, m),
            LibError::Conflict(m) => (StatusCode::CONFLICT, m),
            LibError::Other(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
        };
        (code, Json(json!({ "error": msg }))).into_response()
    }
}

type Reply = Result<Json<Value>, LibError>;

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/skills", get(list_skills).post(create_skill))
        .route("/skills/import", post(import_skill))
        .route("/skills/upload", post(upload_skill))
        .route("/skills/{name}", get(get_skill).put(edit_skill).delete(delete_skill))
        .route("/skills/{name}/files", get(skill_files))
        .route("/skills/{name}/status", post(skill_status))
        .route("/skills/{name}/attach", post(attach_skill))
        .route("/skills/{name}/tools", post(skill_tools))
        .route("/skills/{name}/restore", post(restore_skill))
        .route("/roles", get(list_roles).post(create_role))
        .route("/roles/{name}", get(get_role).put(edit_role).delete(delete_role))
        .route("/roles/{name}/status", post(role_status))
        .route("/roles/{name}/restore", post(restore_role))
        .route("/roles/{name}/limit", post(role_limit))
        .route("/roles/{name}/tools", post(role_tools))
}

#[derive(Deserialize)]
struct Name {
    name: String,
}

#[derive(Deserialize)]
struct Edit {
    text: String,
    #[serde(default)]
    why: Option<String>,
}

#[derive(Deserialize)]
struct Status {
    status: String,
}

#[derive(Deserialize)]
struct Restore {
    ts: u64,
}

#[derive(Deserialize)]
struct Tools {
    tools: Vec<String>,
}

#[derive(Deserialize)]
struct Attach {
    #[serde(default)]
    roles: Vec<String>,
    #[serde(default)]
    clients: Vec<String>,
}

#[derive(Deserialize)]
struct NewRole {
    name: String,
    code: String,
    title: String,
    description: String,
}

fn ok() -> Reply {
    Ok(Json(json!({"ok": true})))
}

fn used_7d(s: &ApiState) -> LibResult<std::collections::HashMap<String, i64>> {
    Ok(s.db.skill_use_counts(7)?)
}

async fn list_skills(State(s): State<ApiState>) -> Reply {
    Ok(Json(json!(skills::list(&s.data_dir, &used_7d(&s)?))))
}

async fn get_skill(State(s): State<ApiState>, Path(name): Path<String>) -> Reply {
    Ok(Json(skills::get(&s.data_dir, &name, &used_7d(&s)?)?))
}

async fn skill_files(State(s): State<ApiState>, Path(name): Path<String>) -> Reply {
    Ok(Json(json!(skills::files(&s.data_dir, &name)?)))
}

async fn skill_reply(s: &ApiState, name: &str) -> Reply {
    Ok(Json(skills::get(&s.data_dir, name, &used_7d(s)?)?))
}

async fn create_skill(State(s): State<ApiState>, Json(b): Json<Name>) -> Reply {
    skills::create(&s.data_dir, &b.name)?;
    skill_reply(&s, &b.name).await
}

#[derive(Deserialize)]
struct Url {
    url: String,
}

async fn import_skill(State(s): State<ApiState>, Json(b): Json<Url>) -> Reply {
    let data = s.data_dir.clone();
    let name = tokio::task::spawn_blocking(move || skills::import(&data, &b.url))
        .await
        .map_err(|e| LibError::Other(e.into()))??;
    skill_reply(&s, &name).await
}

#[derive(Deserialize)]
struct Folder {
    path: String,
}

async fn upload_skill(State(s): State<ApiState>, Json(b): Json<Folder>) -> Reply {
    let name = skills::upload(&s.data_dir, &b.path)?;
    skill_reply(&s, &name).await
}

async fn edit_skill(State(s): State<ApiState>, Path(name): Path<String>, Json(b): Json<Edit>) -> Reply {
    lib::edit(&s.data_dir, Kind::Skill, &name, &b.text, b.why.as_deref())?;
    skill_reply(&s, &name).await
}

async fn delete_skill(State(s): State<ApiState>, Path(name): Path<String>) -> Reply {
    skills::delete(&s.data_dir, &name)?;
    ok()
}

async fn skill_status(State(s): State<ApiState>, Path(name): Path<String>, Json(b): Json<Status>) -> Reply {
    lib::set_status(&s.data_dir, Kind::Skill, &name, &b.status)?;
    skill_reply(&s, &name).await
}

async fn attach_skill(State(s): State<ApiState>, Path(name): Path<String>, Json(b): Json<Attach>) -> Reply {
    skills::attach(&s.data_dir, &name, &b.roles, &b.clients)?;
    skill_reply(&s, &name).await
}

async fn skill_tools(State(s): State<ApiState>, Path(name): Path<String>, Json(b): Json<Tools>) -> Reply {
    skills::set_tools(&s.data_dir, &name, &b.tools)?;
    skill_reply(&s, &name).await
}

async fn restore_skill(State(s): State<ApiState>, Path(name): Path<String>, Json(b): Json<Restore>) -> Reply {
    lib::restore(&s.data_dir, Kind::Skill, &name, b.ts)?;
    skill_reply(&s, &name).await
}

async fn list_roles(State(s): State<ApiState>) -> Reply {
    Ok(Json(json!(roles::list(&s.data_dir))))
}

async fn get_role(State(s): State<ApiState>, Path(name): Path<String>) -> Reply {
    Ok(Json(roles::get(&s.data_dir, &name)?))
}

async fn create_role(State(s): State<ApiState>, Json(b): Json<NewRole>) -> Reply {
    roles::create(&s.data_dir, &b.name, &b.code, &b.title, &b.description)?;
    Ok(Json(roles::get(&s.data_dir, &b.name)?))
}

async fn edit_role(State(s): State<ApiState>, Path(name): Path<String>, Json(b): Json<Edit>) -> Reply {
    lib::edit(&s.data_dir, Kind::Role, &name, &b.text, b.why.as_deref())?;
    Ok(Json(roles::get(&s.data_dir, &name)?))
}

async fn delete_role(State(s): State<ApiState>, Path(name): Path<String>) -> Reply {
    roles::delete(&s.data_dir, &name)?;
    ok()
}

async fn role_status(State(s): State<ApiState>, Path(name): Path<String>, Json(b): Json<Status>) -> Reply {
    lib::set_status(&s.data_dir, Kind::Role, &name, &b.status)?;
    Ok(Json(roles::get(&s.data_dir, &name)?))
}

async fn restore_role(State(s): State<ApiState>, Path(name): Path<String>, Json(b): Json<Restore>) -> Reply {
    lib::restore(&s.data_dir, Kind::Role, &name, b.ts)?;
    Ok(Json(roles::get(&s.data_dir, &name)?))
}

#[derive(Deserialize)]
struct Limit {
    max_loops: i64,
}

async fn role_limit(State(s): State<ApiState>, Path(name): Path<String>, Json(b): Json<Limit>) -> Reply {
    roles::set_limit(&s.data_dir, &name, b.max_loops)?;
    Ok(Json(roles::get(&s.data_dir, &name)?))
}

async fn role_tools(State(s): State<ApiState>, Path(name): Path<String>, Json(b): Json<Tools>) -> Reply {
    roles::set_tools(&s.data_dir, &name, &b.tools)?;
    Ok(Json(roles::get(&s.data_dir, &name)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{api::router, db::Db, live::Live, apps::Apps, scheduler::Runs, seat::SeatCache};
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use std::sync::Arc;
    use tower::ServiceExt;

    fn app(data: &std::path::Path) -> Router {
        let (chat_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let db = Arc::new(Db::open_test());
        router(ApiState { db, chat_tx, seat: SeatCache::default(), runs: Runs::default(), live: Live::default(), data_dir: data.to_path_buf(), apps: Apps::default() })
    }

    async fn call(app: &Router, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
        let req = Request::builder().method(method).uri(uri).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn skill_round_trip_over_http() {
        let d = tempfile::tempdir().unwrap();
        let app = app(d.path());
        let (st, _) = call(&app, "POST", "/skills", json!({"name": "my-skill"})).await;
        assert_eq!(st, StatusCode::OK);
        let (st, _) = call(&app, "POST", "/skills", json!({"name": "my-skill"})).await;
        assert_eq!(st, StatusCode::CONFLICT);
        let (st, _) = call(&app, "POST", "/skills", json!({"name": "../x"})).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        let (_, v) = call(&app, "GET", "/skills/my-skill", json!(null)).await;
        let text = v["text"].as_str().unwrap().replace("Describe when", "Use when");
        let (st, v) = call(&app, "PUT", "/skills/my-skill", json!({"text": text, "why": "tidy"})).await;
        assert_eq!((st, v["hist"].as_array().unwrap().len(), v["versions"].as_array().unwrap().len()), (StatusCode::OK, 2, 1));
        let (st, v) = call(&app, "POST", "/skills/my-skill/status", json!({"status": "on"})).await;
        assert_eq!((st, v["status"].as_str()), (StatusCode::OK, Some("on")));
        let (st, _) = call(&app, "GET", "/skills/nope", json!(null)).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        let (st, v) = call(&app, "PUT", "/skills/my-skill", json!({"text": "junk"})).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert!(v["error"].is_string());
        let (st, _) = call(&app, "DELETE", "/skills/my-skill", json!(null)).await;
        assert_eq!(st, StatusCode::OK);
        let (_, v) = call(&app, "GET", "/skills", json!(null)).await;
        assert_eq!(v, json!([]));
    }

    #[tokio::test]
    async fn roles_over_http() {
        let d = tempfile::tempdir().unwrap();
        let app = app(d.path());
        let body = json!({"name": "ops", "code": "OP", "title": "Ops", "description": "Runs things"});
        assert_eq!(call(&app, "POST", "/roles", body.clone()).await.0, StatusCode::OK);
        assert_eq!(call(&app, "POST", "/roles", body).await.0, StatusCode::CONFLICT);
        let (_, v) = call(&app, "GET", "/roles", json!(null)).await;
        assert_eq!((v[0]["status"].as_str(), v[0].get("body").is_none()), (Some("off"), true));
        assert_eq!(call(&app, "DELETE", "/roles/lead", json!(null)).await.0, StatusCode::BAD_REQUEST);
        assert_eq!(call(&app, "POST", "/roles/ops/limit", json!({"max_loops": 2})).await.0, StatusCode::BAD_REQUEST);
        assert_eq!(call(&app, "POST", "/roles/ops/limit", json!({"max_loops": 8})).await.0, StatusCode::OK);
    }
}

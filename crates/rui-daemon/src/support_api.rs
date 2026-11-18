//! HTTP routes for the support agents:
//! POST /tasks/{id}/diagnose (ask the Scout), POST /proposals/{event_id} (approve or dismiss a
//! Coach proposal), POST /scout-reports/{event_id}/dismiss (clear a Scout report from the Inbox).

use crate::api::ApiState;
use crate::{coach, scout};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::post,
};
use serde::Deserialize;
use serde_json::{Value, json};

type Reply = Result<Json<Value>, (StatusCode, Json<Value>)>;

fn fail(code: StatusCode, e: impl std::fmt::Display) -> (StatusCode, Json<Value>) {
    (code, Json(json!({ "error": e.to_string() })))
}

fn internal(e: anyhow::Error) -> (StatusCode, Json<Value>) {
    fail(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}"))
}

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/tasks/{id}/diagnose", post(diagnose))
        .route("/proposals/{event_id}", post(resolve_proposal))
        .route("/scout-reports/{event_id}/dismiss", post(dismiss_report))
}

/// Asks the Scout why this task failed or stalled. The report shows in the Inbox.
async fn diagnose(State(s): State<ApiState>, Path(id): Path<i64>) -> Reply {
    if s.db.get_task(id).map_err(internal)?.is_none() {
        return Err(fail(StatusCode::NOT_FOUND, "task not found"));
    }
    let event_id = scout::request(&s.db, id, scout::DIAGNOSE_QUESTION, "owner").map_err(internal)?;
    Ok(Json(json!({ "request_event_id": event_id })))
}

#[derive(Deserialize)]
struct VerdictBody {
    verdict: String,
}

/// Approves (applies, append-only) or dismisses one pending Coach proposal.
async fn resolve_proposal(State(s): State<ApiState>, Path(event_id): Path<i64>, Json(b): Json<VerdictBody>) -> Reply {
    if !["approve", "dismiss"].contains(&b.verdict.as_str()) {
        return Err(fail(StatusCode::BAD_REQUEST, "verdict must be approve or dismiss"));
    }
    let Some(e) = coach::find_pending(&s.db, event_id).map_err(internal)? else {
        return Err(fail(StatusCode::NOT_FOUND, "no pending proposal with that id"));
    };
    coach::resolve(&s.db, &s.data_dir, &e, &b.verdict).map_err(internal)?;
    Ok(Json(json!({ "ok": true, "verdict": b.verdict })))
}

async fn dismiss_report(State(s): State<ApiState>, Path(event_id): Path<i64>) -> Reply {
    if !scout::dismiss(&s.db, event_id).map_err(internal)? {
        return Err(fail(StatusCode::NOT_FOUND, "no open Scout report with that id"));
    }
    Ok(Json(json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use crate::api::{ApiState, router};
    use crate::db::{Db, NewTask};
    use crate::{apps::Apps, client_state, live::Live, scheduler::Runs, seat::SeatCache};
    use axum::{Router, body::Body, http::Request, http::StatusCode};
    use http_body_util::BodyExt;
    use rui_core::{Role, SourceKind, TaskStatus};
    use serde_json::{Value, json};
    use std::sync::Arc;
    use tower::ServiceExt;

    fn app(data: &std::path::Path) -> (Router, Arc<Db>, i64) {
        let (chat_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let db = Arc::new(Db::open_test());
        let c = db.create_client("acme", "/tmp/acme", None, &[]).unwrap();
        client_state::ensure(data, "acme", "/tmp/acme", None).unwrap();
        let t = db
            .create_task(&NewTask {
                client_id: c.id, parent_id: None, title: "t".into(), body: String::new(), status: TaskStatus::Done,
                done_command: None, max_loops: 5, role: Role::Lead, source_kind: SourceKind::User, source_ref: None,
                agent: None, pod: vec![],
            })
            .unwrap();
        let state = ApiState { db: db.clone(), chat_tx, seat: SeatCache::default(), runs: Runs::default(), live: Live::default(), data_dir: data.to_path_buf(), apps: Apps::default() };
        (router(state), db, t.id)
    }

    async fn post(app: &Router, uri: &str, body: Value) -> (StatusCode, Value) {
        let req = Request::post(uri).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn diagnose_creates_an_owner_request() {
        let d = tempfile::tempdir().unwrap();
        let (app, db, id) = app(d.path());
        let (code, body) = post(&app, &format!("/tasks/{id}/diagnose"), json!({})).await;
        assert_eq!(code, StatusCode::OK);
        let open = crate::scout::open_requests(&db).unwrap();
        assert_eq!((open.len(), open[0].id), (1, body["request_event_id"].as_i64().unwrap()));
        assert_eq!(open[0].payload["by"], "owner");
        assert_eq!(post(&app, "/tasks/999/diagnose", json!({})).await.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn proposals_approve_dismiss_and_errors() {
        let d = tempfile::tempdir().unwrap();
        let (app, db, id) = app(d.path());
        let p = |text: &str| crate::coach::Proposal { target: "playbook".into(), text: text.into(), why: "w".into() };
        let a = crate::coach::propose(&db, d.path(), id, p("Keep it short")).unwrap();
        let b = crate::coach::propose(&db, d.path(), id, p("Never do X")).unwrap();
        assert_eq!(post(&app, &format!("/proposals/{a}"), json!({"verdict": "maybe"})).await.0, StatusCode::BAD_REQUEST);
        assert_eq!(post(&app, &format!("/proposals/{a}"), json!({"verdict": "approve"})).await.0, StatusCode::OK);
        assert_eq!(post(&app, &format!("/proposals/{b}"), json!({"verdict": "dismiss"})).await.0, StatusCode::OK);
        assert_eq!(post(&app, &format!("/proposals/{a}"), json!({"verdict": "approve"})).await.0, StatusCode::NOT_FOUND);
        assert_eq!(client_state::read_playbook(d.path(), "acme").unwrap(), "- Keep it short\n");
    }
}

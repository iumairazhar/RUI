//! HTTP routes for seats: GET /seats, POST /seats, DELETE /seats/{id}. Key values are never returned.

use crate::api::ApiState;
use crate::seats::{self, Seat, SeatStatus};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get},
};
use serde_json::{Value, json};

type Reply<T> = Result<Json<T>, (StatusCode, Json<Value>)>;

fn fail(code: StatusCode, e: impl std::fmt::Display) -> (StatusCode, Json<Value>) {
    (code, Json(json!({ "error": e.to_string() })))
}

fn internal(e: anyhow::Error) -> (StatusCode, Json<Value>) {
    fail(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}"))
}

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/seats", get(list_seats).post(save_seat))
        .route("/seats/{id}", delete(delete_seat))
}

async fn list_seats(State(s): State<ApiState>) -> Reply<Vec<SeatStatus>> {
    let list = seats::list_status(&s.db, &s.data_dir, crate::thread::now_secs()).map_err(internal)?;
    Ok(Json(list))
}

/// Adds a seat or replaces the one with the same id.
async fn save_seat(State(s): State<ApiState>, Json(seat): Json<Seat>) -> Reply<Value> {
    seats::validate(&seat).map_err(|e| fail(StatusCode::BAD_REQUEST, e))?;
    seats::upsert(&s.data_dir, seat).map_err(internal)?;
    Ok(Json(json!({ "ok": true })))
}

async fn delete_seat(State(s): State<ApiState>, Path(id): Path<String>) -> Reply<Value> {
    if !seats::remove(&s.data_dir, &id).map_err(internal)? {
        return Err(fail(StatusCode::NOT_FOUND, "seat not found"));
    }
    Ok(Json(json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use crate::api::{ApiState, router};
    use crate::{apps::Apps, db::Db, live::Live, scheduler::Runs, seat::SeatCache};
    use axum::{Router, body::Body, http::Request, http::StatusCode};
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use std::sync::Arc;
    use tower::ServiceExt;

    fn app(data: &std::path::Path) -> Router {
        let (chat_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let db = Arc::new(Db::open_test());
        router(ApiState { db, chat_tx, seat: SeatCache::default(), runs: Runs::default(), live: Live::default(), data_dir: data.to_path_buf(), apps: Apps::default() })
    }

    async fn call(app: &Router, method: &str, uri: &str, body: Value) -> (StatusCode, String) {
        let req = Request::builder().method(method).uri(uri).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    #[tokio::test]
    async fn missing_file_lists_the_default_seat_as_active() {
        let d = tempfile::tempdir().unwrap();
        let (st, body) = call(&app(d.path()), "GET", "/seats", Value::Null).await;
        assert_eq!(st, StatusCode::OK);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 1);
        assert_eq!((v[0]["id"].as_str(), v[0]["active"].as_bool()), (Some("default"), Some(true)));
        assert_eq!(v[0]["env_var_present"], true);
    }

    #[tokio::test]
    async fn api_never_returns_key_values() {
        let d = tempfile::tempdir().unwrap();
        let app = app(d.path());
        let name = "RUI_TEST_SEATS_API_KEY";
        let secret = "sk-ant-fake-secret-value-123";
        // SAFETY: this test alone uses this variable name.
        unsafe { std::env::set_var(name, secret) };
        let seat = json!({"id": "work", "kind": "api_key", "label": "Work key", "env_var": name, "budget_usd": 20.0, "order": 1});
        assert_eq!(call(&app, "POST", "/seats", seat).await.0, StatusCode::OK);
        let (st, body) = call(&app, "GET", "/seats", Value::Null).await;
        unsafe { std::env::remove_var(name) };
        assert_eq!(st, StatusCode::OK);
        assert!(!body.contains(secret));
        let v: Value = serde_json::from_str(&body).unwrap();
        let work = v.as_array().unwrap().iter().find(|s| s["id"] == "work").unwrap().clone();
        assert_eq!((work["env_var"].as_str(), work["env_var_present"].as_bool()), (Some(name), Some(true)));
        assert_eq!(work["used_usd"], 0.0);
        let file = std::fs::read_to_string(d.path().join("seats.json")).unwrap();
        assert!(!file.contains(secret));
    }

    #[tokio::test]
    async fn rejects_bad_fields_and_deletes() {
        let d = tempfile::tempdir().unwrap();
        let app = app(d.path());
        let key_as_name = json!({"id": "k", "kind": "api_key", "label": "K", "env_var": "sk-ant-abc"});
        assert_eq!(call(&app, "POST", "/seats", key_as_name).await.0, StatusCode::BAD_REQUEST);
        let rel = json!({"id": "s", "kind": "subscription", "label": "S", "config_dir": "rel"});
        assert_eq!(call(&app, "POST", "/seats", rel).await.0, StatusCode::BAD_REQUEST);
        let ok = json!({"id": "s", "kind": "subscription", "label": "S", "config_dir": "~/.claude-two", "order": 2});
        assert_eq!(call(&app, "POST", "/seats", ok).await.0, StatusCode::OK);
        assert_eq!(call(&app, "DELETE", "/seats/s", Value::Null).await.0, StatusCode::OK);
        assert_eq!(call(&app, "DELETE", "/seats/s", Value::Null).await.0, StatusCode::NOT_FOUND);
    }
}

//! Cost report: GET /costs?since=<RFC 3339> or ?days=N (default 30 days).
//! Sums the `total_cost_usd` and token `usage` of every `result` event in the window, by client,
//! by thread (top 50; subtask spend rolls up into its top-level task), by day and by seat.
//! On subscription seats the dollars are the CLI's own estimate, so those seat rows say `estimate: true`.

use crate::api::ApiState;
use crate::db::{Db, ResultCost};
use anyhow::{Result, bail};
use axum::{Json, Router, extract::{Query, State}, http::StatusCode, routing::get};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

const TOP_THREADS: usize = 50;
const DEFAULT_DAYS: i64 = 30;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Report {
    pub since: String,
    pub total_usd: f64,
    pub total_tokens: i64,
    pub by_client: Vec<ClientCost>,
    pub by_thread: Vec<ThreadCost>,
    pub by_day: Vec<DayCost>,
    pub by_seat: Vec<SeatCost>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ClientCost {
    pub client_id: i64,
    pub name: String,
    pub usd: f64,
    pub tokens: i64,
    /// Top-level tasks with spend in the window.
    pub tasks: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ThreadCost {
    pub task_id: i64,
    pub title: String,
    pub client: String,
    pub usd: f64,
    pub tokens: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DayCost {
    pub date: String,
    pub usd: f64,
    pub tokens: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SeatCost {
    pub seat_id: String,
    pub usd: f64,
    pub tokens: i64,
    /// True on subscription seats: the dollars are the CLI's estimate, not a bill.
    pub estimate: bool,
}

/// Running dollars and tokens for one group.
#[derive(Debug, Clone, Copy, Default)]
struct Sum {
    usd: f64,
    tokens: i64,
}

impl Sum {
    fn add(&mut self, r: &ResultCost) {
        self.usd += r.usd;
        self.tokens += r.tokens;
    }
}

/// The window start as an ISO time: `since` (RFC 3339) wins, else `days` back from now (default 30).
pub fn window_start(since: Option<&str>, days: Option<i64>) -> Result<String> {
    if let Some(s) = since {
        let t = chrono::DateTime::parse_from_rfc3339(s).map_err(|_| anyhow::anyhow!("since must be an RFC 3339 time"))?;
        return Ok(iso(t.to_utc()));
    }
    let days = days.unwrap_or(DEFAULT_DAYS);
    if !(1..=3650).contains(&days) {
        bail!("days must be between 1 and 3650");
    }
    Ok(iso(chrono::Utc::now() - chrono::Duration::days(days)))
}

fn iso(t: chrono::DateTime<chrono::Utc>) -> String {
    t.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// The top-level task of every task id (a top-level task maps to itself).
fn tops(db: &Db) -> Result<HashMap<i64, i64>> {
    let parents: HashMap<i64, Option<i64>> = db.list_tasks()?.into_iter().map(|t| (t.id, t.parent_id)).collect();
    let top = |mut id: i64| {
        for _ in 0..32 {
            match parents.get(&id).copied().flatten() {
                Some(p) => id = p,
                None => break,
            }
        }
        id
    };
    Ok(parents.keys().map(|id| (*id, top(*id))).collect())
}

/// Builds the whole report for result events since `since`.
pub fn report(db: &Db, data_dir: &Path, since: &str) -> Result<Report> {
    let rows = db.result_costs_since(since)?;
    let tops = tops(db)?;
    let mut threads: HashMap<i64, Sum> = HashMap::new();
    let mut days: BTreeMap<String, Sum> = BTreeMap::new();
    let mut seats: BTreeMap<String, Sum> = BTreeMap::new();
    let mut total = Sum::default();
    for r in &rows {
        total.add(r);
        days.entry(r.created_at.chars().take(10).collect()).or_default().add(r);
        seats.entry(r.seat_id.clone().unwrap_or_else(|| "default".into())).or_default().add(r);
        if let Some(top) = r.task_id.map(|id| tops.get(&id).copied().unwrap_or(id)) {
            threads.entry(top).or_default().add(r);
        }
    }
    let (by_client, by_thread) = group_threads(db, &threads)?;
    let by_day = days.into_iter().map(|(date, s)| DayCost { date, usd: s.usd, tokens: s.tokens }).collect();
    Ok(Report { since: since.into(), total_usd: total.usd, total_tokens: total.tokens, by_client, by_thread, by_day, by_seat: seat_rows(data_dir, seats) })
}

/// Seat rows, flagged as estimates when the seat is a subscription (unknown seats count as one).
fn seat_rows(data_dir: &Path, seats: BTreeMap<String, Sum>) -> Vec<SeatCost> {
    let api: HashSet<String> = crate::seats::load(data_dir)
        .unwrap_or_default()
        .into_iter()
        .filter(|s| s.kind == crate::seats::SeatKind::ApiKey)
        .map(|s| s.id)
        .collect();
    seats.into_iter().map(|(id, s)| SeatCost { estimate: !api.contains(&id), seat_id: id, usd: s.usd, tokens: s.tokens }).collect()
}

/// Per-client sums and the top threads by dollars, then tokens.
fn group_threads(db: &Db, threads: &HashMap<i64, Sum>) -> Result<(Vec<ClientCost>, Vec<ThreadCost>)> {
    let clients: HashMap<i64, String> = db.list_clients()?.into_iter().map(|c| (c.id, c.name)).collect();
    let mut by_client: BTreeMap<i64, ClientCost> = BTreeMap::new();
    let mut by_thread = vec![];
    for (id, s) in threads {
        let Some(task) = db.get_task(*id)? else { continue };
        let name = clients.get(&task.client_id).cloned().unwrap_or_default();
        let c = by_client.entry(task.client_id).or_insert(ClientCost { client_id: task.client_id, name: name.clone(), usd: 0.0, tokens: 0, tasks: 0 });
        (c.usd, c.tokens, c.tasks) = (c.usd + s.usd, c.tokens + s.tokens, c.tasks + 1);
        by_thread.push(ThreadCost { task_id: *id, title: task.title, client: name, usd: s.usd, tokens: s.tokens });
    }
    by_thread.sort_by(|a, b| b.usd.total_cmp(&a.usd).then(b.tokens.cmp(&a.tokens)).then(a.task_id.cmp(&b.task_id)));
    by_thread.truncate(TOP_THREADS);
    let mut clients: Vec<ClientCost> = by_client.into_values().collect();
    clients.sort_by(|a, b| b.usd.total_cmp(&a.usd).then(a.client_id.cmp(&b.client_id)));
    Ok((clients, by_thread))
}

// ---- HTTP ----

pub fn routes() -> Router<ApiState> {
    Router::new().route("/costs", get(get_costs))
}

#[derive(Deserialize)]
struct CostQuery {
    since: Option<String>,
    days: Option<i64>,
}

async fn get_costs(State(s): State<ApiState>, Query(q): Query<CostQuery>) -> Result<Json<Report>, (StatusCode, Json<Value>)> {
    let fail = |code, e: anyhow::Error| (code, Json(json!({ "error": format!("{e:#}") })));
    let since = window_start(q.since.as_deref(), q.days).map_err(|e| fail(StatusCode::BAD_REQUEST, e))?;
    let report = report(&s.db, &s.data_dir, &since).map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::NewTask;
    use rui_core::{Role, SourceKind, TaskStatus};

    fn task(db: &Db, client: i64, parent: Option<i64>, title: &str) -> i64 {
        db.create_task(&NewTask {
            client_id: client, parent_id: parent, title: title.into(), body: String::new(), status: TaskStatus::Done,
            done_command: None, max_loops: 3, role: Role::Lead, source_kind: SourceKind::User, source_ref: None, agent: None, pod: vec![],
        })
        .unwrap()
        .id
    }

    fn spend(db: &Db, task: Option<i64>, usd: f64, seat: &str, usage: Value) {
        db.add_event(task, "lead", "result", &json!({"total_cost_usd": usd, "seat_id": seat, "usage": usage})).unwrap();
    }

    #[test]
    fn rolls_subtasks_into_threads_and_sums_tokens() {
        let (db, data) = (Db::open_test(), tempfile::tempdir().unwrap());
        let seats = json!([{"id": "work", "kind": "subscription", "label": "W"}, {"id": "key", "kind": "api_key", "label": "K", "env_var": "X"}]);
        std::fs::write(data.path().join("seats.json"), seats.to_string()).unwrap();
        let (a, b) = (db.create_client("acme", "/a", None, &[]).unwrap().id, db.create_client("beta", "/b", None, &[]).unwrap().id);
        let top = task(&db, a, None, "Login");
        let sub = task(&db, a, Some(top), "Form");
        let deep = task(&db, a, Some(sub), "Field");
        let other = task(&db, b, None, "Docs");
        let u = |i: i64, o: i64, cr: i64, cc: i64| json!({"input_tokens": i, "output_tokens": o, "cache_read_input_tokens": cr, "cache_creation_input_tokens": cc});
        spend(&db, Some(top), 0.5, "work", u(10, 20, 30, 40));
        spend(&db, Some(sub), 0.25, "work", u(1, 2, 0, 0));
        spend(&db, Some(deep), 0.25, "key", json!({"input_tokens": 5, "output_tokens": 5}));
        spend(&db, Some(other), 2.0, "key", Value::Null);
        spend(&db, None, 0.1, "work", u(1, 1, 1, 1));
        db.add_event(Some(top), "lead", "assistant", &json!({"total_cost_usd": 9.0})).unwrap();

        let r = report(&db, data.path(), &window_start(None, Some(1)).unwrap()).unwrap();
        assert!((r.total_usd - 3.1).abs() < 1e-9);
        assert_eq!(r.total_tokens, 100 + 3 + 10 + 4);
        let t: Vec<(i64, &str, i64)> = r.by_thread.iter().map(|t| (t.task_id, t.client.as_str(), t.tokens)).collect();
        assert_eq!(t, [(other, "beta", 0), (top, "acme", 113)]);
        assert!((r.by_thread[1].usd - 1.0).abs() < 1e-9, "subtask spend rolls up into the thread");
        let c: Vec<(&str, usize)> = r.by_client.iter().map(|c| (c.name.as_str(), c.tasks)).collect();
        assert_eq!(c, [("beta", 1), ("acme", 1)]);
        let s: Vec<(&str, bool, i64)> = r.by_seat.iter().map(|s| (s.seat_id.as_str(), s.estimate, s.tokens)).collect();
        assert_eq!(s, [("key", false, 10), ("work", true, 107)]);
        assert_eq!((r.by_day.len(), r.by_day[0].tokens), (1, 117));
    }

    #[test]
    fn window_respects_since_and_validates() {
        let (db, data) = (Db::open_test(), tempfile::tempdir().unwrap());
        spend(&db, None, 1.0, "default", Value::Null);
        assert_eq!(report(&db, data.path(), &window_start(Some("2999-01-01T00:00:00Z"), None).unwrap()).unwrap().total_usd, 0.0);
        let all = report(&db, data.path(), &window_start(Some("2000-01-01T00:00:00+02:00"), None).unwrap()).unwrap();
        assert_eq!((all.total_usd, all.by_seat[0].estimate), (1.0, true));
        assert!(window_start(Some("yesterday"), None).is_err());
        assert!(window_start(None, Some(0)).is_err());
    }

    #[tokio::test]
    async fn http_reports_and_rejects_a_bad_window() {
        use http_body_util::BodyExt;
        use tower::ServiceExt;
        let data = tempfile::tempdir().unwrap();
        let (chat_tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let db = std::sync::Arc::new(Db::open_test());
        spend(&db, None, 0.5, "default", json!({"input_tokens": 3}));
        let state = ApiState {
            db, chat_tx, seat: Default::default(), runs: Default::default(), live: Default::default(),
            data_dir: data.path().to_path_buf(), apps: Default::default(), owner_token: crate::owner_auth::test_token(),
        };
        let app = crate::api::router(state);
        let get = |uri: &str| axum::http::Request::get(uri).header(crate::owner_auth::HEADER, "test-owner-token").body(axum::body::Body::empty()).unwrap();
        let resp = app.clone().oneshot(get("/costs?days=7")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v: Value = serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!((v["total_usd"].as_f64(), v["total_tokens"].as_i64()), (Some(0.5), Some(3)));
        assert_eq!(app.oneshot(get("/costs?since=soon")).await.unwrap().status(), StatusCode::BAD_REQUEST);
    }
}

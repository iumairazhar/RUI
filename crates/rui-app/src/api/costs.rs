//! Daemon calls for spend (`GET /costs`) and deploys (the Inbox buttons and the client's deploy config).

use super::{get, library};
use anyhow::Result;
use serde::Deserialize;
use serde_json::json;

/// Spend of one client in the window.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ClientCost {
    pub client_id: i64,
    pub name: String,
    pub usd: f64,
    pub tokens: i64,
    #[serde(default)]
    pub tasks: usize,
}

/// Spend of one top-level thread, subtasks rolled in.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ThreadCost {
    pub task_id: i64,
    pub title: String,
    #[serde(default)]
    pub client: String,
    pub usd: f64,
    pub tokens: i64,
}

/// Spend on one day, `date` as "YYYY-MM-DD".
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct DayCost {
    pub date: String,
    pub usd: f64,
    pub tokens: i64,
}

/// Spend on one seat; `estimate` is true on subscription seats.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct SeatCost {
    pub seat_id: String,
    pub usd: f64,
    pub tokens: i64,
    #[serde(default)]
    pub estimate: bool,
}

/// The whole `GET /costs` reply.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Costs {
    pub total_usd: f64,
    pub total_tokens: i64,
    #[serde(default)]
    pub by_client: Vec<ClientCost>,
    #[serde(default)]
    pub by_thread: Vec<ThreadCost>,
    #[serde(default)]
    pub by_day: Vec<DayCost>,
    #[serde(default)]
    pub by_seat: Vec<SeatCost>,
}

/// Spend over the last `days` days.
pub fn get_costs(days: u32) -> Result<Costs> {
    Ok(serde_json::from_value(get(&format!("/costs?days={days}"))?)?)
}

/// Deploy or skip a requested deploy: `action` is "deploy" or "skip". Errors carry the daemon's text (409 included).
pub fn deploy_action(event_id: i64, action: &str) -> Result<()> {
    library("POST", &format!("/deploys/{event_id}"), Some(json!({ "action": action }))).map(|_| ())
}

/// Set a client's deploy command and environment ("production" or "staging").
pub fn set_deploy_config(client_id: i64, command: &str, environment: &str) -> Result<()> {
    let body = json!({ "command": command, "environment": environment });
    library("POST", &format!("/clients/{client_id}/deploy-config"), Some(body)).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_costs() {
        let j = r#"{"since":"x","total_usd":1.5,"total_tokens":900,
            "by_client":[{"client_id":1,"name":"Acme","usd":1.5,"tokens":900,"tasks":2}],
            "by_thread":[{"task_id":7,"title":"Fix","client":"Acme","usd":1.5,"tokens":900}],
            "by_day":[{"date":"2026-11-18","usd":1.5,"tokens":900}],
            "by_seat":[{"seat_id":"work","usd":1.5,"tokens":900,"estimate":true}]}"#;
        let c: Costs = serde_json::from_str(j).unwrap();
        assert_eq!((c.by_client[0].tasks, c.by_thread[0].task_id, c.by_seat[0].estimate), (2, 7, true));
        assert_eq!(c.by_day[0].date, "2026-11-18");
    }
}

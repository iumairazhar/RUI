//! Daemon calls for seats, the Scout and Coach, task limits and email inputs.

use super::{get, library, post};
use anyhow::Result;
use serde::Deserialize;
use serde_json::{Value, json};

/// One seat with its usage, from `GET /seats`. Key values are never sent; only the variable's name.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SeatStatus {
    pub id: String,
    /// "subscription" or "api_key".
    pub kind: String,
    pub label: String,
    #[serde(default)]
    pub config_dir: Option<String>,
    #[serde(default)]
    pub env_var: Option<String>,
    #[serde(default)]
    pub budget_usd: Option<f64>,
    #[serde(default)]
    pub window_hours: u32,
    #[serde(default)]
    pub order: i32,
    #[serde(default)]
    pub used_usd: f64,
    /// Share of the budget used, 0 to 100 (and past 100 when over).
    #[serde(default)]
    pub used_pct: Option<f64>,
    /// Unix seconds when a hit limit stops being in force.
    #[serde(default)]
    pub exhausted_until: Option<i64>,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub env_var_present: bool,
}

/// Fetch every seat with its usage.
pub fn get_seats() -> Result<Vec<SeatStatus>> {
    Ok(serde_json::from_value(get("/seats")?)?)
}

/// Add a seat, or replace the one with the same id. Errors carry the daemon's text.
pub fn save_seat(body: Value) -> Result<Value> {
    library("POST", "/seats", Some(body))
}

/// Remove a seat.
pub fn delete_seat(id: &str) -> Result<Value> {
    library("DELETE", &format!("/seats/{id}"), None)
}

/// Ask the Scout why a task failed or stalled; the report lands in the Inbox.
pub fn diagnose(task_id: i64) -> Result<()> {
    post(&format!("/tasks/{task_id}/diagnose"), json!({}))
}

/// Approve or dismiss a Coach proposal: `verdict` is "approve" or "dismiss".
pub fn resolve_proposal(event_id: i64, verdict: &str) -> Result<()> {
    post(&format!("/proposals/{event_id}"), json!({ "verdict": verdict }))
}

/// Clear a Scout report from the Inbox.
pub fn dismiss_scout_report(event_id: i64) -> Result<()> {
    post(&format!("/scout-reports/{event_id}/dismiss"), json!({}))
}

/// Clear a "role added to the Pod" note from the Inbox.
pub fn dismiss_role_added(event_id: i64) -> Result<()> {
    post(&format!("/role-added/{event_id}/dismiss"), json!({}))
}

/// Clear a "the Lead replied" note from the Inbox.
pub fn dismiss_lead_reply(event_id: i64) -> Result<()> {
    post(&format!("/lead-replies/{event_id}/dismiss"), json!({}))
}

/// Give a task a new attempt limit.
pub fn set_limit(task_id: i64, max_loops: i64) -> Result<()> {
    post(&format!("/tasks/{task_id}/limit"), json!({ "max_loops": max_loops }))
}

/// Path segment under `/inputs/` for email items.
pub const EMAIL_INPUT: &str = "email";

/// Act on an input item: `POST /inputs/{input}/{event_id}` with `{action}` ("make_task" or "dismiss").
pub fn input_action(input: &str, event_id: i64, action: &str) -> Result<()> {
    post(&format!("/inputs/{input}/{event_id}"), json!({ "action": action }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_seats() {
        let j = r#"[{"id":"w","kind":"api_key","label":"Work","env_var":"K","budget_usd":20.0,"window_hours":5,
            "order":1,"used_usd":3.5,"used_pct":17.5,"exhausted_until":null,"active":true,"env_var_present":false}]"#;
        let v: Vec<SeatStatus> = serde_json::from_str(j).unwrap();
        assert_eq!((v[0].kind.as_str(), v[0].used_pct, v[0].env_var_present), ("api_key", Some(17.5), false));
    }
}

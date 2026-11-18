//! Daemon calls for a client's branch rules (`/clients/{id}/branches`) and the Claude limits (`GET /limits`).

use super::{get, library};
use anyhow::Result;
use serde::Deserialize;
use serde_json::json;

/// Where agents may land and push for one client. `base` is the resolved base; `base_set` the owner's pick.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Branches {
    #[serde(default)]
    pub base: String,
    #[serde(default)]
    pub base_set: Option<String>,
    #[serde(default)]
    pub allow_land: bool,
    #[serde(default)]
    pub allow_push: bool,
    #[serde(default)]
    pub protected: Vec<String>,
}

/// A client's branch rules.
pub fn get_branches(client_id: i64) -> Result<Branches> {
    Ok(serde_json::from_value(library("GET", &format!("/clients/{client_id}/branches"), None)?)?)
}

/// Save a client's branch rules; `base` `None` means auto-detect. Errors carry the daemon's `{error}` text.
pub fn set_branches(client_id: i64, base: Option<&str>, allow_land: bool, allow_push: bool, protected: &[String]) -> Result<Branches> {
    let body = json!({ "base": base, "allow_land": allow_land, "allow_push": allow_push, "protected": protected });
    Ok(serde_json::from_value(library("POST", &format!("/clients/{client_id}/branches"), Some(body))?)?)
}

/// One rate-limit window of a seat, like "five_hour" or "seven_day".
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct LimitWindow {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub used_pct: f64,
    #[serde(default)]
    pub left_pct: f64,
    /// Unix seconds when the window resets.
    #[serde(default)]
    pub resets_at: Option<i64>,
    #[serde(default)]
    pub status: String,
}

/// Whether the seat is spending past its plan.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Overage {
    #[serde(default)]
    pub using: bool,
    #[serde(default)]
    pub status: String,
}

/// The last limits Claude reported for one seat.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct SeatLimits {
    #[serde(default)]
    pub seat_id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub windows: Vec<LimitWindow>,
    #[serde(default)]
    pub overage: Overage,
    /// Unix seconds of the report.
    #[serde(default)]
    pub seen_at: Option<i64>,
    #[serde(default)]
    pub stale: bool,
}

/// The limits of every seat that has reported them.
pub fn get_limits() -> Result<Vec<SeatLimits>> {
    Ok(serde_json::from_value(get("/limits")?)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_branches_and_limits_with_missing_fields() {
        let b: Branches = serde_json::from_str(r#"{"base":"main","base_set":null,"allow_land":true,"protected":["prod"]}"#).unwrap();
        assert_eq!((b.base.as_str(), b.base_set, b.allow_land, b.allow_push), ("main", None, true, false));
        let j = r#"[{"seat_id":"w","label":"Work","windows":[{"name":"five_hour","used_pct":46,"left_pct":54,"resets_at":100,"status":"allowed"}],"overage":{"using":false,"status":""},"seen_at":90,"stale":true}]"#;
        let l: Vec<SeatLimits> = serde_json::from_str(j).unwrap();
        assert_eq!((l[0].windows[0].left_pct, l[0].windows[0].resets_at, l[0].stale), (54.0, Some(100), true));
    }
}

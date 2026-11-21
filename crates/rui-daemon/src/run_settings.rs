//! Which model and effort an agent run uses.
//!
//! The Lead picks them per subtask (`create_subtask`, `send_back`, `set_run_settings`); each pick is
//! stored as a `run_settings` event on the subtask. A role's AGENT.md may set defaults with
//! `model:` and `effort:`. Resolution for a member run, per field: the latest `run_settings` event,
//! then the role's default, then unset (the CLI's own default).

use crate::db::Db;
use anyhow::{Result, bail};
use serde::Serialize;
use serde_json::{Value, json};
use std::path::Path;

/// Model aliases the Claude CLI accepts with `--model`.
pub const MODELS: [&str; 3] = ["haiku", "sonnet", "opus"];
/// Levels the Claude CLI accepts with `--effort`.
pub const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
/// Event kind that stores a pick on a subtask.
pub const EVENT: &str = "run_settings";

/// A model and effort; `None` leaves the CLI's default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RunSettings {
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// `v` trimmed and lowercased; None when empty. Errors when it is not in `allowed`.
fn pick(v: Option<&str>, allowed: &[&str], what: &str) -> Result<Option<String>> {
    let Some(v) = v.map(|v| v.trim().to_ascii_lowercase()).filter(|v| !v.is_empty()) else { return Ok(None) };
    if !allowed.contains(&v.as_str()) {
        bail!("{what} must be one of {}, not '{v}'", allowed.join(", "));
    }
    Ok(Some(v))
}

/// The one place model and effort values are checked.
pub fn check(model: Option<&str>, effort: Option<&str>) -> Result<RunSettings> {
    Ok(RunSettings { model: pick(model, &MODELS, "model")?, effort: pick(effort, &EFFORTS, "effort")? })
}

impl RunSettings {
    pub fn is_empty(&self) -> bool {
        self.model.is_none() && self.effort.is_none()
    }

    /// Each unset field taken from `other`.
    pub fn or(self, other: RunSettings) -> RunSettings {
        RunSettings { model: self.model.or(other.model), effort: self.effort.or(other.effort) }
    }

    fn of(payload: &Value) -> RunSettings {
        let s = |k: &str| payload[k].as_str().map(String::from);
        RunSettings { model: s("model"), effort: s("effort") }
    }
}

/// Stores `s` on `task_id`; nothing when `s` is empty. `why` is kept when given.
pub fn record(db: &Db, task_id: i64, s: &RunSettings, by: &str, why: Option<&str>) -> Result<()> {
    if s.is_empty() {
        return Ok(());
    }
    let mut payload = json!({"model": s.model, "effort": s.effort, "by": by});
    if let Some(w) = why {
        payload["why"] = json!(w);
    }
    db.add_event(Some(task_id), by, EVENT, &payload)?;
    Ok(())
}

/// The latest pick stored on `task_id`, or empty.
pub fn latest(db: &Db, task_id: i64) -> Result<RunSettings> {
    let events = db.events_for_tasks(&[task_id], &[EVENT], 0, i64::MAX)?;
    Ok(events.last().map(|e| RunSettings::of(&e.payload)).unwrap_or_default())
}

/// The defaults in role `role`'s AGENT.md, or empty.
pub fn role_defaults(data_dir: &Path, role: &str) -> RunSettings {
    crate::roster::load(data_dir, role).map(|r| RunSettings { model: r.model, effort: r.effort }).unwrap_or_default()
}

/// A member run of subtask `task_id` with role `role`: its latest pick, then the role's defaults.
pub fn for_member(db: &Db, data_dir: &Path, task_id: i64, role: Option<&str>) -> Result<RunSettings> {
    let role = role.map(|r| role_defaults(data_dir, r)).unwrap_or_default();
    Ok(latest(db, task_id)?.or(role))
}

/// An Account Lead turn: the "lead" role's defaults.
pub fn for_lead(data_dir: &Path) -> RunSettings {
    role_defaults(data_dir, "lead")
}

/// A planning-meeting or Scout turn: the role's defaults, else sonnet at medium effort.
pub fn for_support(data_dir: &Path, role: &str) -> RunSettings {
    role_defaults(data_dir, role).or(fixed("sonnet", Some("medium")))
}

/// A Coach turn: sonnet, effort left to the CLI.
pub fn for_coach() -> RunSettings {
    fixed("sonnet", None)
}

fn fixed(model: &str, effort: Option<&str>) -> RunSettings {
    RunSettings { model: Some(model.into()), effort: effort.map(String::from) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data_with_role(name: &str, front: &str) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("roles").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("AGENT.md"), format!("---\nname: {name}\ndescription: d\n{front}---\nbody\n")).unwrap();
        d
    }

    fn task(db: &Db) -> i64 {
        let c = db.create_client("acme", "/tmp/acme", None, &[]).unwrap();
        let t = crate::db::NewTask {
            client_id: c.id, parent_id: None, title: "t".into(), body: String::new(), status: rui_core::TaskStatus::Open,
            done_command: None, max_loops: 5, role: rui_core::Role::Builder, source_kind: rui_core::SourceKind::User,
            source_ref: None, agent: None, pod: vec![],
        };
        db.create_task(&t).unwrap().id
    }

    #[test]
    fn check_accepts_known_values_and_rejects_others() {
        let s = check(Some(" Opus "), Some("XHIGH")).unwrap();
        assert_eq!((s.model.as_deref(), s.effort.as_deref()), (Some("opus"), Some("xhigh")));
        assert!(check(None, Some("")).unwrap().is_empty());
        assert!(check(Some("gpt-5"), None).is_err());
        assert!(check(None, Some("extreme")).is_err());
        assert!(check(Some("claude-opus"), Some("low")).is_err());
    }

    #[test]
    fn record_skips_empty_and_latest_wins() {
        let db = Db::open_test();
        let id = task(&db);
        record(&db, id, &RunSettings::default(), "lead", None).unwrap();
        assert!(latest(&db, id).unwrap().is_empty());
        record(&db, id, &check(Some("haiku"), Some("low")).unwrap(), "lead", None).unwrap();
        record(&db, id, &check(Some("opus"), None).unwrap(), "lead", Some("tricky")).unwrap();
        assert_eq!(latest(&db, id).unwrap(), fixed("opus", None));
    }

    #[test]
    fn member_resolution_is_event_then_role_then_none() {
        let db = Db::open_test();
        let id = task(&db);
        let data = data_with_role("backend", "model: sonnet\neffort: high\n");
        assert!(for_member(&db, data.path(), id, None).unwrap().is_empty());
        assert!(for_member(&db, data.path(), id, Some("ghost")).unwrap().is_empty());
        assert_eq!(for_member(&db, data.path(), id, Some("backend")).unwrap(), fixed("sonnet", Some("high")));
        record(&db, id, &check(Some("opus"), None).unwrap(), "lead", None).unwrap();
        assert_eq!(for_member(&db, data.path(), id, Some("backend")).unwrap(), fixed("opus", Some("high")));
    }

    #[test]
    fn lead_support_and_coach_defaults() {
        let empty = tempfile::tempdir().unwrap();
        assert!(for_lead(empty.path()).is_empty());
        assert_eq!(for_support(empty.path(), "backend"), fixed("sonnet", Some("medium")));
        assert_eq!(for_coach(), fixed("sonnet", None));
        let lead = data_with_role("lead", "model: opus\n");
        assert_eq!(for_lead(lead.path()), fixed("opus", None));
        let be = data_with_role("backend", "effort: low\n");
        assert_eq!(for_support(be.path(), "backend"), fixed("sonnet", Some("low")));
    }
}

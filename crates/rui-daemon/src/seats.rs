//! Seats: the Claude logins and API keys the daemon can run turns on, kept in `<data_dir>/seats.json`.
//! An api_key seat stores only the NAME of an environment variable, never the key itself.

use crate::db::Db;
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};

/// Share of the budget at which the next seat takes over.
pub const SWITCH_AT: f64 = 0.9;
/// Phrases in a failed turn that mean the seat hit a usage or rate limit.
const LIMIT_PHRASES: [&str; 5] = ["usage limit", "rate limit", "rate_limit", "limit reached", "credit balance is too low"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeatKind {
    Subscription,
    ApiKey,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Seat {
    pub id: String,
    pub kind: SeatKind,
    pub label: String,
    /// Subscription only: a CLAUDE_CONFIG_DIR for a second login; None means the default login.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_dir: Option<String>,
    /// Api key only: the environment variable that holds the key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_var: Option<String>,
    /// Spend allowed per window; None (or 0) means no limit.
    #[serde(default)]
    pub budget_usd: Option<f64>,
    #[serde(default = "default_window")]
    pub window_hours: u32,
    #[serde(default)]
    pub order: i32,
}

fn default_window() -> u32 {
    5
}

/// The seat used when seats.json is missing: the default login, no budget.
pub fn default_seat() -> Seat {
    Seat {
        id: "default".into(),
        kind: SeatKind::Subscription,
        label: "Default login".into(),
        config_dir: None,
        env_var: None,
        budget_usd: None,
        window_hours: default_window(),
        order: 0,
    }
}

fn path(data_dir: &Path) -> PathBuf {
    data_dir.join("seats.json")
}

/// All seats; a missing or empty file means the one default seat.
pub fn load(data_dir: &Path) -> Result<Vec<Seat>> {
    let p = path(data_dir);
    if !p.exists() {
        return Ok(vec![default_seat()]);
    }
    let seats: Vec<Seat> = serde_json::from_str(&std::fs::read_to_string(p)?)?;
    if seats.is_empty() {
        return Ok(vec![default_seat()]);
    }
    Ok(seats)
}

fn save(data_dir: &Path, seats: &[Seat]) -> Result<()> {
    std::fs::create_dir_all(data_dir)?;
    Ok(std::fs::write(path(data_dir), serde_json::to_string_pretty(seats)?)?)
}

/// Adds the seat, or replaces the one with the same id.
pub fn upsert(data_dir: &Path, seat: Seat) -> Result<()> {
    validate(&seat)?;
    let mut seats = load(data_dir)?;
    seats.retain(|s| s.id != seat.id);
    seats.push(seat);
    save(data_dir, &seats)
}

/// Removes a seat by id; false when there was none.
pub fn remove(data_dir: &Path, id: &str) -> Result<bool> {
    let mut seats = load(data_dir)?;
    let before = seats.len();
    seats.retain(|s| s.id != id);
    if seats.len() == before {
        return Ok(false);
    }
    save(data_dir, &seats)?;
    Ok(true)
}

/// True for names like `ANTHROPIC_API_KEY_WORK`.
pub fn is_env_name(s: &str) -> bool {
    let mut chars = s.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    first_ok && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_id(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Checks a seat's fields before it is saved.
pub fn validate(s: &Seat) -> Result<()> {
    if !is_id(&s.id) {
        bail!("id must be letters, digits, '-' or '_'");
    }
    if s.label.trim().is_empty() {
        bail!("label must not be empty");
    }
    if s.window_hours == 0 || s.budget_usd.is_some_and(|b| !b.is_finite() || b < 0.0) {
        bail!("window_hours must be at least 1 and budget_usd must not be negative");
    }
    match s.kind {
        SeatKind::Subscription => validate_subscription(s),
        SeatKind::ApiKey => validate_api_key(s),
    }
}

fn validate_subscription(s: &Seat) -> Result<()> {
    if s.env_var.is_some() {
        bail!("a subscription seat has no env_var");
    }
    let bad_dir = s.config_dir.as_deref().is_some_and(|d| !(d.starts_with('/') || d.starts_with('~')));
    if bad_dir {
        bail!("config_dir must be an absolute path or start with ~");
    }
    Ok(())
}

fn validate_api_key(s: &Seat) -> Result<()> {
    if s.config_dir.is_some() {
        bail!("an api_key seat has no config_dir");
    }
    if !s.env_var.as_deref().is_some_and(is_env_name) {
        bail!("env_var must be an environment variable name like ANTHROPIC_API_KEY_WORK");
    }
    Ok(())
}

/// True when the seat's key can be read now (always true for subscriptions).
pub fn env_var_present(s: &Seat) -> bool {
    match (&s.kind, &s.env_var) {
        (SeatKind::ApiKey, Some(name)) => std::env::var(name).is_ok_and(|v| !v.is_empty()),
        (SeatKind::ApiKey, None) => false,
        (SeatKind::Subscription, _) => true,
    }
}

/// Names of the secret variables no agent or done command may inherit: ANTHROPIC_API_KEY and
/// every seat's env_var from seats.json.
pub fn secret_env_names(data_dir: &Path) -> Vec<String> {
    let mut names = vec!["ANTHROPIC_API_KEY".to_string()];
    names.extend(load(data_dir).unwrap_or_default().into_iter().filter_map(|s| s.env_var));
    names
}

/// Removes every secret variable from a child process's environment.
pub fn scrub_env(cmd: &mut std::process::Command, data_dir: &Path) {
    for name in secret_env_names(data_dir) {
        cmd.env_remove(name);
    }
}

/// Environment the CLI process runs with for this seat. The key is read here, at run time.
pub fn seat_env(s: &Seat) -> Result<Vec<(String, String)>> {
    match s.kind {
        SeatKind::Subscription => Ok(s
            .config_dir
            .iter()
            .map(|d| ("CLAUDE_CONFIG_DIR".to_string(), crate::scheduler::expand_home(d).to_string_lossy().into_owned()))
            .collect()),
        SeatKind::ApiKey => {
            let name = s.env_var.as_deref().unwrap_or_default();
            let Ok(key) = std::env::var(name).map(|v| v.trim().to_string()) else {
                bail!("seat {}: environment variable {name} is not set", s.id);
            };
            Ok(vec![("ANTHROPIC_API_KEY".to_string(), key)])
        }
    }
}

fn iso(secs: i64) -> String {
    let t = chrono::DateTime::from_timestamp(secs, 0).unwrap_or_default();
    t.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

fn secs_of(ts: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(ts).ok().map(|d| d.timestamp())
}

fn window_secs(s: &Seat) -> i64 {
    i64::from(s.window_hours) * 3600
}

/// Spend on this seat inside its window, and when the oldest of those turns ran.
fn usage(db: &Db, s: &Seat, now: i64) -> Result<(f64, Option<i64>)> {
    let (used, oldest) = db.seat_usage_since(&s.id, &iso(now - window_secs(s)))?;
    Ok((used, oldest.as_deref().and_then(secs_of)))
}

/// Spend on this seat inside its window.
pub fn used_usd(db: &Db, s: &Seat, now: i64) -> Result<f64> {
    Ok(usage(db, s, now)?.0)
}

fn budget(s: &Seat) -> Option<f64> {
    s.budget_usd.filter(|b| *b > 0.0)
}

/// Used share of the budget, 0 to 100+; None without a budget.
pub fn used_pct(s: &Seat, used: f64) -> Option<f64> {
    budget(s).map(|b| used / b * 100.0)
}

fn over_budget(s: &Seat, used: f64) -> bool {
    budget(s).is_some_and(|b| used >= b * SWITCH_AT)
}

/// When a hit limit stops being in force, if it still is at `now`.
pub fn exhausted_until(db: &Db, seat_id: &str, now: i64) -> Result<Option<i64>> {
    let evs = db.events_of_kinds(&["seat_exhausted"])?;
    let mine = evs.iter().filter(|e| e.payload["seat_id"].as_str() == Some(seat_id));
    let until = mine.filter_map(|e| e.payload["until"].as_i64()).max();
    Ok(until.filter(|u| *u > now))
}

/// Records that `seat` hit a usage or rate limit; it rests for one window.
pub fn mark_exhausted(db: &Db, s: &Seat, now: i64) -> Result<()> {
    let payload = json!({"seat_id": s.id, "until": now + window_secs(s)});
    db.add_event(None, "scheduler", "seat_exhausted", &payload)?;
    Ok(())
}

/// True when this text (a failed turn's result or error) says a limit was hit.
pub fn hit_limit(text: &str) -> bool {
    let lower = text.to_lowercase();
    LIMIT_PHRASES.iter().any(|p| lower.contains(p))
}

fn usable(db: &Db, s: &Seat, now: i64) -> Result<bool> {
    if exhausted_until(db, &s.id, now)?.is_some() || over_budget(s, used_usd(db, s, now)?) {
        return Ok(false);
    }
    if !env_var_present(s) {
        eprintln!("seat {}: environment variable {} is not set, skipping", s.id, s.env_var.as_deref().unwrap_or("?"));
        return Ok(false);
    }
    Ok(true)
}

/// The seat to run on: lowest `order` that is under 90% of its budget and not resting. None when all are used up.
pub fn pick_seat(db: &Db, seats: &[Seat], now: i64) -> Result<Option<Seat>> {
    let mut sorted = seats.to_vec();
    sorted.sort_by_key(|s| s.order);
    for s in sorted {
        if usable(db, &s, now)? {
            return Ok(Some(s));
        }
    }
    Ok(None)
}

/// `pick_seat` over the seats in the data dir.
pub fn pick_from(db: &Db, data_dir: &Path, now: i64) -> Result<Option<Seat>> {
    pick_seat(db, &load(data_dir)?, now)
}

fn clock(secs: i64) -> String {
    let t = chrono::DateTime::from_timestamp(secs, 0).unwrap_or_default();
    t.format("%Y-%m-%d %H:%M UTC").to_string()
}

/// One line saying why a seat is out and when it frees up.
fn seat_line(db: &Db, s: &Seat, now: i64) -> Result<String> {
    if let Some(u) = exhausted_until(db, &s.id, now)? {
        return Ok(format!("{}: limit hit, frees up {}", s.label, clock(u)));
    }
    if !env_var_present(s) {
        return Ok(format!("{}: {} is not set", s.label, s.env_var.as_deref().unwrap_or("env_var")));
    }
    let (_, oldest) = usage(db, s, now)?;
    let when = oldest.map(|o| clock(o + window_secs(s))).unwrap_or_else(|| "soon".into());
    Ok(format!("{}: over 90% of budget, frees up from {when}", s.label))
}

/// Every seat and when it frees up, one per line.
pub fn exhausted_reason(db: &Db, seats: &[Seat], now: i64) -> Result<String> {
    let lines: Result<Vec<String>> = seats.iter().map(|s| seat_line(db, s, now)).collect();
    Ok(lines?.join("\n"))
}

/// One seat as the API shows it. Never carries a key value.
#[derive(Debug, Clone, Serialize)]
pub struct SeatStatus {
    pub id: String,
    pub kind: SeatKind,
    pub label: String,
    pub config_dir: Option<String>,
    pub env_var: Option<String>,
    pub budget_usd: Option<f64>,
    pub window_hours: u32,
    pub order: i32,
    pub used_usd: f64,
    pub used_pct: Option<f64>,
    pub exhausted_until: Option<i64>,
    pub active: bool,
    pub env_var_present: bool,
}

fn status(db: &Db, s: Seat, active: Option<&str>, now: i64) -> Result<SeatStatus> {
    let used = used_usd(db, &s, now)?;
    Ok(SeatStatus {
        used_pct: used_pct(&s, used),
        exhausted_until: exhausted_until(db, &s.id, now)?,
        active: active == Some(s.id.as_str()),
        env_var_present: env_var_present(&s),
        used_usd: used,
        id: s.id,
        kind: s.kind,
        label: s.label,
        config_dir: s.config_dir,
        env_var: s.env_var,
        budget_usd: s.budget_usd,
        window_hours: s.window_hours,
        order: s.order,
    })
}

/// Every seat with its usage, sorted by order.
pub fn list_status(db: &Db, data_dir: &Path, now: i64) -> Result<Vec<SeatStatus>> {
    let mut seats = load(data_dir)?;
    seats.sort_by_key(|s| s.order);
    let active = pick_seat(db, &seats, now)?.map(|s| s.id);
    seats.into_iter().map(|s| status(db, s, active.as_deref(), now)).collect()
}

/// Logs `seats_exhausted` once when every seat is used up and `seats_freed` once one frees up.
/// Returns true when a seat is free, so new turns may start.
pub fn check_gate(db: &Db, data_dir: &Path, now: i64) -> Result<bool> {
    let seats = load(data_dir)?;
    let free = pick_seat(db, &seats, now)?.is_some();
    let flagged = latest_gate_event(db)?.is_some_and(|e| e.kind == "seats_exhausted");
    if !free && !flagged {
        let reason = exhausted_reason(db, &seats, now)?;
        db.add_event(None, "scheduler", "seats_exhausted", &json!({"reason": reason}))?;
    }
    if free && flagged {
        db.add_event(None, "scheduler", "seats_freed", &json!({}))?;
    }
    Ok(free)
}

/// The newest `seats_exhausted` or `seats_freed` event.
pub fn latest_gate_event(db: &Db) -> Result<Option<rui_core::Event>> {
    Ok(db.events_of_kinds(&["seats_exhausted", "seats_freed"])?.into_iter().last())
}

/// Claude usage-limit windows from a `rate_limit_event` stream line; empty for any other line.
pub fn parse_limits(line: &serde_json::Value) -> Vec<rui_core::LimitWindow> {
    if line["type"] != "rate_limit_event" {
        return vec![];
    }
    let info = &line["rate_limit_info"];
    let window = |name: &str, v: &serde_json::Value| rui_core::LimitWindow {
        name: name.to_string(),
        used_pct: v["utilization"].as_f64().map(|u| (u * 100.0).clamp(0.0, 100.0)),
        resets_at: v["resetsAt"].as_i64(),
        status: v["status"].as_str().map(String::from),
    };
    if let Some(all) = info["unifiedWindows"].as_object().filter(|m| !m.is_empty()) {
        return all.iter().map(|(k, v)| window(k, v)).collect();
    }
    info["rateLimitType"].as_str().map(|n| window(n, info)).into_iter().collect()
}

static LATEST_LIMITS: std::sync::Mutex<Option<(Vec<rui_core::LimitWindow>, i64)>> = std::sync::Mutex::new(None);

/// Remembers the limits of a stream line, if it carries any.
pub fn record_limits(line: &serde_json::Value, now: i64) {
    let w = parse_limits(line);
    if !w.is_empty() {
        *LATEST_LIMITS.lock().unwrap_or_else(|e| e.into_inner()) = Some((w, now));
    }
}

/// The newest limits: from memory, else from the newest stored rate-limit event (after a restart).
pub fn current_limits(db: &Db) -> Result<(Vec<rui_core::LimitWindow>, Option<i64>)> {
    if let Some((w, at)) = LATEST_LIMITS.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        return Ok((w, Some(at)));
    }
    let found = db.list_events(None, 2000)?.into_iter().rev().find_map(|e| {
        let w = if e.kind == "other" { parse_limits(&e.payload) } else { vec![] };
        (!w.is_empty()).then(|| (w, secs_of(&e.created_at)))
    });
    let Some((w, at)) = found else { return Ok((vec![], None)) };
    if let Some(at) = at {
        *LATEST_LIMITS.lock().unwrap_or_else(|e| e.into_inner()) = Some((w.clone(), at));
    }
    Ok((w, at))
}

/// Fresh limits are older than this: check again.
const LIMITS_STALE_SECS: i64 = 20 * 60;
/// Never run the check more often than this.
const PROBE_GAP_SECS: i64 = 10 * 60;

/// True when the limits should be checked: none known, a window's reset has passed,
/// or the newest numbers are older than LIMITS_STALE_SECS.
pub fn limits_stale(limits: &[rui_core::LimitWindow], seen_at: Option<i64>, now: i64) -> bool {
    let Some(seen) = seen_at else { return true };
    let reset_passed = limits.iter().any(|w| w.resets_at.is_some_and(|at| at <= now));
    reset_passed || now - seen > LIMITS_STALE_SECS
}

/// Every minute: when the limits are stale, run a tiny Claude turn that only reports them.
/// One check costs a fraction of a cent of the 5-hour limit; it runs at most every 10 minutes.
pub async fn probe_loop(db: std::sync::Arc<Db>) {
    let mut last_probe = 0;
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        let now = crate::thread::now_secs();
        let Ok((limits, seen)) = current_limits(&db) else { continue };
        if now - last_probe < PROBE_GAP_SECS || !limits_stale(&limits, seen, now) {
            continue;
        }
        last_probe = now;
        probe_limits().await;
    }
}

/// Runs `claude -p` once with no tools and no MCP servers and records the rate-limit line it sends.
async fn probe_limits() {
    let bin = std::env::var("RUI_CLAUDE_BIN").unwrap_or_else(|_| "claude".into());
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(["-p", "Reply with just: ok", "--model", "haiku", "--max-turns", "1"])
        .args(["--output-format", "stream-json", "--verbose", "--strict-mcp-config", "--tools", ""])
        .current_dir(std::env::temp_dir())
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let run = tokio::time::timeout(std::time::Duration::from_secs(90), cmd.output());
    let Ok(Ok(out)) = run.await else { return };
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            record_limits(&v, crate::thread::now_secs());
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn limits_are_stale_when_unknown_old_or_past_reset() {
        let w = |at| vec![rui_core::LimitWindow { name: "five_hour".into(), used_pct: Some(10.0), resets_at: Some(at), status: None }];
        assert!(super::limits_stale(&[], None, 1000));
        assert!(!super::limits_stale(&w(5000), Some(900), 1000));
        assert!(super::limits_stale(&w(950), Some(900), 1000));
        assert!(super::limits_stale(&w(99_999), Some(0), 2000));
    }

    use super::*;

    const REAL_LIMIT_LINE: &str = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","resetsAt":1795170600,"rateLimitType":"five_hour","overageStatus":"rejected","overageDisabledReason":"out_of_credits","isUsingOverage":false,"unifiedWindows":{"five_hour":{"utilization":0.42,"resetsAt":1795170600,"status":"allowed"},"seven_day":{"utilization":0.7,"resetsAt":1795402800,"status":"allowed"}}}}"#;

    #[test]
    fn parses_real_rate_limit_line() {
        let w = parse_limits(&serde_json::from_str(REAL_LIMIT_LINE).unwrap());
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].name, "five_hour");
        assert_eq!(w[0].used_pct, Some(42.0));
        assert_eq!(w[0].resets_at, Some(1795170600));
        assert_eq!(w[1].name, "seven_day");
        assert_eq!(w[1].used_pct.map(|p| p.round()), Some(70.0));
    }

    #[test]
    fn limit_line_without_windows_has_status_and_reset_only() {
        let v = json!({"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":5,"rateLimitType":"five_hour"}});
        let w = parse_limits(&v);
        assert_eq!((w[0].used_pct, w[0].resets_at, w[0].status.as_deref()), (None, Some(5), Some("rejected")));
        assert!(parse_limits(&json!({"type":"assistant"})).is_empty());
    }

    fn seat(id: &str, order: i32, budget: Option<f64>) -> Seat {
        Seat { id: id.into(), label: id.into(), order, budget_usd: budget, ..default_seat() }
    }

    fn spend(db: &Db, seat_id: &str, usd: f64) {
        db.add_event(None, "lead", "result", &json!({"total_cost_usd": usd, "seat_id": seat_id})).unwrap();
    }

    fn now() -> i64 {
        crate::thread::now_secs()
    }

    #[test]
    fn picks_lowest_order_first() {
        let db = Db::open_test();
        let seats = [seat("b", 2, None), seat("a", 1, None)];
        assert_eq!(pick_seat(&db, &seats, now()).unwrap().unwrap().id, "a");
    }

    #[test]
    fn switches_at_ninety_percent() {
        let db = Db::open_test();
        let seats = [seat("a", 1, Some(10.0)), seat("b", 2, None)];
        spend(&db, "a", 8.9);
        assert_eq!(pick_seat(&db, &seats, now()).unwrap().unwrap().id, "a");
        spend(&db, "a", 0.1);
        assert_eq!(pick_seat(&db, &seats, now()).unwrap().unwrap().id, "b");
    }

    #[test]
    fn skips_exhausted_and_none_when_all_used() {
        let db = Db::open_test();
        let seats = [seat("a", 1, None), seat("b", 2, Some(1.0))];
        mark_exhausted(&db, &seats[0], now()).unwrap();
        assert_eq!(pick_seat(&db, &seats, now()).unwrap().unwrap().id, "b");
        spend(&db, "b", 1.0);
        assert!(pick_seat(&db, &seats, now()).unwrap().is_none());
        let later = now() + 6 * 3600;
        assert_eq!(pick_seat(&db, &seats, later).unwrap().unwrap().id, "a");
    }

    #[test]
    fn usage_counts_only_this_seat_inside_its_window() {
        let db = Db::open_test();
        let s = seat("a", 1, Some(10.0));
        spend(&db, "a", 2.0);
        spend(&db, "a", 3.0);
        spend(&db, "other", 50.0);
        db.add_event(None, "lead", "result", &json!({"total_cost_usd": 7.0})).unwrap();
        assert_eq!(used_usd(&db, &s, now()).unwrap(), 5.0);
        assert_eq!(used_pct(&s, 5.0), Some(50.0));
        assert_eq!(used_usd(&db, &s, now() + 5 * 3600 + 5).unwrap(), 0.0);
    }

    #[test]
    fn missing_file_is_one_unlimited_default_seat() {
        let d = tempfile::tempdir().unwrap();
        let db = Db::open_test();
        assert_eq!(load(d.path()).unwrap(), vec![default_seat()]);
        spend(&db, "default", 1000.0);
        assert_eq!(pick_from(&db, d.path(), now()).unwrap().unwrap().id, "default");
        assert!(seat_env(&default_seat()).unwrap().is_empty());
    }

    #[test]
    fn validates_fields() {
        let key = Seat { kind: SeatKind::ApiKey, env_var: Some("ANTHROPIC_API_KEY_WORK".into()), ..seat("k", 1, None) };
        assert!(validate(&key).is_ok());
        assert!(validate(&Seat { env_var: Some("sk-ant-123".into()), ..key.clone() }).is_err());
        assert!(validate(&Seat { env_var: None, ..key.clone() }).is_err());
        let sub = seat("s", 1, None);
        assert!(validate(&Seat { config_dir: Some("~/.claude-work".into()), ..sub.clone() }).is_ok());
        assert!(validate(&Seat { config_dir: Some("/abs".into()), ..sub.clone() }).is_ok());
        assert!(validate(&Seat { config_dir: Some("rel/dir".into()), ..sub.clone() }).is_err());
        assert!(validate(&Seat { id: "a b".into(), ..sub }).is_err());
    }

    #[test]
    fn upsert_and_remove_round_trip() {
        let d = tempfile::tempdir().unwrap();
        upsert(d.path(), seat("w", 2, Some(5.0))).unwrap();
        upsert(d.path(), seat("w", 3, Some(6.0))).unwrap();
        let ids: Vec<String> = load(d.path()).unwrap().into_iter().map(|s| s.id).collect();
        assert_eq!(ids, ["default", "w"]);
        assert!(remove(d.path(), "w").unwrap());
        assert!(!remove(d.path(), "w").unwrap());
    }

    #[test]
    fn api_key_seat_reads_env_at_run_time_and_skips_when_missing() {
        let db = Db::open_test();
        let name = "RUI_TEST_SEAT_KEY_PICK";
        let key = Seat { kind: SeatKind::ApiKey, env_var: Some(name.into()), ..seat("k", 1, None) };
        assert!(pick_seat(&db, std::slice::from_ref(&key), now()).unwrap().is_none());
        assert!(seat_env(&key).is_err());
        // SAFETY: this test alone uses this variable name.
        unsafe { std::env::set_var(name, "fake-value") };
        assert_eq!(pick_seat(&db, std::slice::from_ref(&key), now()).unwrap().unwrap().id, "k");
        assert_eq!(seat_env(&key).unwrap(), [("ANTHROPIC_API_KEY".to_string(), "fake-value".to_string())]);
        unsafe { std::env::remove_var(name) };
    }

    #[test]
    fn scrub_removes_every_seat_key_and_the_default_key() {
        let d = tempfile::tempdir().unwrap();
        let key = Seat { kind: SeatKind::ApiKey, env_var: Some("ANTHROPIC_API_KEY_WORK".into()), ..seat("k", 1, None) };
        upsert(d.path(), key).unwrap();
        let mut cmd = std::process::Command::new("true");
        scrub_env(&mut cmd, d.path());
        let removed: Vec<String> = cmd.get_envs().filter(|(_, v)| v.is_none()).map(|(k, _)| k.to_string_lossy().into_owned()).collect();
        assert_eq!(removed, ["ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY_WORK"]);
    }

    #[test]
    fn limit_phrases_are_detected() {
        assert!(hit_limit("Claude AI usage limit reached|1700000000"));
        assert!(hit_limit("API Error: 429 rate_limit_error"));
        assert!(!hit_limit("all tests pass"));
    }

    #[test]
    fn gate_logs_exhausted_once_then_freed() {
        let d = tempfile::tempdir().unwrap();
        let db = Db::open_test();
        upsert(d.path(), seat("default", 1, Some(1.0))).unwrap();
        spend(&db, "default", 1.0);
        assert!(!check_gate(&db, d.path(), now()).unwrap());
        assert!(!check_gate(&db, d.path(), now()).unwrap());
        let evs = db.events_of_kinds(&["seats_exhausted"]).unwrap();
        assert_eq!(evs.len(), 1);
        assert!(evs[0].payload["reason"].as_str().unwrap().contains("frees up"));
        assert!(check_gate(&db, d.path(), now() + 6 * 3600).unwrap());
        assert_eq!(latest_gate_event(&db).unwrap().unwrap().kind, "seats_freed");
    }
}

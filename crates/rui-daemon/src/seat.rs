//! Reads which account the Claude CLI is signed in with.

use rui_core::SeatInfo;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::RwLock;

/// Shared, periodically refreshed seat info.
pub type SeatCache = Arc<RwLock<SeatInfo>>;

fn failed(msg: &str) -> SeatInfo {
    SeatInfo { logged_in: false, error: Some(msg.into()), ..Default::default() }
}

/// Parses the JSON printed by `claude auth status`.
pub fn parse_auth_status(json: &str) -> SeatInfo {
    let Ok(v) = serde_json::from_str::<Value>(json) else {
        return failed("could not read auth status");
    };
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(String::from);
    SeatInfo {
        logged_in: v.get("loggedIn").and_then(Value::as_bool).unwrap_or(false),
        auth_method: s("authMethod"),
        email: s("email"),
        subscription_type: s("subscriptionType"),
        org_name: s("orgName"),
        last_model: None,
        error: None,
        ..Default::default()
    }
}

/// Runs `claude auth status` (10s timeout) and parses the output.
pub async fn read_seat() -> SeatInfo {
    let bin = std::env::var("RUI_CLAUDE_BIN").unwrap_or_else(|_| "claude".into());
    let mut cmd = Command::new(&bin);
    cmd.args(["auth", "status"]).kill_on_drop(true);
    match tokio::time::timeout(Duration::from_secs(10), cmd.output()).await {
        Ok(Ok(out)) => parse_auth_status(&String::from_utf8_lossy(&out.stdout)),
        Ok(Err(e)) => failed(&format!("could not run {bin}: {e}")),
        Err(_) => failed("auth status timed out"),
    }
}

/// Refreshes the cache now, then every 60 seconds.
pub async fn refresh_loop(cache: SeatCache) {
    loop {
        let info = read_seat().await;
        *cache.write().await = info;
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_logged_in() {
        let j = r#"{"loggedIn":true,"authMethod":"claude.ai","email":"me@example.com","subscriptionType":"max","orgName":"Me Org"}"#;
        let s = parse_auth_status(j);
        assert!(s.logged_in);
        assert_eq!(s.auth_method.as_deref(), Some("claude.ai"));
        assert_eq!(s.email.as_deref(), Some("me@example.com"));
        assert_eq!(s.subscription_type.as_deref(), Some("max"));
        assert_eq!(s.org_name.as_deref(), Some("Me Org"));
        assert!(s.last_model.is_none() && s.error.is_none());
    }

    #[test]
    fn parses_logged_out() {
        let s = parse_auth_status(r#"{"loggedIn": false}"#);
        assert!(!s.logged_in);
        assert!(s.email.is_none() && s.error.is_none());
    }

    #[test]
    fn garbage_gives_error() {
        let s = parse_auth_status("not json");
        assert!(!s.logged_in);
        assert_eq!(s.error.as_deref(), Some("could not read auth status"));
    }
}

//! Per-run MCP tokens. Every agent run gets a fresh random token, registered with the MCP server
//! it may call and the task and source it acts for. The MCP router accepts only a known token for
//! its own server and takes the caller's identity from the grant, never from request headers.
//!
//! The token reaches the agent only through its process environment (`RUI_RUN_TOKEN`); the MCP
//! config names the variable, not the value. The `RunToken` guard removes the entry when it drops,
//! so a run that ends, fails, times out or is aborted loses MCP access.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

/// Header an agent sends its run token in.
pub const HEADER: &str = "x-rui-run-token";
/// Environment variable that carries the token into the agent process.
pub const ENV: &str = "RUI_RUN_TOKEN";

/// The MCP server a run may call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Server {
    Rui,
    Lead,
    Builder,
    Scout,
    Coach,
}

/// What a run token allows: one server, acting for one task and source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunGrant {
    pub server: Server,
    pub task_id: Option<i64>,
    pub source_ref: Option<String>,
}

impl RunGrant {
    /// A grant for `server` acting on `task_id`, with no source ref.
    pub fn new(server: Server, task_id: Option<i64>) -> Self {
        RunGrant { server, task_id, source_ref: None }
    }
}

/// Live run tokens and their grants.
#[derive(Clone, Default)]
pub struct RunTokens(Arc<Mutex<HashMap<String, RunGrant>>>);

impl RunTokens {
    /// Registers a new random token for `grant`. The entry lives as long as the returned guard.
    pub fn issue(&self, grant: RunGrant) -> anyhow::Result<RunToken> {
        let token = crate::owner_auth::generate()?;
        self.lock().insert(token.clone(), grant);
        Ok(RunToken { tokens: self.clone(), token })
    }

    /// The grant for `token`, if it is live.
    pub fn grant(&self, token: &str) -> Option<RunGrant> {
        self.lock().get(token).cloned()
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, RunGrant>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The daemon's token registry, shared by the runner and the MCP router.
pub fn global() -> &'static RunTokens {
    static TOKENS: OnceLock<RunTokens> = OnceLock::new();
    TOKENS.get_or_init(RunTokens::default)
}

/// A registered token. Dropping it revokes the token.
pub struct RunToken {
    tokens: RunTokens,
    token: String,
}

impl RunToken {
    /// The secret value, for the agent's environment only. Never log or write it.
    pub fn value(&self) -> &str {
        &self.token
    }
}

impl Drop for RunToken {
    fn drop(&mut self) {
        self.tokens.lock().remove(&self.token);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_live_until_the_guard_drops() {
        let tokens = RunTokens::default();
        let guard = tokens.issue(RunGrant::new(Server::Builder, Some(3))).unwrap();
        let value = guard.value().to_string();
        assert_eq!(value.len(), 64);
        assert_eq!(tokens.grant(&value), Some(RunGrant::new(Server::Builder, Some(3))));
        assert!(tokens.grant("nope").is_none());
        drop(guard);
        assert!(tokens.grant(&value).is_none());
    }

    #[test]
    fn each_run_gets_its_own_token() {
        let tokens = RunTokens::default();
        let a = tokens.issue(RunGrant::new(Server::Lead, Some(1))).unwrap();
        let b = tokens.issue(RunGrant::new(Server::Lead, Some(1))).unwrap();
        assert_ne!(a.value(), b.value());
    }
}

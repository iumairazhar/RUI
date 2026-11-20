//! Owner token for the HTTP API. A fresh random token is written to `<data_dir>/owner.token`
//! (mode 0600) at every boot; every owner route requires it in the `x-rui-owner` header.
//!
//! What this protects, and what it doesn't:
//! - Routes under /mcp don't use this token. They need a per-run token instead (run_tokens.rs),
//!   which only works on that run's own MCP server and only while the run is alive.
//! - The token is never put into an agent's prompt, environment or MCP config. But the file is
//!   readable by any process running as the same OS user. Account Lead and member runs on Claude
//!   are kept away from it by the sandbox and deny rules in sandbox.rs. Rui chat, Rui decision,
//!   Scout and Coach turns, and Codex runs with RUI_ALLOW_CODEX_UNSANDBOXED_READS set, aren't.
//! - `touches_owner_api` only applies where the approve permission tool runs (Rui chat). It is not
//!   a guard for member runs.
//! - The full fix for unattended runs is to run agents as a separate OS user or in a container.

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Header the owner's app must send on every owner route.
pub const HEADER: &str = "x-rui-owner";

/// File name of the token inside the data dir.
pub const FILE: &str = "owner.token";

/// Path of the token file.
pub fn token_path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE)
}

/// 32 random bytes from /dev/urandom, hex encoded.
pub fn generate() -> anyhow::Result<String> {
    let mut buf = [0u8; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut buf)?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

/// Writes a new token to the data dir with mode 0600, replacing any old one, and returns it.
pub fn issue(data_dir: &Path) -> anyhow::Result<String> {
    std::fs::create_dir_all(data_dir)?;
    let token = generate()?;
    let path = token_path(data_dir);
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    f.write_all(token.as_bytes())?;
    Ok(token)
}

/// Compares two byte strings in time that depends only on their lengths.
pub fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Middleware: 401 unless the request carries the owner token.
pub async fn require(State(token): State<Arc<str>>, req: Request, next: Next) -> Response {
    let sent = req.headers().get(HEADER).map(|v| v.as_bytes()).unwrap_or_default();
    if !constant_eq(sent, token.as_bytes()) {
        return (StatusCode::UNAUTHORIZED, axum::Json(serde_json::json!({"error": "owner token required"}))).into_response();
    }
    next.run(req).await
}

/// Port the daemon listens on (same rule as main).
pub fn port() -> u16 {
    std::env::var("RUI_PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(7878)
}

/// True when a shell command could reach the owner API or its token: it names the token
/// file, the data dir (outside worktrees), or the daemon's address with a non-/mcp path.
pub fn touches_owner_api(command: &str, data_dir: &Path, port: u16) -> bool {
    if command.contains(FILE) || names_data_dir(command, data_dir) {
        return true;
    }
    ["127.0.0.1", "localhost"].iter().any(|host| hits_non_mcp(command, &format!("{host}:{port}")))
}

/// True when the command names the data dir anywhere but its worktrees folder.
fn names_data_dir(command: &str, data_dir: &Path) -> bool {
    let dir = data_dir.to_string_lossy();
    if dir.is_empty() {
        return false;
    }
    let worktrees = format!("{dir}/worktrees/");
    command.match_indices(dir.as_ref()).any(|(at, _)| !command[at..].starts_with(&worktrees))
}

/// True when any occurrence of `addr` is followed by a path that is not under /mcp.
fn hits_non_mcp(command: &str, addr: &str) -> bool {
    command.match_indices(addr).any(|(at, _)| {
        let rest = &command[at + addr.len()..];
        !(rest.starts_with("/mcp/") || rest == "/mcp" || rest.starts_with("/mcp ") || rest.starts_with("/mcp\"") || rest.starts_with("/mcp'"))
    })
}

/// Token used by tests that build the router.
#[cfg(test)]
pub fn test_token() -> Arc<str> {
    Arc::from("test-owner-token")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_writes_a_private_fresh_token() {
        let d = tempfile::tempdir().unwrap();
        let a = issue(d.path()).unwrap();
        let b = issue(d.path()).unwrap();
        assert_eq!((a.len(), a == b), (64, false));
        assert_eq!(std::fs::read_to_string(token_path(d.path())).unwrap(), b);
        let mode = std::fs::metadata(token_path(d.path())).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn constant_eq_checks_length_and_bytes() {
        assert!(constant_eq(b"abc", b"abc"));
        assert!(!constant_eq(b"abc", b"abd"));
        assert!(!constant_eq(b"abc", b"ab"));
    }

    #[test]
    fn owner_api_commands_are_detected() {
        let d = Path::new("/data/rui");
        assert!(touches_owner_api("cat owner.token", d, 7878));
        assert!(touches_owner_api("ls /data/rui", d, 7878));
        assert!(!touches_owner_api("cd /data/rui/worktrees/5 && ls", d, 7878));
        assert!(touches_owner_api("curl http://127.0.0.1:7878/tasks", d, 7878));
        assert!(touches_owner_api("curl localhost:7878", d, 7878));
        assert!(!touches_owner_api("curl http://127.0.0.1:7878/mcp/rui", d, 7878));
        assert!(!touches_owner_api("curl http://127.0.0.1:3000/tasks", d, 7878));
    }
}

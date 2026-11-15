//! Runs one headless Claude Code CLI turn and streams its JSON output.

use anyhow::{Context, bail};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;

/// Everything needed to run one agent turn.
pub struct AgentRun {
    pub role: rui_core::Role,
    pub task_id: Option<i64>,
    pub cwd: PathBuf,
    pub prompt: String,
    pub system_prompt: String,
    pub allowed_tools: Vec<String>,
    pub permission_mode: Option<String>,
    pub mcp_url: String,
    pub mcp_headers: Vec<(String, String)>,
    pub resume: Option<String>,
}

/// Final result of a turn.
pub struct RunOutcome {
    pub session_id: Option<String>,
    pub result_text: Option<String>,
    pub is_error: bool,
    pub exit_ok: bool,
}

/// One parsed stream-json line.
pub enum ParsedLine {
    Init { session_id: String, raw: Value },
    Assistant(Value),
    ToolResult(Value),
    Result { session_id: Option<String>, text: Option<String>, is_error: bool, raw: Value },
    Other(Value),
}

/// Builds the MCP config JSON pointing at the RUI server.
pub fn mcp_config_json(url: &str, headers: &[(String, String)]) -> Value {
    let hdrs: serde_json::Map<String, Value> =
        headers.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    json!({"mcpServers": {"rui": {"type": "http", "url": url, "headers": hdrs}}})
}

/// Builds the CLI argument list for a run.
pub fn build_args(run: &AgentRun, mcp_config_path: &Path) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "-p".into(),
        run.prompt.clone(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--mcp-config".into(),
        mcp_config_path.to_string_lossy().into_owned(),
        "--strict-mcp-config".into(),
        "--setting-sources".into(),
        "project".into(),
        "--disable-slash-commands".into(),
        "--append-system-prompt".into(),
        run.system_prompt.clone(),
    ];
    if !run.allowed_tools.is_empty() {
        a.push("--allowedTools".into());
        a.push(run.allowed_tools.join(","));
    }
    if let Some(m) = &run.permission_mode {
        a.push("--permission-mode".into());
        a.push(m.clone());
    }
    if let Some(id) = &run.resume {
        a.push("--resume".into());
        a.push(id.clone());
    }
    a
}

/// Parses one stdout line; None for blank or non-JSON lines.
pub fn parse_line(line: &str) -> Option<ParsedLine> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(String::from);
    Some(match v.get("type").and_then(Value::as_str) {
        Some("system") if s("subtype").as_deref() == Some("init") => {
            ParsedLine::Init { session_id: s("session_id").unwrap_or_default(), raw: v }
        }
        Some("assistant") => ParsedLine::Assistant(v),
        Some("user") => ParsedLine::ToolResult(v),
        Some("result") => ParsedLine::Result {
            session_id: s("session_id"),
            text: s("result"),
            is_error: v.get("is_error").and_then(Value::as_bool).unwrap_or(false),
            raw: v,
        },
        _ => ParsedLine::Other(v),
    })
}

fn temp_config_path() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("rui-mcp-{}-{}.json", std::process::id(), nanos))
}

/// Runs the Claude CLI once, calling `on_line` for every parsed line.
pub async fn run_agent(
    run: &AgentRun,
    on_line: impl FnMut(&ParsedLine),
) -> anyhow::Result<RunOutcome> {
    let cfg = temp_config_path();
    let json = mcp_config_json(&run.mcp_url, &run.mcp_headers);
    std::fs::write(&cfg, serde_json::to_vec(&json)?).context("write mcp config")?;
    let res = run_with_config(run, &cfg, on_line).await;
    let _ = std::fs::remove_file(&cfg);
    res
}

async fn run_with_config(
    run: &AgentRun,
    cfg: &Path,
    mut on_line: impl FnMut(&ParsedLine),
) -> anyhow::Result<RunOutcome> {
    let bin = std::env::var("RUI_CLAUDE_BIN").unwrap_or_else(|_| "claude".into());
    let mut child = Command::new(&bin)
        .args(build_args(run, cfg))
        .current_dir(&run.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("spawn {bin}"))?;
    let mut stderr = child.stderr.take().context("stderr")?;
    let err_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf).await;
        buf
    });
    let mut out = RunOutcome { session_id: None, result_text: None, is_error: false, exit_ok: false };
    let mut lines = BufReader::new(child.stdout.take().context("stdout")?).lines();
    while let Some(line) = lines.next_line().await? {
        let Some(p) = parse_line(&line) else { continue };
        track(&mut out, &p);
        on_line(&p);
    }
    out.exit_ok = child.wait().await?.success();
    let err = err_task.await.unwrap_or_default();
    if !out.exit_ok && out.result_text.is_none() {
        let start = err.len().saturating_sub(2048);
        bail!("claude exited with failure: {}", String::from_utf8_lossy(&err[start..]));
    }
    Ok(out)
}

fn track(out: &mut RunOutcome, p: &ParsedLine) {
    match p {
        ParsedLine::Init { session_id, .. } => out.session_id = Some(session_id.clone()),
        ParsedLine::Result { session_id, text, is_error, .. } => {
            if session_id.is_some() {
                out.session_id = session_id.clone();
            }
            out.result_text = text.clone();
            out.is_error = *is_error;
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn run(cwd: &Path) -> AgentRun {
        AgentRun {
            role: rui_core::Role::Lead,
            task_id: Some(1),
            cwd: cwd.to_path_buf(),
            prompt: "hello".into(),
            system_prompt: "sys".into(),
            allowed_tools: vec![],
            permission_mode: None,
            mcp_url: "http://127.0.0.1:1/none".into(),
            mcp_headers: vec![],
            resume: None,
        }
    }

    #[test]
    fn parses_each_variant() {
        let init = r#"{"type":"system","subtype":"init","session_id":"s1"}"#;
        assert!(matches!(parse_line(init), Some(ParsedLine::Init { session_id, .. }) if session_id == "s1"));
        assert!(matches!(parse_line(r#"{"type":"assistant"}"#), Some(ParsedLine::Assistant(_))));
        assert!(matches!(parse_line(r#"{"type":"user"}"#), Some(ParsedLine::ToolResult(_))));
        let res = r#"{"type":"result","session_id":"s2","result":"hi","is_error":true}"#;
        match parse_line(res) {
            Some(ParsedLine::Result { session_id, text, is_error, .. }) => {
                assert_eq!(session_id.as_deref(), Some("s2"));
                assert_eq!(text.as_deref(), Some("hi"));
                assert!(is_error);
            }
            _ => panic!("expected result"),
        }
        assert!(matches!(parse_line(r#"{"type":"system","subtype":"x"}"#), Some(ParsedLine::Other(_))));
        assert!(parse_line("").is_none());
        assert!(parse_line("   ").is_none());
        assert!(parse_line("not json").is_none());
    }

    #[test]
    fn result_defaults_not_error() {
        match parse_line(r#"{"type":"result"}"#) {
            Some(ParsedLine::Result { is_error, text, .. }) => {
                assert!(!is_error);
                assert!(text.is_none());
            }
            _ => panic!("expected result"),
        }
    }

    #[test]
    fn args_without_and_with_optionals() {
        let d = tempfile::tempdir().unwrap();
        let mut r = run(d.path());
        let a = build_args(&r, Path::new("/x.json"));
        assert_eq!(a[..2], ["-p", "hello"]);
        assert!(a.contains(&"--strict-mcp-config".to_string()));
        assert!(a.contains(&"--disable-slash-commands".to_string()));
        let ss = a.iter().position(|x| x == "--setting-sources").unwrap();
        assert_eq!(a[ss + 1], "project");
        assert!(!a.contains(&"--allowedTools".to_string()));
        assert!(!a.contains(&"--permission-mode".to_string()));
        assert!(!a.contains(&"--resume".to_string()));
        r.allowed_tools = vec!["Read".into(), "Bash".into()];
        r.permission_mode = Some("acceptEdits".into());
        r.resume = Some("abc".into());
        let a = build_args(&r, Path::new("/x.json"));
        let pos = |k: &str| a.iter().position(|x| x == k).unwrap();
        assert_eq!(a[pos("--allowedTools") + 1], "Read,Bash");
        assert_eq!(a[pos("--permission-mode") + 1], "acceptEdits");
        assert_eq!(a[pos("--resume") + 1], "abc");
    }

    #[test]
    fn mcp_config_shape() {
        let v = mcp_config_json("http://h/mcp", &[("X-Rui-Task-Id".into(), "5".into())]);
        assert_eq!(v["mcpServers"]["rui"]["type"], "http");
        assert_eq!(v["mcpServers"]["rui"]["url"], "http://h/mcp");
        assert_eq!(v["mcpServers"]["rui"]["headers"]["X-Rui-Task-Id"], "5");
    }

    fn fake(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[tokio::test]
    async fn fake_claude_success_and_failure() {
        let d = tempfile::tempdir().unwrap();
        let ok = fake(
            d.path(),
            "ok.sh",
            r#"echo '{"type":"system","subtype":"init","session_id":"s1"}'
echo '{"type":"assistant","message":{}}'
echo '{"type":"result","result":"hi","is_error":false}'"#,
        );
        let bad = fake(d.path(), "bad.sh", "echo boom >&2\nexit 1");
        // SAFETY: single test mutates this env var, sequentially.
        unsafe { std::env::set_var("RUI_CLAUDE_BIN", &ok) };
        let mut n = 0;
        let out = run_agent(&run(d.path()), |_| n += 1).await.unwrap();
        assert_eq!(out.session_id.as_deref(), Some("s1"));
        assert_eq!(out.result_text.as_deref(), Some("hi"));
        assert!(out.exit_ok && !out.is_error);
        assert_eq!(n, 3);
        unsafe { std::env::set_var("RUI_CLAUDE_BIN", &bad) };
        let err = run_agent(&run(d.path()), |_| {}).await.err().unwrap();
        assert!(err.to_string().contains("boom"));
        unsafe { std::env::remove_var("RUI_CLAUDE_BIN") };
    }

    #[tokio::test]
    #[ignore]
    async fn real_claude_smoke() {
        let d = tempfile::tempdir().unwrap();
        let mut r = run(d.path());
        r.prompt = "Reply with the word ok".into();
        let out = run_agent(&r, |l| {
            if let ParsedLine::Result { text, .. } = l {
                println!("result: {text:?}");
            }
        })
        .await
        .unwrap();
        assert!(out.exit_ok);
        assert!(out.session_id.is_some());
    }
}

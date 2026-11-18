//! Runs one headless agent CLI turn (Claude Code by default, or Codex; see `codex.rs`) and streams its JSON output.

use anyhow::{Context, bail};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;

/// Which agent CLI runs a turn. Claude is the default; Codex is chosen per role or per client pod.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Cli {
    #[default]
    Claude,
    Codex,
}

impl Cli {
    /// "claude" or "codex" (any case); None for anything else.
    pub fn parse(s: &str) -> Option<Cli> {
        match s.trim().to_ascii_lowercase().as_str() {
            "claude" => Some(Cli::Claude),
            "codex" => Some(Cli::Codex),
            _ => None,
        }
    }
}

/// Everything needed to run one agent turn.
pub struct AgentRun {
    pub role: rui_core::Role,
    pub task_id: Option<i64>,
    pub cwd: PathBuf,
    pub prompt: String,
    pub system_prompt: String,
    pub allowed_tools: Vec<String>,
    /// Built-in CLI tools the agent may see; `Some("")` hides them all.
    pub builtin_tools: Option<String>,
    pub permission_mode: Option<String>,
    /// MCP tool that decides permission prompts (`--permission-prompt-tool`).
    pub permission_prompt_tool: Option<String>,
    pub mcp_url: String,
    /// The MCP server, task and source this run acts for; bound to a per-run token.
    pub grant: crate::run_tokens::RunGrant,
    pub resume: Option<String>,
    /// Ask the CLI for partial message events (live typing).
    pub partial: bool,
    /// Seat to run on; None runs on the default login with the daemon's own environment.
    pub seat: Option<crate::seats::Seat>,
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
    /// Inner `event` of a `stream_event` line; live only, never stored.
    Stream(Value),
    Result { session_id: Option<String>, text: Option<String>, is_error: bool, raw: Value },
    Other(Value),
}

/// Builds the MCP config JSON pointing at the RUI server. The run token header names the
/// environment variable; Claude Code expands it, so the token value never lands in the file.
pub fn mcp_config_json(url: &str) -> Value {
    let token = format!("${{{}}}", crate::run_tokens::ENV);
    json!({"mcpServers": {"rui": {"type": "http", "url": url, "headers": {crate::run_tokens::HEADER: token}}}})
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
    if let Some(t) = &run.builtin_tools {
        a.push("--tools".into());
        a.push(t.clone());
    }
    if let Some(m) = &run.permission_mode {
        a.push("--permission-mode".into());
        a.push(m.clone());
    }
    if let Some(t) = &run.permission_prompt_tool {
        a.push("--permission-prompt-tool".into());
        a.push(t.clone());
    }
    if let Some(id) = &run.resume {
        a.push("--resume".into());
        a.push(id.clone());
    }
    if run.partial {
        a.push("--include-partial-messages".into());
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
        Some("stream_event") => ParsedLine::Stream(v.get("event").cloned().unwrap_or(Value::Null)),
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
    run_agent_with(Cli::Claude, run, on_line).await
}

/// Runs one turn on the chosen agent CLI. Both report through the same `ParsedLine` shape.
/// The run's MCP token lives exactly as long as this future: dropping it (abort, timeout) revokes it.
pub async fn run_agent_with(
    cli: Cli,
    run: &AgentRun,
    on_line: impl FnMut(&ParsedLine),
) -> anyhow::Result<RunOutcome> {
    let token = crate::run_tokens::global().issue(run.grant.clone())?;
    match cli {
        Cli::Claude => run_claude(run, token.value(), on_line).await,
        Cli::Codex => crate::codex::run(run, token.value(), on_line).await,
    }
}

async fn run_claude(
    run: &AgentRun,
    token: &str,
    on_line: impl FnMut(&ParsedLine),
) -> anyhow::Result<RunOutcome> {
    let cfg = temp_config_path();
    let json = mcp_config_json(&run.mcp_url);
    std::fs::write(&cfg, serde_json::to_vec(&json)?).context("write mcp config")?;
    let res = run_with_config(run, &cfg, token, on_line).await;
    let _ = std::fs::remove_file(&cfg);
    res
}

async fn run_with_config(
    run: &AgentRun,
    cfg: &Path,
    token: &str,
    on_line: impl FnMut(&ParsedLine),
) -> anyhow::Result<RunOutcome> {
    let bin = std::env::var("RUI_CLAUDE_BIN").unwrap_or_else(|_| "claude".into());
    let mut cmd = command(&bin, run, cfg)?;
    cmd.env(crate::run_tokens::ENV, token);
    stream(cmd, &bin, "claude", parse_line, on_line).await
}

/// Kills a whole process group when dropped, so an aborted, stopped or timed-out run takes its
/// test runners and builds down with it. Disarmed once the agent exits on its own.
struct GroupKill(Option<u32>);

impl GroupKill {
    fn disarm(mut self) {
        self.0 = None;
    }
}

impl Drop for GroupKill {
    fn drop(&mut self) {
        let Some(pgid) = self.0 else { return };
        let _ = std::process::Command::new("kill")
            .args(["-KILL", "--", &format!("-{pgid}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// Spawns `cmd` in its own process group, parses each stdout line with `parse` and tracks the
/// outcome. `name` is the CLI's name in the failure message.
pub(crate) async fn stream(
    mut cmd: Command,
    bin: &str,
    name: &str,
    mut parse: impl FnMut(&str) -> Option<ParsedLine>,
    mut on_line: impl FnMut(&ParsedLine),
) -> anyhow::Result<RunOutcome> {
    cmd.process_group(0);
    let mut child = cmd.spawn().with_context(|| format!("spawn {bin}"))?;
    let group = GroupKill(child.id());
    let mut stderr = child.stderr.take().context("stderr")?;
    let err_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf).await;
        buf
    });
    let mut out = RunOutcome { session_id: None, result_text: None, is_error: false, exit_ok: false };
    let mut lines = BufReader::new(child.stdout.take().context("stdout")?).lines();
    while let Some(line) = lines.next_line().await? {
        let Some(p) = parse(&line) else { continue };
        track(&mut out, &p);
        on_line(&p);
    }
    out.exit_ok = child.wait().await?.success();
    group.disarm();
    let err = err_task.await.unwrap_or_default();
    if !out.exit_ok && out.result_text.is_none() {
        let start = err.len().saturating_sub(2048);
        bail!("{name} exited with failure: {}", String::from_utf8_lossy(&err[start..]));
    }
    Ok(out)
}

/// The CLI command for a run: every secret variable removed, then the seat's environment applied
/// (ANTHROPIC_API_KEY only for an active api_key seat).
fn command(bin: &str, run: &AgentRun, cfg: &Path) -> anyhow::Result<Command> {
    let env = match &run.seat {
        Some(seat) => crate::seats::seat_env(seat)?,
        None => vec![],
    };
    let mut cmd = Command::new(bin);
    crate::seats::scrub_env(cmd.as_std_mut(), &crate::state::data_dir());
    cmd.args(build_args(run, cfg))
        .envs(env)
        .current_dir(&run.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    Ok(cmd)
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
            builtin_tools: None,
            permission_mode: None,
            permission_prompt_tool: None,
            mcp_url: "http://127.0.0.1:1/none".into(),
            grant: crate::run_tokens::RunGrant::new(crate::run_tokens::Server::Lead, Some(1)),
            resume: None,
            partial: false,
            seat: None,
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
        let se = r#"{"type":"stream_event","event":{"type":"message_start"}}"#;
        assert!(matches!(parse_line(se), Some(ParsedLine::Stream(e)) if e["type"] == "message_start"));
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
        assert!(!a.contains(&"--tools".to_string()));
        assert!(!a.contains(&"--permission-prompt-tool".to_string()));
        r.allowed_tools = vec!["Read".into(), "Bash".into()];
        r.permission_mode = Some("acceptEdits".into());
        r.resume = Some("abc".into());
        r.builtin_tools = Some(String::new());
        r.permission_prompt_tool = Some("mcp__rui__approve".into());
        let a = build_args(&r, Path::new("/x.json"));
        let pos = |k: &str| a.iter().position(|x| x == k).unwrap();
        assert_eq!(a[pos("--allowedTools") + 1], "Read,Bash");
        assert_eq!(a[pos("--permission-mode") + 1], "acceptEdits");
        assert_eq!(a[pos("--resume") + 1], "abc");
        assert_eq!(a[pos("--tools") + 1], "");
        assert_eq!(a[pos("--permission-prompt-tool") + 1], "mcp__rui__approve");
    }

    #[test]
    fn partial_flag_only_when_set() {
        let d = tempfile::tempdir().unwrap();
        let mut r = run(d.path());
        let flag = "--include-partial-messages".to_string();
        assert!(!build_args(&r, Path::new("/x.json")).contains(&flag));
        r.partial = true;
        assert!(build_args(&r, Path::new("/x.json")).contains(&flag));
    }

    #[test]
    fn seat_env_is_applied_to_the_command() {
        let d = tempfile::tempdir().unwrap();
        let mut r = run(d.path());
        let envs = |r: &AgentRun| {
            let cmd = command("claude", r, Path::new("/x.json")).unwrap();
            let set = cmd.as_std().get_envs().filter(|(_, v)| v.is_some());
            set.map(|(k, v)| (k.to_owned(), v.map(|v| v.to_owned()))).collect::<Vec<_>>()
        };
        assert!(envs(&r).is_empty());
        let cmd = command("claude", &r, Path::new("/x.json")).unwrap();
        assert!(cmd.as_std().get_envs().any(|(k, v)| k == "ANTHROPIC_API_KEY" && v.is_none()), "the daemon's key is removed");
        r.seat = Some(crate::seats::Seat { config_dir: Some("/tmp/claude-two".into()), ..crate::seats::default_seat() });
        let want = (std::ffi::OsString::from("CLAUDE_CONFIG_DIR"), Some(std::ffi::OsString::from("/tmp/claude-two")));
        assert_eq!(envs(&r), [want]);
        let missing = Some("RUI_TEST_RUNNER_MISSING_KEY".to_string());
        r.seat = Some(crate::seats::Seat { kind: crate::seats::SeatKind::ApiKey, env_var: missing, ..crate::seats::default_seat() });
        assert!(command("claude", &r, Path::new("/x.json")).is_err());
    }

    #[test]
    fn mcp_config_shape() {
        let v = mcp_config_json("http://h/mcp");
        assert_eq!(v["mcpServers"]["rui"]["type"], "http");
        assert_eq!(v["mcpServers"]["rui"]["url"], "http://h/mcp");
        let headers = v["mcpServers"]["rui"]["headers"].as_object().unwrap();
        assert_eq!(headers.len(), 1, "only the run token header: {headers:?}");
        assert_eq!(headers["x-rui-run-token"], "${RUI_RUN_TOKEN}");
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

    fn running(marker: &str) -> bool {
        let out = std::process::Command::new("pgrep").args(["-f", marker]).output().unwrap();
        !out.stdout.is_empty()
    }

    async fn wait_until(want: bool, marker: &str) -> bool {
        for _ in 0..100 {
            if running(marker) == want {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        false
    }

    #[tokio::test]
    async fn aborting_a_run_kills_its_whole_process_group() {
        let d = tempfile::tempdir().unwrap();
        // A sleep length no other command line will contain, so pgrep only finds this test's child.
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos();
        let marker = format!("600.{nanos:09}");
        let agent = fake(d.path(), "hang.sh", &format!("sleep {marker} &\nwait"));
        // Run through sh: exec'ing a script just written can fail with ETXTBSY under parallel tests.
        let mut cmd = Command::new("sh");
        cmd.arg(&agent).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        let run = tokio::spawn(async move { stream(cmd, "hang.sh", "hang", parse_line, |_| {}).await });
        assert!(wait_until(true, &marker).await, "the agent's child never started");
        run.abort();
        let _ = run.await;
        assert!(wait_until(false, &marker).await, "the agent's child outlived the abort");
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

//! The Codex arm of the agent runner, and which CLI a member runs on.
//!
//! A member runs on Codex when its role's AGENT.md frontmatter says `cli: codex`, or, when the role
//! says nothing, when the client's pod.json says `"cli": "codex"`. Everything else runs on Claude.
//! Only members (builder turns) can run on Codex; the Account Lead, Rui, Scout and Coach always run
//! on Claude because their work is done through RUI's MCP tools.
//!
//! A Codex turn runs `codex exec --json --skip-git-repo-check -C <cwd> --sandbox <mode> <prompt>`
//! (binary override: `RUI_CODEX_BIN`). Codex has no system prompt flag, so the role's system prompt
//! goes first in the prompt. The RUI MCP server is passed with `-c mcp_servers.rui.url=...` and
//! `-c mcp_servers.rui.http_headers={...}`; a Codex build without HTTP MCP support cannot call
//! report_status or ask_lead, and then the done_command is the only check of its work.
//! Not supported on Codex: resuming a session (every turn is fresh), allowed-tool lists, live typing,
//! seat environments, and dollar costs (Codex reports tokens only, so its result has no total_cost_usd).
//!
//! Codex JSONL is mapped onto the Claude shapes: `thread.started` is Init, an `agent_message` is an
//! assistant text block, a command or MCP call is an assistant tool_use when it starts and a
//! tool_result when it completes, and `turn.completed` / `turn.failed` / `error` is the Result,
//! with usage as `{input_tokens, output_tokens, cache_read_input_tokens, cache_creation_input_tokens}`.

use crate::runner::{AgentRun, Cli, ParsedLine, RunOutcome};
use serde_json::{Value, json};
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

/// The CLI for a member with role `role` on client `client`: the role's `cli`, else the pod's, else Claude.
pub fn for_member(data_dir: &Path, client: &str, role: &str) -> Cli {
    if let Some(cli) = role_cli(data_dir, role) {
        return cli;
    }
    let pod = crate::client_state::read_pod(data_dir, client).ok();
    pod.and_then(|p| p.cli).and_then(|c| Cli::parse(&c)).unwrap_or_default()
}

/// The `cli` frontmatter value of the role named `role`, if it sets a known one.
pub fn role_cli(data_dir: &Path, role: &str) -> Option<Cli> {
    let entries = std::fs::read_dir(data_dir.join("roles")).ok()?;
    for e in entries.flatten() {
        let Ok(text) = std::fs::read_to_string(e.path().join("AGENT.md")) else { continue };
        let Ok((front, _)) = crate::skills::split_frontmatter(&text) else { continue };
        if crate::skills::field(front, "name") == Some(role) {
            return crate::skills::field(front, "cli").and_then(Cli::parse);
        }
    }
    None
}

/// `s` as a TOML basic string (JSON string escapes are valid TOML).
fn toml_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// `-c` overrides that point Codex at the RUI MCP server with the run's headers.
pub fn mcp_args(url: &str, headers: &[(String, String)]) -> Vec<String> {
    let pairs: Vec<String> = headers.iter().map(|(k, v)| format!("{} = {}", toml_str(k), toml_str(v))).collect();
    vec![
        "-c".into(),
        format!("mcp_servers.rui.url={}", toml_str(url)),
        "-c".into(),
        format!("mcp_servers.rui.http_headers={{{}}}", pairs.join(", ")),
    ]
}

/// The prompt Codex gets: the system prompt first, since Codex has no flag for it.
fn prompt(run: &AgentRun) -> String {
    if run.system_prompt.trim().is_empty() {
        return run.prompt.clone();
    }
    format!("{}\n\n---\n\n{}", run.system_prompt, run.prompt)
}

/// The Codex argument list. A run allowed to edit (any permission mode) gets a workspace-write sandbox.
pub fn build_args(run: &AgentRun) -> Vec<String> {
    let sandbox = if run.permission_mode.is_some() { "workspace-write" } else { "read-only" };
    let mut a: Vec<String> = ["exec", "--json", "--skip-git-repo-check", "-C"].map(String::from).to_vec();
    a.push(run.cwd.to_string_lossy().into_owned());
    a.extend(["--sandbox".to_string(), sandbox.to_string()]);
    a.extend(mcp_args(&run.mcp_url, &run.mcp_headers));
    a.push(prompt(run));
    a
}

/// Turns Codex JSONL lines into `ParsedLine`s. Keeps the last agent message for the result text.
#[derive(Debug, Default)]
pub struct Parser {
    last_text: Option<String>,
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

impl Parser {
    /// One Codex stdout line; None for blank or non-JSON lines.
    pub fn parse(&mut self, line: &str) -> Option<ParsedLine> {
        let v: Value = serde_json::from_str(line.trim()).ok()?;
        Some(match v["type"].as_str().unwrap_or_default() {
            "thread.started" => ParsedLine::Init { session_id: text(&v["thread_id"]), raw: v },
            "item.started" => item(&v["item"], false).unwrap_or(ParsedLine::Other(v)),
            "item.completed" => self.completed(v),
            "turn.completed" => self.result(&v, None),
            "turn.failed" => self.result(&v, Some(text(&v["error"]["message"]))),
            "error" => self.result(&v, Some(text(&v["message"]))),
            _ => ParsedLine::Other(v),
        })
    }

    fn completed(&mut self, v: Value) -> ParsedLine {
        if v["item"]["type"] == "agent_message" {
            self.last_text = Some(text(&v["item"]["text"]));
        }
        item(&v["item"], true).unwrap_or(ParsedLine::Other(v))
    }

    /// A Claude-shaped result line; `error` set means the turn failed.
    fn result(&self, v: &Value, error: Option<String>) -> ParsedLine {
        let is_error = error.is_some();
        let text = error.or_else(|| self.last_text.clone());
        let raw = json!({"type": "result", "cli": "codex", "result": text, "is_error": is_error, "usage": usage(&v["usage"])});
        ParsedLine::Result { session_id: None, text, is_error, raw }
    }
}

/// Codex usage in Claude's keys. Codex counts cached tokens inside input_tokens, so they are split out.
pub fn usage(u: &Value) -> Value {
    let n = |k: &str| u[k].as_i64().unwrap_or(0);
    let cached = n("cached_input_tokens");
    json!({
        "input_tokens": (n("input_tokens") - cached).max(0),
        "output_tokens": n("output_tokens"),
        "cache_read_input_tokens": cached,
        "cache_creation_input_tokens": 0,
    })
}

/// An item as Claude-shaped assistant text, tool_use (started) or tool_result (completed).
fn item(it: &Value, done: bool) -> Option<ParsedLine> {
    let id = text(&it["id"]);
    match (it["type"].as_str()?, done) {
        ("agent_message", true) => Some(assistant(json!({"type": "text", "text": text(&it["text"])}))),
        ("command_execution", false) => Some(tool_use(&id, "Bash", json!({"command": text(&it["command"])}))),
        ("command_execution", true) => Some(tool_result(&id, text(&it["aggregated_output"]), it["exit_code"].as_i64().unwrap_or(0) != 0)),
        ("mcp_tool_call", false) => Some(tool_use(&id, &mcp_name(it), it["arguments"].clone())),
        ("mcp_tool_call", true) => Some(tool_result(&id, mcp_output(it), it["status"] == "failed")),
        ("file_change", true) => Some(tool_use(&id, "Edit", json!({"changes": it["changes"].clone()}))),
        _ => None,
    }
}

fn mcp_name(it: &Value) -> String {
    format!("mcp__{}__{}", text(&it["server"]), text(&it["tool"]))
}

fn mcp_output(it: &Value) -> String {
    match it["error"]["message"].as_str() {
        Some(e) => e.to_string(),
        None => it["result"].to_string(),
    }
}

fn assistant(block: Value) -> ParsedLine {
    ParsedLine::Assistant(json!({"type": "assistant", "cli": "codex", "message": {"role": "assistant", "content": [block]}}))
}

fn tool_use(id: &str, name: &str, input: Value) -> ParsedLine {
    assistant(json!({"type": "tool_use", "id": id, "name": name, "input": input}))
}

fn tool_result(id: &str, content: String, is_error: bool) -> ParsedLine {
    let block = json!({"type": "tool_result", "tool_use_id": id, "content": content, "is_error": is_error});
    ParsedLine::ToolResult(json!({"type": "user", "cli": "codex", "message": {"role": "user", "content": [block]}}))
}

fn command(bin: &str, run: &AgentRun) -> Command {
    let mut cmd = Command::new(bin);
    cmd.args(build_args(run))
        .current_dir(&run.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    cmd
}

/// Runs Codex once, calling `on_line` for every parsed line.
pub async fn run(run: &AgentRun, on_line: impl FnMut(&ParsedLine)) -> anyhow::Result<RunOutcome> {
    let bin = std::env::var("RUI_CODEX_BIN").unwrap_or_else(|_| "codex".into());
    let mut parser = Parser::default();
    crate::runner::stream(command(&bin, run), &bin, "codex", |l| parser.parse(l), on_line).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(cwd: &Path) -> AgentRun {
        AgentRun {
            role: rui_core::Role::Builder,
            task_id: Some(4),
            cwd: cwd.to_path_buf(),
            prompt: "Fix the bug".into(),
            system_prompt: "You are a builder".into(),
            allowed_tools: vec!["Read".into()],
            builtin_tools: None,
            permission_mode: Some("acceptEdits".into()),
            permission_prompt_tool: None,
            mcp_url: "http://127.0.0.1:7/mcp/builder".into(),
            mcp_headers: vec![("X-Rui-Task-Id".into(), "4".into())],
            resume: Some("old".into()),
            partial: true,
            seat: None,
        }
    }

    #[test]
    fn args_run_exec_json_in_the_worktree_with_mcp() {
        let mut r = run(Path::new("/wt/4"));
        let a = build_args(&r);
        assert_eq!(a[..6], ["exec", "--json", "--skip-git-repo-check", "-C", "/wt/4", "--sandbox"]);
        assert_eq!(a[6], "workspace-write");
        assert_eq!(a[7..11], [
            "-c", "mcp_servers.rui.url=\"http://127.0.0.1:7/mcp/builder\"",
            "-c", "mcp_servers.rui.http_headers={\"X-Rui-Task-Id\" = \"4\"}",
        ]);
        assert_eq!(a.last().unwrap(), "You are a builder\n\n---\n\nFix the bug");
        assert_eq!(a.len(), 12, "no resume or tool flags: Codex does not take them");
        r.permission_mode = None;
        r.system_prompt = String::new();
        let a = build_args(&r);
        assert_eq!((a[6].as_str(), a.last().unwrap().as_str()), ("read-only", "Fix the bug"));
    }

    const FIXTURE: &str = r#"{"type":"thread.started","thread_id":"th_1"}
{"type":"turn.started"}
{"type":"item.started","item":{"id":"item_0","type":"command_execution","command":"bash -lc 'cargo test'","aggregated_output":"","exit_code":null,"status":"in_progress"}}
{"type":"item.completed","item":{"id":"item_0","type":"command_execution","command":"bash -lc 'cargo test'","aggregated_output":"1 failed","exit_code":101,"status":"failed"}}
{"type":"item.completed","item":{"id":"item_1","type":"reasoning","text":"thinking"}}
{"type":"item.started","item":{"id":"item_2","type":"mcp_tool_call","server":"rui","tool":"report_status","arguments":{"text":"halfway"},"status":"in_progress"}}
{"type":"item.completed","item":{"id":"item_2","type":"mcp_tool_call","server":"rui","tool":"report_status","arguments":{"text":"halfway"},"result":{"content":[]},"status":"completed"}}
{"type":"item.completed","item":{"id":"item_3","type":"file_change","changes":[{"path":"src/a.rs","kind":"update"}],"status":"completed"}}
{"type":"item.completed","item":{"id":"item_4","type":"agent_message","text":"Fixed the off-by-one."}}

not json
{"type":"turn.completed","usage":{"input_tokens":1200,"cached_input_tokens":1000,"output_tokens":300}}"#;

    #[test]
    fn parses_a_codex_stream_into_claude_shapes() {
        let mut p = Parser::default();
        let lines: Vec<ParsedLine> = FIXTURE.lines().filter_map(|l| p.parse(l)).collect();
        assert_eq!(lines.len(), 10, "blank and non-JSON lines are skipped");
        assert!(matches!(&lines[0], ParsedLine::Init { session_id, .. } if session_id == "th_1"));
        assert!(matches!(&lines[1], ParsedLine::Other(_)));
        let ParsedLine::Assistant(cmd) = &lines[2] else { panic!("command start is a tool_use") };
        assert_eq!((cmd["message"]["content"][0]["name"].as_str(), cmd["message"]["content"][0]["input"]["command"].as_str()), (Some("Bash"), Some("bash -lc 'cargo test'")));
        let ParsedLine::ToolResult(res) = &lines[3] else { panic!("command end is a tool_result") };
        let block = &res["message"]["content"][0];
        assert_eq!((block["tool_use_id"].as_str(), block["content"].as_str(), block["is_error"].as_bool()), (Some("item_0"), Some("1 failed"), Some(true)));
        assert!(matches!(&lines[4], ParsedLine::Other(_)), "reasoning is not stored as text");
        let ParsedLine::Assistant(mcp) = &lines[5] else { panic!() };
        assert_eq!(mcp["message"]["content"][0]["name"], "mcp__rui__report_status");
        assert!(matches!(&lines[6], ParsedLine::ToolResult(r) if r["message"]["content"][0]["is_error"] == false));
        assert!(matches!(&lines[7], ParsedLine::Assistant(e) if e["message"]["content"][0]["name"] == "Edit"));
        assert!(matches!(&lines[8], ParsedLine::Assistant(m) if m["message"]["content"][0]["text"] == "Fixed the off-by-one."));
        let ParsedLine::Result { text, is_error, raw, session_id } = &lines[9] else { panic!("expected result") };
        assert_eq!((text.as_deref(), *is_error, session_id), (Some("Fixed the off-by-one."), false, &None));
        let want = json!({"input_tokens": 200, "output_tokens": 300, "cache_read_input_tokens": 1000, "cache_creation_input_tokens": 0});
        assert_eq!(raw["usage"], want);
        assert!(raw.get("total_cost_usd").is_none(), "Codex reports no dollars");
    }

    #[test]
    fn failed_turns_are_error_results() {
        let mut p = Parser::default();
        let failed = p.parse(r#"{"type":"turn.failed","error":{"message":"usage limit reached"}}"#);
        assert!(matches!(failed, Some(ParsedLine::Result { is_error: true, text: Some(t), .. }) if t == "usage limit reached"));
        let err = p.parse(r#"{"type":"error","message":"stream closed"}"#);
        assert!(matches!(err, Some(ParsedLine::Result { is_error: true, .. })));
    }

    fn write_role(data: &Path, name: &str, cli: Option<&str>) {
        let dir = data.join("roles").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let cli = cli.map(|c| format!("cli: {c}\n")).unwrap_or_default();
        std::fs::write(dir.join("AGENT.md"), format!("---\nname: {name}\ndescription: d\n{cli}---\nBody\n")).unwrap();
    }

    #[test]
    fn role_cli_wins_over_the_pod_default() {
        let d = tempfile::tempdir().unwrap();
        write_role(d.path(), "backend", Some("codex"));
        write_role(d.path(), "frontend", None);
        write_role(d.path(), "qa", Some("claude"));
        assert_eq!(for_member(d.path(), "acme", "frontend"), Cli::Claude);
        assert_eq!(for_member(d.path(), "acme", "backend"), Cli::Codex);
        let pod = crate::client_state::Pod { cli: Some("codex".into()), ..Default::default() };
        crate::client_state::write_pod(d.path(), "acme", &pod).unwrap();
        assert_eq!(for_member(d.path(), "acme", "frontend"), Cli::Codex);
        assert_eq!(for_member(d.path(), "acme", "qa"), Cli::Claude);
        assert_eq!(for_member(d.path(), "acme", "missing"), Cli::Codex);
        assert_eq!(Cli::parse("bogus"), None);
    }
}

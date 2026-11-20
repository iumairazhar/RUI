//! Handles chat messages from the owner by running one Rui turn each.

use crate::db::Db;
use crate::live::{Live, LiveGuard, LiveKey, on_line};
use crate::runner::{AgentRun, run_agent};
use crate::scheduler::{Config, line_event};
use crate::roster;
use anyhow::{Context, Result};
use rui_core::{ChatMessage, ChatRole, Role};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedReceiver;

const RUI_PROMPT: &str = include_str!("../prompts/rui.md");
const RUI_TOOLS: &str = "mcp__rui__use_repo,mcp__rui__create_client,mcp__rui__set_up_pod,mcp__rui__create_task,mcp__rui__report_status,mcp__rui__run_apps,mcp__rui__stop_apps,mcp__rui__stop_task,mcp__rui__instruct_task,mcp__rui__ask_owner";

/// Processes chat message ids one at a time, forever.
pub async fn run(db: Arc<Db>, mut rx: UnboundedReceiver<i64>, cfg: Config, live: Live) {
    while let Some(id) = rx.recv().await {
        let msg = match load_message(&db, id) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("chat error: {e:#}");
                continue;
            }
        };
        if let Err(e) = handle(&db, &cfg, &live, msg.clone()).await {
            eprintln!("chat error: {e:#}");
            let _ = db.add_chat_message(msg.chat_id, ChatRole::Rui, &format!("Sorry, that failed: {e:#}"));
        }
    }
}

fn load_message(db: &Db, id: i64) -> Result<ChatMessage> {
    db.get_chat_message(id)?.context("chat message not found")
}

/// Rui's prompt with the catalog of roles it can staff filled in.
fn system_prompt(data_dir: &Path) -> String {
    crate::prompts::fill(RUI_PROMPT, &[("role_catalog", roster::catalog(data_dir))])
}

/// Tools Rui may use without asking: its MCP tools plus read-only built-ins (not Bash).
fn allowed_tools() -> Vec<String> {
    let extra = ["Read", "Glob", "Grep", "WebSearch", "WebFetch"];
    RUI_TOOLS.split(',').chain(extra).map(String::from).collect()
}

/// The Rui chat run for one owner message. Its Read and Edit tools are denied the private
/// files, since Rui reads untrusted text (email, meetings, web pages, PR comments).
fn chat_run(db: &Db, cfg: &Config, msg: ChatMessage) -> Result<AgentRun> {
    let (id, chat_id) = (msg.id, msg.chat_id);
    Ok(AgentRun {
        role: Role::Rui,
        task_id: None,
        cwd: cfg.data_dir.clone(),
        prompt: msg.content,
        system_prompt: system_prompt(&cfg.data_dir),
        allowed_tools: allowed_tools(),
        // Rui reads and researches with real tools; risky Bash calls go through
        // `mcp__rui__approve`, which holds them for the owner's OK.
        builtin_tools: Some("Bash,Read,Glob,Grep,WebSearch,WebFetch".into()),
        permission_mode: None,
        permission_prompt_tool: Some("mcp__rui__approve".into()),
        mcp_url: format!("http://127.0.0.1:{}/mcp/rui", cfg.port),
        grant: crate::run_tokens::RunGrant { server: crate::run_tokens::Server::Rui, task_id: None, source_ref: Some(format!("chat:{id}")) },
        resume: db.get_rui_session(chat_id)?,
        seat: None,
        sandbox: None,
        deny_secrets: Some(cfg.data_dir.clone()),
        partial: true,
    })
}

async fn handle(db: &Db, cfg: &Config, live: &Live, msg: ChatMessage) -> Result<()> {
    let chat_id = msg.chat_id;
    let run = chat_run(db, cfg, msg)?;
    let key = LiveKey::Chat(chat_id);
    let _guard = LiveGuard(live.clone(), key);
    let out = run_agent(&run, |p| {
        on_line(live, key, p);
        let Some((kind, payload)) = line_event(p) else { return };
        let _ = db.add_event_in_chat(chat_id, "rui", kind, &payload);
    })
    .await?;
    if let Some(sid) = &out.session_id {
        db.set_rui_session(chat_id, sid)?;
    }
    let reply = out.result_text.unwrap_or_default();
    db.add_chat_message(chat_id, ChatRole::Rui, &reply)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn chat_runs_get_the_secret_deny_settings() {
        let data = tempfile::tempdir().unwrap();
        let db = Db::open_test();
        let chat = db.create_chat().unwrap();
        let cfg = Config { port: 0, tick: std::time::Duration::from_secs(1), data_dir: data.path().to_path_buf() };
        let msg = ChatMessage { id: 1, chat_id: chat.id, role: ChatRole::User, content: "hi".into(), created_at: String::new() };
        let mut run = chat_run(&db, &cfg, msg).unwrap();
        run.cwd = data.path().to_path_buf();
        let v = crate::runner::tests::captured_settings(&run).await.expect("--settings passed");
        let want = crate::sandbox::secret_deny_rules(&std::fs::canonicalize(data.path()).unwrap());
        assert_eq!(v["permissions"]["deny"], serde_json::json!(want));
        assert!(run.sandbox.is_none(), "chat keeps the approve prompt, not the member sandbox");
    }

    #[test]
    fn prompt_and_tools_cover_stop_and_instruct() {
        for t in ["stop_task", "instruct_task", "ask_owner"] {
            assert!(RUI_PROMPT.contains(t), "{t} missing from rui.md");
            assert!(RUI_TOOLS.contains(&format!("mcp__rui__{t}")), "{t} missing from RUI_TOOLS");
        }
        assert!(!RUI_TOOLS.contains("approve"));
        assert!(!allowed_tools().iter().any(|t| t == "Bash" || t.ends_with("approve")));
        assert!(allowed_tools().iter().any(|t| t == "Read"));
        assert!(RUI_PROMPT.contains("duplicate"));
        assert!(RUI_PROMPT.contains("STOP"));
    }
}

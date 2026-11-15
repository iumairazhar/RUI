//! Handles chat messages from the owner by running one Rui turn each.

use crate::db::Db;
use crate::runner::{AgentRun, run_agent};
use crate::scheduler::{Config, line_event};
use anyhow::{Context, Result};
use rui_core::{ChatMessage, ChatRole, Role};
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedReceiver;

const RUI_PROMPT: &str = include_str!("../prompts/rui.md");
const RUI_TOOLS: &str = "mcp__rui__create_client,mcp__rui__create_task,mcp__rui__report_status";

/// Processes chat message ids one at a time, forever.
pub async fn run(db: Arc<Db>, mut rx: UnboundedReceiver<i64>, cfg: Config) {
    while let Some(id) = rx.recv().await {
        if let Err(e) = handle(&db, &cfg, id).await {
            eprintln!("chat error: {e:#}");
            let _ = db.add_chat_message(ChatRole::Rui, &format!("Sorry, that failed: {e:#}"));
        }
    }
}

fn load_message(db: &Db, id: i64) -> Result<ChatMessage> {
    let recent = db.list_chat(500)?;
    recent.into_iter().find(|m| m.id == id).context("chat message not found")
}

async fn handle(db: &Db, cfg: &Config, id: i64) -> Result<()> {
    let msg = load_message(db, id)?;
    let run = AgentRun {
        role: Role::Rui,
        task_id: None,
        cwd: cfg.data_dir.clone(),
        prompt: msg.content,
        system_prompt: RUI_PROMPT.into(),
        allowed_tools: RUI_TOOLS.split(',').map(String::from).collect(),
        // Rui only delegates: no shell, file or web tools, just its MCP tools.
        builtin_tools: Some(String::new()),
        permission_mode: None,
        mcp_url: format!("http://127.0.0.1:{}/mcp/rui", cfg.port),
        mcp_headers: vec![("X-Rui-Source-Ref".into(), format!("chat:{id}"))],
        resume: db.get_session(Role::Rui, None)?,
    };
    let out = run_agent(&run, |p| {
        let (kind, payload) = line_event(p);
        let _ = db.add_event(None, "rui", kind, &payload);
    })
    .await?;
    if let Some(sid) = &out.session_id {
        db.set_session(Role::Rui, None, sid)?;
    }
    let reply = out.result_text.unwrap_or_default();
    db.add_chat_message(ChatRole::Rui, &reply)?;
    Ok(())
}

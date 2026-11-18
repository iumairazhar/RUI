//! MCP servers for the two support agents: the Scout (`report`) and the Coach (`propose`, `done`).
//! Each exposes only its own tools. The task comes from `X-Rui-Task-Id`; a Scout run also sends
//! its request event id as `X-Rui-Source-Ref`.

use crate::coach::{self, Proposal};
use crate::db::Db;
use crate::mcp::{Ctx, NoArgs, ctx_from, finish, info};
use crate::scout::{self, Report};
use anyhow::Context;
use axum::http::request::Parts;
use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, tool::Extension, wrapper::Parameters},
    model::{CallToolResult, ServerConfig},
    schemars::JsonSchema,
    tool, tool_handler, tool_router,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Arguments for `report`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ReportArgs {
    /// The answer in two or three plain sentences, with the next step.
    pub summary: String,
    /// Short facts, one line each, each backed by a file path or source.
    #[serde(default)]
    pub findings: Vec<String>,
    /// File paths and URLs you relied on.
    #[serde(default)]
    pub sources: Vec<String>,
}

/// Arguments for `propose`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ProposeArgs {
    /// "playbook", "global_playbook", "role:<name>" or "skill:<name>".
    pub target: String,
    /// One short line to append. Never about done commands, tests, evals or protected paths.
    pub text: String,
    /// What happened in this task that shows the change is needed.
    pub why: String,
}

/// Tool logic as plain functions, unit tested without MCP.
pub mod logic {
    use super::*;

    fn task_id(ctx: &Ctx) -> anyhow::Result<i64> {
        ctx.task_id.context("missing X-Rui-Task-Id header")
    }

    /// The Scout's answer for the request named in `X-Rui-Source-Ref`.
    pub fn report(db: &Db, ctx: &Ctx, a: ReportArgs) -> anyhow::Result<Value> {
        let request: i64 = ctx.source_ref.as_deref().and_then(|s| s.parse().ok()).context("missing Scout request id")?;
        let r = Report { summary: a.summary, findings: a.findings, sources: a.sources };
        let id = scout::report(db, request, Some(task_id(ctx)?), r)?;
        Ok(json!({"ok": true, "report_event_id": id}))
    }

    /// One Coach proposal; refused when it touches done commands, tests, evals or protected paths.
    pub fn propose(db: &Db, data_dir: &Path, ctx: &Ctx, a: ProposeArgs) -> anyhow::Result<Value> {
        let p = Proposal { target: a.target, text: a.text, why: a.why };
        let id = coach::propose(db, data_dir, task_id(ctx)?, p)?;
        Ok(json!({"ok": true, "proposal_event_id": id}))
    }
}

/// MCP server for the Scout.
#[derive(Clone)]
pub struct ScoutServer {
    db: Arc<Db>,
    #[expect(dead_code, reason = "read by the tool_handler macro")]
    tool_router: ToolRouter<Self>,
}

impl ScoutServer {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db, tool_router: Self::tool_router() }
    }
}

#[tool_router]
impl ScoutServer {
    #[tool(description = "Finish: report your answer with a short summary, the findings and the sources. Call it once.")]
    async fn report(&self, Extension(parts): Extension<Parts>, Parameters(a): Parameters<ReportArgs>) -> CallToolResult {
        finish(logic::report(&self.db, &ctx_from(&parts), a))
    }
}

#[tool_handler]
impl ServerHandler for ScoutServer {
    fn get_info(&self) -> ServerConfig {
        info()
    }
}

/// MCP server for the Coach.
#[derive(Clone)]
pub struct CoachServer {
    db: Arc<Db>,
    data_dir: PathBuf,
    #[expect(dead_code, reason = "read by the tool_handler macro")]
    tool_router: ToolRouter<Self>,
}

impl CoachServer {
    pub fn new(db: Arc<Db>, data_dir: PathBuf) -> Self {
        Self { db, data_dir, tool_router: Self::tool_router() }
    }
}

#[tool_router]
impl CoachServer {
    #[tool(description = "Propose one small change for the owner to approve: target, one line of text, and why. At most 3.")]
    async fn propose(&self, Extension(parts): Extension<Parts>, Parameters(a): Parameters<ProposeArgs>) -> CallToolResult {
        finish(logic::propose(&self.db, &self.data_dir, &ctx_from(&parts), a))
    }

    #[tool(description = "Call when you have no more proposals.")]
    async fn done(&self, Parameters(_a): Parameters<NoArgs>) -> CallToolResult {
        finish(Ok(json!({"ok": true})))
    }
}

#[tool_handler]
impl ServerHandler for CoachServer {
    fn get_info(&self) -> ServerConfig {
        info()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_needs_the_request_header() {
        let db = Db::open_test();
        let ctx = Ctx { task_id: Some(1), source_ref: None };
        let a = ReportArgs { summary: "x".into(), findings: vec![], sources: vec![] };
        assert!(logic::report(&db, &ctx, a).is_err());
    }
}

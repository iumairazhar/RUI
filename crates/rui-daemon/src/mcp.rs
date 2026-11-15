//! MCP servers exposed to agents: one per role (rui, lead, builder).
//!
//! Tool logic lives in plain functions (`logic` module) so it can be unit
//! tested without MCP. The rmcp handlers below are thin wrappers.
//! Agent identity comes from HTTP headers (`X-Rui-Task-Id`, `X-Rui-Source-Ref`)
//! read from the request parts rmcp injects into the tool call.

use crate::db::{Db, NewTask};
use axum::http::request::Parts;
use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, tool::Extension, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, ServerCapabilities, ServerConfig},
    schemars::JsonSchema,
    tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use rui_core::{Role, SourceKind, Task, TaskStatus};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

/// Who is calling: read from the agent's HTTP headers.
#[derive(Debug, Clone, Default)]
pub struct Ctx {
    pub task_id: Option<i64>,
    pub source_ref: Option<String>,
}

/// Builds the router with `/mcp/rui`, `/mcp/lead` and `/mcp/builder`.
pub fn mcp_router(db: Arc<Db>) -> axum::Router {
    axum::Router::new()
        .nest_service("/mcp/rui", service(RuiServer::new(db.clone())))
        .nest_service("/mcp/lead", service(LeadServer::new(db.clone())))
        .nest_service("/mcp/builder", service(BuilderServer::new(db)))
}

fn service<S: ServerHandler + Clone + Send + 'static>(
    server: S,
) -> StreamableHttpService<S, LocalSessionManager> {
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None);
    StreamableHttpService::new(move || Ok(server.clone()), Default::default(), config)
}

fn ctx_from(parts: &Parts) -> Ctx {
    let header = |name: &str| {
        let v = parts.headers.get(name)?.to_str().ok()?.trim();
        (!v.is_empty()).then(|| v.to_string())
    };
    Ctx {
        task_id: header("x-rui-task-id").and_then(|v| v.parse().ok()),
        source_ref: header("x-rui-source-ref"),
    }
}

fn finish(res: anyhow::Result<Value>) -> CallToolResult {
    match res {
        Ok(v) => CallToolResult::success(vec![ContentBlock::text(v.to_string())]),
        Err(e) => CallToolResult::error(vec![ContentBlock::text(format!("{e:#}"))]),
    }
}

fn info() -> ServerConfig {
    ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
}

// ---- argument types ----

/// Arguments for `create_client`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CreateClientArgs {
    /// Unique client name.
    pub name: String,
    /// Absolute path (or ~/...) of an existing git repository.
    pub repo_path: String,
    /// Command that runs the repo's tests.
    pub test_command: Option<String>,
    /// Shell commands builders may run in this repo.
    pub allowed_commands: Option<Vec<String>>,
}

/// Arguments for the rui `create_task`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CreateTaskArgs {
    /// Name of an existing client.
    pub client_name: String,
    /// Short task title.
    pub title: String,
    /// Full task description.
    pub body: String,
}

/// Empty arguments.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct NoArgs {}

/// Arguments for `ask_question`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct AskQuestionArgs {
    /// The question for the human.
    pub text: String,
}

/// Arguments for `create_subtask`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CreateSubtaskArgs {
    /// Short subtask title.
    pub title: String,
    /// Full instructions for the builder.
    pub body: String,
    /// Command that must pass for the subtask to be done; defaults to the client's test command.
    pub done_command: Option<String>,
    /// Max build attempts, 1 to 20 (default 5).
    pub max_loops: Option<i64>,
    /// Ids of sibling subtasks that must finish first.
    pub depends_on: Option<Vec<i64>>,
}

/// Arguments for `report_progress`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ProgressArgs {
    /// Short progress note.
    pub note: String,
}

/// Arguments for `report_stuck`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct StuckArgs {
    /// Why you cannot continue.
    pub reason: String,
}

// ---- plain tool logic ----

/// Tool logic as plain functions: `(&Db, &Ctx, args) -> Result<Value>`.
pub mod logic {
    use super::*;

    fn record(db: &Db, ctx: &Ctx, role: Role, tool: &str, args: &impl Serialize) -> anyhow::Result<()> {
        let payload = json!({"tool": tool, "args": serde_json::to_value(args)?});
        db.add_event(ctx.task_id, role.as_str(), "tool_call", &payload)?;
        Ok(())
    }

    fn current_task(db: &Db, ctx: &Ctx) -> anyhow::Result<Task> {
        let id = ctx.task_id.ok_or_else(|| anyhow::anyhow!("missing X-Rui-Task-Id header"))?;
        db.get_task(id)?.ok_or_else(|| anyhow::anyhow!("unknown task {id}"))
    }

    fn expand_home(path: &str) -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_default();
        match path.strip_prefix("~/") {
            Some(rest) if !home.is_empty() => PathBuf::from(home).join(rest),
            _ => PathBuf::from(path),
        }
    }

    fn check_git_repo(path: &PathBuf) -> anyhow::Result<()> {
        if !path.is_dir() {
            anyhow::bail!("repo_path {} does not exist", path.display());
        }
        let ok = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["rev-parse", "--git-dir"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            anyhow::bail!("repo_path {} is not a git repository", path.display());
        }
        Ok(())
    }

    /// Registers a client after validating its repo path and name.
    pub fn create_client(db: &Db, ctx: &Ctx, a: CreateClientArgs) -> anyhow::Result<Value> {
        let name = a.name.trim();
        if name.is_empty() {
            anyhow::bail!("name must not be empty");
        }
        let path = expand_home(&a.repo_path);
        check_git_repo(&path)?;
        if db.get_client_by_name(name)?.is_some() {
            anyhow::bail!("client '{name}' already exists");
        }
        let cmds = a.allowed_commands.clone().unwrap_or_default();
        let repo = path.to_string_lossy().into_owned();
        let client = db.create_client(name, &repo, a.test_command.as_deref(), &cmds)?;
        record(db, ctx, Role::Rui, "create_client", &a)?;
        Ok(serde_json::to_value(client)?)
    }

    /// Creates a top-level lead task for an existing client.
    pub fn create_task(db: &Db, ctx: &Ctx, a: CreateTaskArgs) -> anyhow::Result<Value> {
        if a.title.trim().is_empty() {
            anyhow::bail!("title must not be empty");
        }
        let client = db
            .get_client_by_name(&a.client_name)?
            .ok_or_else(|| anyhow::anyhow!("client '{}' not found", a.client_name))?;
        let task = db.create_task(&NewTask {
            client_id: client.id,
            parent_id: None,
            title: a.title.clone(),
            body: a.body.clone(),
            status: TaskStatus::New,
            done_command: None,
            max_loops: 5,
            role: Role::Lead,
            source_kind: SourceKind::Chat,
            source_ref: ctx.source_ref.clone(),
        })?;
        record(db, ctx, Role::Rui, "create_task", &a)?;
        Ok(serde_json::to_value(task)?)
    }

    /// Latest done-check result for a task, or null if it never ran.
    fn last_check(db: &Db, task_id: i64) -> Value {
        let events = db.list_events(Some(task_id), 1000).unwrap_or_default();
        match events.iter().rev().find(|e| e.kind == "done_check") {
            Some(e) => json!({"passed": e.payload["passed"], "output_tail": e.payload["output_tail"],
                              "at": e.created_at}),
            None => Value::Null,
        }
    }

    /// Summarises clients with their tasks, plus open questions.
    pub fn report_status(db: &Db, ctx: &Ctx, a: NoArgs) -> anyhow::Result<Value> {
        let tasks = db.list_tasks()?;
        let clients: Vec<Value> = db
            .list_clients()?
            .iter()
            .map(|c| {
                let ts: Vec<Value> = tasks
                    .iter()
                    .filter(|t| t.client_id == c.id)
                    .map(|t| {
                        json!({"id": t.id, "title": t.title, "status": t.status,
                               "parent_id": t.parent_id, "loops_used": t.loops_used,
                               "max_loops": t.max_loops, "last_check": last_check(db, t.id)})
                    })
                    .collect();
                json!({"id": c.id, "name": c.name, "tasks": ts})
            })
            .collect();
        let open: Vec<Value> = db
            .list_questions(Some(rui_core::QuestionStatus::Open))?
            .iter()
            .map(|q| json!({"id": q.id, "task_id": q.task_id, "text": q.text}))
            .collect();
        record(db, ctx, Role::Rui, "report_status", &a)?;
        Ok(json!({"clients": clients, "open_questions": open}))
    }

    /// Lead asks the human a question and the task waits for the answer.
    pub fn ask_question(db: &Db, ctx: &Ctx, a: AskQuestionArgs) -> anyhow::Result<Value> {
        let task = current_task(db, ctx)?;
        let q = db.create_question(task.id, Role::Lead, &a.text)?;
        db.set_task_status(task.id, TaskStatus::WaitingAnswer)?;
        record(db, ctx, Role::Lead, "ask_question", &a)?;
        Ok(serde_json::to_value(q)?)
    }

    fn check_depends(db: &Db, parent: i64, deps: &[i64]) -> anyhow::Result<()> {
        let siblings: Vec<i64> = db.subtasks(parent)?.iter().map(|t| t.id).collect();
        match deps.iter().find(|d| !siblings.contains(d)) {
            Some(d) => anyhow::bail!("depends_on {d} is not a subtask of task {parent}"),
            None => Ok(()),
        }
    }

    /// Lead creates a builder subtask under its own task.
    pub fn create_subtask(db: &Db, ctx: &Ctx, a: CreateSubtaskArgs) -> anyhow::Result<Value> {
        let parent = current_task(db, ctx)?;
        let max_loops = a.max_loops.unwrap_or(5);
        if !(1..=20).contains(&max_loops) {
            anyhow::bail!("max_loops must be between 1 and 20");
        }
        let client = db
            .get_client(parent.client_id)?
            .ok_or_else(|| anyhow::anyhow!("client of task {} not found", parent.id))?;
        let done = a.done_command.clone().or(client.test_command);
        let Some(done) = done else {
            anyhow::bail!("no done_command given and the client has no test_command");
        };
        let deps = a.depends_on.clone().unwrap_or_default();
        check_depends(db, parent.id, &deps)?;
        let task = db.create_task(&NewTask {
            client_id: parent.client_id,
            parent_id: Some(parent.id),
            title: a.title.clone(),
            body: a.body.clone(),
            status: TaskStatus::Open,
            done_command: Some(done),
            max_loops,
            role: Role::Builder,
            source_kind: parent.source_kind,
            source_ref: Some(format!("task:{}", parent.id)),
        })?;
        for d in &deps {
            db.add_task_dep(task.id, *d)?;
        }
        record(db, ctx, Role::Lead, "create_subtask", &a)?;
        let fresh = db.get_task(task.id)?.unwrap_or(task);
        Ok(serde_json::to_value(fresh)?)
    }

    /// Lead declares the task split into its subtasks.
    pub fn mark_split(db: &Db, ctx: &Ctx, a: NoArgs) -> anyhow::Result<Value> {
        let task = current_task(db, ctx)?;
        let n = db.subtasks(task.id)?.len();
        if n == 0 {
            anyhow::bail!("task {} has no subtasks; create some first", task.id);
        }
        db.set_task_status(task.id, TaskStatus::Split)?;
        record(db, ctx, Role::Lead, "mark_split", &a)?;
        Ok(json!({"ok": true, "subtasks": n}))
    }

    /// Builder logs a progress note (event only).
    pub fn report_progress(db: &Db, ctx: &Ctx, a: ProgressArgs) -> anyhow::Result<Value> {
        let task = current_task(db, ctx)?;
        db.add_event(Some(task.id), "builder", "progress", &json!({"note": a.note}))?;
        record(db, ctx, Role::Builder, "report_progress", &a)?;
        Ok(json!({"ok": true}))
    }

    /// Builder reports it cannot continue; the task becomes stuck.
    pub fn report_stuck(db: &Db, ctx: &Ctx, a: StuckArgs) -> anyhow::Result<Value> {
        let task = current_task(db, ctx)?;
        db.set_task_status(task.id, TaskStatus::Stuck)?;
        db.add_event(Some(task.id), "builder", "stuck", &json!({"reason": a.reason}))?;
        record(db, ctx, Role::Builder, "report_stuck", &a)?;
        Ok(json!({"ok": true}))
    }
}

// ---- rmcp handlers ----

/// MCP server for the Rui (chat) role.
#[derive(Clone)]
pub struct RuiServer {
    db: Arc<Db>,
    #[expect(dead_code, reason = "read by the tool_handler macro")]
    tool_router: ToolRouter<Self>,
}

impl RuiServer {
    /// Creates the server over a shared database.
    pub fn new(db: Arc<Db>) -> Self {
        Self { db, tool_router: Self::tool_router() }
    }
}

#[tool_router]
impl RuiServer {
    #[tool(description = "Register a new client from an existing git repo on disk (repo_path must be a git repository).")]
    async fn create_client(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<CreateClientArgs>,
    ) -> CallToolResult {
        finish(logic::create_client(&self.db, &ctx_from(&parts), a))
    }

    #[tool(description = "Create a new top-level task for an existing client; a lead agent will pick it up.")]
    async fn create_task(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<CreateTaskArgs>,
    ) -> CallToolResult {
        finish(logic::create_task(&self.db, &ctx_from(&parts), a))
    }

    #[tool(description = "Get all clients with their tasks and statuses, plus open questions.")]
    async fn report_status(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<NoArgs>,
    ) -> CallToolResult {
        finish(logic::report_status(&self.db, &ctx_from(&parts), a))
    }
}

#[tool_handler]
impl ServerHandler for RuiServer {
    fn get_info(&self) -> ServerConfig {
        info()
    }
}

/// MCP server for the Lead role.
#[derive(Clone)]
pub struct LeadServer {
    db: Arc<Db>,
    #[expect(dead_code, reason = "read by the tool_handler macro")]
    tool_router: ToolRouter<Self>,
}

impl LeadServer {
    /// Creates the server over a shared database.
    pub fn new(db: Arc<Db>) -> Self {
        Self { db, tool_router: Self::tool_router() }
    }
}

#[tool_router]
impl LeadServer {
    #[tool(description = "Ask the human a question about this task; the task waits until it is answered.")]
    async fn ask_question(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<AskQuestionArgs>,
    ) -> CallToolResult {
        finish(logic::ask_question(&self.db, &ctx_from(&parts), a))
    }

    #[tool(description = "Create a builder subtask of this task; done_command defaults to the client's test command.")]
    async fn create_subtask(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<CreateSubtaskArgs>,
    ) -> CallToolResult {
        finish(logic::create_subtask(&self.db, &ctx_from(&parts), a))
    }

    #[tool(description = "Call once all subtasks are created to mark this task as split into them.")]
    async fn mark_split(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<NoArgs>,
    ) -> CallToolResult {
        finish(logic::mark_split(&self.db, &ctx_from(&parts), a))
    }
}

#[tool_handler]
impl ServerHandler for LeadServer {
    fn get_info(&self) -> ServerConfig {
        info()
    }
}

/// MCP server for the Builder role.
#[derive(Clone)]
pub struct BuilderServer {
    db: Arc<Db>,
    #[expect(dead_code, reason = "read by the tool_handler macro")]
    tool_router: ToolRouter<Self>,
}

impl BuilderServer {
    /// Creates the server over a shared database.
    pub fn new(db: Arc<Db>) -> Self {
        Self { db, tool_router: Self::tool_router() }
    }
}

#[tool_router]
impl BuilderServer {
    #[tool(description = "Log a short progress note about what you just did.")]
    async fn report_progress(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<ProgressArgs>,
    ) -> CallToolResult {
        finish(logic::report_progress(&self.db, &ctx_from(&parts), a))
    }

    #[tool(description = "Report that you cannot finish this task and explain why; marks the task stuck.")]
    async fn report_stuck(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<StuckArgs>,
    ) -> CallToolResult {
        finish(logic::report_stuck(&self.db, &ctx_from(&parts), a))
    }
}

#[tool_handler]
impl ServerHandler for BuilderServer {
    fn get_info(&self) -> ServerConfig {
        info()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rui_core::QuestionStatus;

    fn git_dir() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let ok = Command::new("git").arg("init").arg(d.path()).output().unwrap().status.success();
        assert!(ok);
        d
    }

    fn client_args(name: &str, path: &std::path::Path, test: Option<&str>) -> CreateClientArgs {
        CreateClientArgs {
            name: name.into(),
            repo_path: path.to_string_lossy().into_owned(),
            test_command: test.map(Into::into),
            allowed_commands: Some(vec!["ls".into()]),
        }
    }

    /// Db with one client and one top-level task; returns (db, ctx for that task).
    fn setup(test_command: Option<&str>) -> (Db, Ctx, tempfile::TempDir) {
        let db = Db::open_test();
        let dir = git_dir();
        let ctx = Ctx { task_id: None, source_ref: Some("chat:12".into()) };
        logic::create_client(&db, &ctx, client_args("Acme", dir.path(), test_command)).unwrap();
        let t = logic::create_task(
            &db,
            &ctx,
            CreateTaskArgs { client_name: "Acme".into(), title: "T".into(), body: "B".into() },
        )
        .unwrap();
        let ctx = Ctx { task_id: t["id"].as_i64(), source_ref: None };
        (db, ctx, dir)
    }

    fn sub_args(title: &str) -> CreateSubtaskArgs {
        CreateSubtaskArgs {
            title: title.into(),
            body: "b".into(),
            done_command: None,
            max_loops: None,
            depends_on: None,
        }
    }

    #[test]
    fn create_client_happy_path_and_duplicate() {
        let db = Db::open_test();
        let dir = git_dir();
        let ctx = Ctx::default();
        let c = logic::create_client(&db, &ctx, client_args("A", dir.path(), Some("cargo test"))).unwrap();
        assert_eq!(c["name"], "A");
        assert_eq!(c["test_command"], "cargo test");
        assert!(logic::create_client(&db, &ctx, client_args("A", dir.path(), None)).is_err());
    }

    #[test]
    fn create_client_rejects_non_git_dir() {
        let db = Db::open_test();
        let dir = tempfile::tempdir().unwrap();
        let err = logic::create_client(&db, &Ctx::default(), client_args("A", dir.path(), None));
        assert!(err.unwrap_err().to_string().contains("not a git repository"));
        assert!(db.list_clients().unwrap().is_empty());
    }

    #[test]
    fn create_task_sets_defaults_and_source_ref() {
        let (db, _, _d) = setup(None);
        let t = &db.list_tasks().unwrap()[0];
        assert_eq!(t.status, TaskStatus::New);
        assert_eq!(t.role, Role::Lead);
        assert_eq!(t.source_kind, SourceKind::Chat);
        assert_eq!(t.max_loops, 5);
        assert_eq!(t.source_ref.as_deref(), Some("chat:12"));
    }

    #[test]
    fn create_task_unknown_client_and_empty_title() {
        let db = Db::open_test();
        let ctx = Ctx::default();
        let a = CreateTaskArgs { client_name: "Nope".into(), title: "T".into(), body: "".into() };
        assert!(logic::create_task(&db, &ctx, a).is_err());
        let (db, ctx, _d) = setup(None);
        let a = CreateTaskArgs { client_name: "Acme".into(), title: " ".into(), body: "".into() };
        assert!(logic::create_task(&db, &ctx, a).is_err());
    }

    #[test]
    fn report_status_lists_tasks_and_questions() {
        let (db, ctx, _d) = setup(None);
        logic::ask_question(&db, &ctx, AskQuestionArgs { text: "why?".into() }).unwrap();
        let s = logic::report_status(&db, &ctx, NoArgs {}).unwrap();
        assert_eq!(s["clients"][0]["name"], "Acme");
        assert_eq!(s["clients"][0]["tasks"][0]["status"], "waiting_answer");
        assert_eq!(s["open_questions"][0]["text"], "why?");
        assert!(s["clients"][0]["tasks"][0]["last_check"].is_null());
        let id = s["clients"][0]["tasks"][0]["id"].as_i64().unwrap();
        db.add_event(Some(id), "scheduler", "done_check",
            &json!({"passed": true, "output_tail": "test result: ok"})).unwrap();
        let s = logic::report_status(&db, &ctx, NoArgs {}).unwrap();
        let c = &s["clients"][0]["tasks"][0]["last_check"];
        assert_eq!(c["passed"], true);
        assert_eq!(c["output_tail"], "test result: ok");
    }

    #[test]
    fn ask_question_flips_status() {
        let (db, ctx, _d) = setup(None);
        let q = logic::ask_question(&db, &ctx, AskQuestionArgs { text: "q".into() }).unwrap();
        assert_eq!(q["asked_by"], "lead");
        let task = db.get_task(ctx.task_id.unwrap()).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::WaitingAnswer);
        assert_eq!(db.list_questions(Some(QuestionStatus::Open)).unwrap().len(), 1);
    }

    #[test]
    fn create_subtask_defaults_from_client() {
        let (db, ctx, _d) = setup(Some("make test"));
        let t = logic::create_subtask(&db, &ctx, sub_args("s1")).unwrap();
        let pid = ctx.task_id.unwrap();
        assert_eq!(t["done_command"], "make test");
        assert_eq!(t["role"], "builder");
        assert_eq!(t["status"], "open");
        assert_eq!(t["parent_id"], pid);
        assert_eq!(t["source_ref"], format!("task:{pid}"));
        assert_eq!(t["max_loops"], 5);
    }

    #[test]
    fn create_subtask_needs_done_command() {
        let (db, ctx, _d) = setup(None);
        assert!(logic::create_subtask(&db, &ctx, sub_args("s")).is_err());
        let mut a = sub_args("s");
        a.done_command = Some("true".into());
        assert!(logic::create_subtask(&db, &ctx, a).is_ok());
    }

    #[test]
    fn create_subtask_validates_deps_and_loops() {
        let (db, ctx, _d) = setup(Some("t"));
        let first = logic::create_subtask(&db, &ctx, sub_args("a")).unwrap();
        let mut a = sub_args("b");
        a.depends_on = Some(vec![first["id"].as_i64().unwrap()]);
        let b = logic::create_subtask(&db, &ctx, a).unwrap();
        assert_eq!(b["depends_on"], json!([first["id"]]));
        let mut bad = sub_args("c");
        bad.depends_on = Some(vec![9999]);
        assert!(logic::create_subtask(&db, &ctx, bad).is_err());
        for n in [0, 21] {
            let mut a = sub_args("d");
            a.max_loops = Some(n);
            assert!(logic::create_subtask(&db, &ctx, a).is_err());
        }
    }

    #[test]
    fn mark_split_requires_subtasks() {
        let (db, ctx, _d) = setup(Some("t"));
        assert!(logic::mark_split(&db, &ctx, NoArgs {}).is_err());
        logic::create_subtask(&db, &ctx, sub_args("a")).unwrap();
        let r = logic::mark_split(&db, &ctx, NoArgs {}).unwrap();
        assert_eq!(r, json!({"ok": true, "subtasks": 1}));
        let t = db.get_task(ctx.task_id.unwrap()).unwrap().unwrap();
        assert_eq!(t.status, TaskStatus::Split);
    }

    #[test]
    fn builder_tools_write_events_and_status() {
        let (db, ctx, _d) = setup(None);
        logic::report_progress(&db, &ctx, ProgressArgs { note: "n".into() }).unwrap();
        logic::report_stuck(&db, &ctx, StuckArgs { reason: "r".into() }).unwrap();
        let t = db.get_task(ctx.task_id.unwrap()).unwrap().unwrap();
        assert_eq!(t.status, TaskStatus::Stuck);
        let ev = db.list_events(ctx.task_id, 50).unwrap();
        assert!(ev.iter().any(|e| e.kind == "progress"));
        assert!(ev.iter().any(|e| e.kind == "stuck"));
    }

    #[test]
    fn tool_call_events_are_written() {
        let (db, ctx, _d) = setup(None);
        logic::ask_question(&db, &ctx, AskQuestionArgs { text: "q".into() }).unwrap();
        let ev = db.list_events(ctx.task_id, 50).unwrap();
        let call = ev.iter().find(|e| e.kind == "tool_call").unwrap();
        assert_eq!(call.agent, "lead");
        assert_eq!(call.payload["tool"], "ask_question");
        assert_eq!(call.payload["args"]["text"], "q");
    }

    #[test]
    fn missing_or_unknown_task_errors() {
        let (db, _, _d) = setup(None);
        let none = Ctx::default();
        let err = logic::mark_split(&db, &none, NoArgs {}).unwrap_err();
        assert!(err.to_string().contains("X-Rui-Task-Id"));
        let bogus = Ctx { task_id: Some(9999), source_ref: None };
        assert!(logic::report_stuck(&db, &bogus, StuckArgs { reason: "r".into() }).is_err());
    }

    async fn post(app: &axum::Router, path: &str, body: Value) -> Value {
        use axum::body::Body;
        use axum::http::Request;
        use http_body_util::BodyExt;
        use tower::ServiceExt;
        let req = Request::post(path)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("host", "localhost")
            .body(Body::from(body.to_string()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let data = text.lines().find_map(|l| l.strip_prefix("data:")).unwrap_or(&text);
        serde_json::from_str(data.trim()).unwrap_or_else(|e| panic!("bad body {text:?}: {e}"))
    }

    #[tokio::test]
    async fn lead_endpoint_lists_three_tools() {
        let app = mcp_router(Arc::new(Db::open_test()));
        let init = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-03-26","capabilities":{},
            "clientInfo":{"name":"t","version":"1"}}});
        let r = post(&app, "/mcp/lead", init).await;
        assert!(r["result"].is_object(), "initialize failed: {r}");
        let list = json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}});
        let r = post(&app, "/mcp/lead", list).await;
        let mut names: Vec<String> = r["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("no tools: {r}"))
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        names.sort();
        assert_eq!(names, ["ask_question", "create_subtask", "mark_split"]);
    }

    #[tokio::test]
    #[ignore = "needs the real claude CLI and network"]
    async fn real_claude_creates_client() {
        use crate::runner::{AgentRun, run_agent};
        let db = Arc::new(Db::open_test());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(axum::serve(listener, mcp_router(db.clone())).into_future());
        let dir = git_dir();
        let run = AgentRun {
            role: Role::Rui,
            task_id: None,
            cwd: dir.path().to_path_buf(),
            prompt: format!(
                "Use the create_client tool to create a client named Demo with repo_path {}. Then reply done.",
                dir.path().display()
            ),
            system_prompt: "You are Rui.".into(),
            allowed_tools: vec!["mcp__rui__create_client".into()],
            builtin_tools: None,
            permission_mode: None,
            mcp_url: format!("http://127.0.0.1:{port}/mcp/rui"),
            mcp_headers: vec![],
            resume: None,
        };
        let out = run_agent(&run, |l| {
            if let crate::runner::ParsedLine::Other(v) | crate::runner::ParsedLine::Assistant(v) = l {
                println!("{v}");
            }
        })
        .await
        .unwrap();
        println!("result: {:?} is_error={}", out.result_text, out.is_error);
        assert!(db.get_client_by_name("Demo").unwrap().is_some());
    }
}

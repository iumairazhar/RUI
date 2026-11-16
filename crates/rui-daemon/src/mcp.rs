//! MCP servers exposed to agents: one per role (rui, lead, builder).
//!
//! Tool logic lives in plain functions (`logic` module) so it can be unit
//! tested without MCP. The rmcp handlers below are thin wrappers.
//! Agent identity comes from HTTP headers (`X-Rui-Task-Id`, `X-Rui-Source-Ref`)
//! read from the request parts rmcp injects into the tool call.

use crate::apps::{self, AppSpec, Apps};
use crate::db::{Db, NewTask};
use crate::roster;
use crate::scheduler;
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
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

/// Who is calling: read from the agent's HTTP headers.
#[derive(Debug, Clone, Default)]
pub struct Ctx {
    pub task_id: Option<i64>,
    pub source_ref: Option<String>,
}

/// Builds the router with `/mcp/rui`, `/mcp/lead` and `/mcp/builder`.
pub fn mcp_router(db: Arc<Db>, data_dir: PathBuf, apps: Apps) -> axum::Router {
    axum::Router::new()
        .nest_service("/mcp/rui", service(RuiServer::new(db.clone(), data_dir.clone(), apps)))
        .nest_service("/mcp/lead", service(LeadServer::new(db.clone(), data_dir.clone())))
        .nest_service("/mcp/builder", service(BuilderServer::new(db, data_dir)))
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

/// Arguments for `use_repo`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CreateClientArgs {
    /// Unique project name.
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

/// Arguments for `staff_task`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct StaffTaskArgs {
    /// Project name for client work. An existing project is reused; an unknown name creates a new one. Omit for a general task.
    pub project: Option<String>,
    /// Short task title.
    pub title: String,
    /// What the owner wants, in full. The Lead and every member see it.
    pub brief: String,
    /// Role names to staff, e.g. ["backend", "tester"]. Do not include "lead"; it is added automatically.
    pub pod: Vec<String>,
    /// Apps to run for this project. Always pass these for anything runnable.
    pub apps: Option<Vec<AppSpec>>,
}

/// Arguments for `run_apps` and `stop_apps`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ProjectArgs {
    /// Project name.
    pub project: String,
}

/// Empty arguments.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct NoArgs {}

/// Arguments for the Lead's `create_subtask`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CreateSubtaskArgs {
    /// Role name of the pod member who does this. Must be in the pod, never "lead".
    pub agent: String,
    /// Short subtask title.
    pub title: String,
    /// Full instructions for the member.
    pub job: String,
    /// Command that must pass for the subtask to be done; defaults to the client's test command.
    pub done_command: Option<String>,
    /// Ids of sibling subtasks that must finish first.
    pub depends_on: Option<Vec<i64>>,
}

/// Arguments for the member's `report_progress`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ProgressArgs {
    /// Short progress note.
    pub note: String,
}

/// Arguments for the member's `report_status`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct StatusArgs {
    /// One of DONE, DONE_WITH_CONCERNS, NEEDS_CONTEXT, BLOCKED.
    pub status: String,
    /// What you did, or what is wrong.
    pub note: String,
}

/// Arguments for `ask_lead`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct AskLeadArgs {
    /// The question for the Lead. Be specific.
    pub question: String,
}

/// Arguments for `rule`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct RuleArgs {
    /// Id of the question to answer.
    pub question_id: i64,
    /// The decision, written for the member who asked.
    pub answer: String,
    /// The reason behind the decision.
    pub why: String,
}

/// Arguments for `escalate` and `escalate_to_owner`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct EscalateArgs {
    /// Id of the question to pass up.
    pub question_id: i64,
    /// Why you cannot decide it.
    pub why: String,
}

/// Arguments for `set_limit`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SetLimitArgs {
    /// Subtask id.
    pub task_id: i64,
    /// New max build attempts, 1 to 20.
    pub max_loops: i64,
}

/// Arguments for `set_done_command`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SetDoneArgs {
    /// Subtask id.
    pub task_id: i64,
    /// New done command; must not be empty.
    pub done_command: String,
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
        record(db, ctx, Role::Rui, "use_repo", &a)?;
        Ok(serde_json::to_value(client)?)
    }

    /// Checks the pod and returns it without duplicates.
    fn validate_pod(data_dir: &Path, pod: &[String]) -> anyhow::Result<Vec<String>> {
        if pod.is_empty() {
            anyhow::bail!("pod must not be empty");
        }
        let mut clean: Vec<String> = Vec::new();
        for name in pod.iter().map(|n| n.trim()) {
            if name == "lead" {
                anyhow::bail!("do not put 'lead' in the pod; it is added automatically");
            }
            if roster::load(data_dir, name).is_none() {
                anyhow::bail!("unknown role '{name}'");
            }
            if !clean.iter().any(|c| c == name) {
                clean.push(name.to_string());
            }
        }
        Ok(clean)
    }

    /// Existing client by name (or by the slug of the name), else a fresh git project.
    fn project_client(db: &Db, data_dir: &Path, name: &str) -> anyhow::Result<rui_core::Client> {
        let slug = crate::projects::slug(name);
        for n in [name, slug.as_str()] {
            if let Some(c) = db.get_client_by_name(n)? {
                return Ok(c);
            }
        }
        let path = crate::projects::new_project(data_dir, name)?;
        db.create_client(&slug, &path.to_string_lossy(), None, &[])
    }

    fn write_apps(repo: &Path, apps: &[AppSpec]) -> anyhow::Result<()> {
        let dir = repo.join(".rui");
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("apps.json"), serde_json::to_string_pretty(apps)?)?;
        Ok(())
    }

    /// Staffs a pod: creates the project if needed and a Lead parent task that the scheduler starts.
    pub fn staff_task(db: &Db, data_dir: &Path, ctx: &Ctx, a: StaffTaskArgs) -> anyhow::Result<Value> {
        if a.title.trim().is_empty() {
            anyhow::bail!("title must not be empty");
        }
        let pod = validate_pod(data_dir, &a.pod)?;
        let project = a.project.as_deref().map(str::trim).filter(|p| !p.is_empty()).unwrap_or("general");
        let client = project_client(db, data_dir, project)?;
        let mut full_pod = vec!["lead".to_string()];
        full_pod.extend(pod);
        let parent = db.create_task(&NewTask {
            client_id: client.id,
            parent_id: None,
            title: a.title.clone(),
            body: a.brief.clone(),
            status: TaskStatus::New,
            done_command: None,
            max_loops: 5,
            role: Role::Lead,
            source_kind: SourceKind::Chat,
            source_ref: ctx.source_ref.clone(),
            agent: Some("lead".into()),
            pod: full_pod.clone(),
        })?;
        if let Some(apps) = a.apps.as_ref().filter(|x| !x.is_empty()) {
            write_apps(&expand_home(&client.repo_path), apps)?;
        }
        record(db, ctx, Role::Rui, "staff_task", &a)?;
        Ok(json!({"project": client.name, "repo_path": client.repo_path, "task_id": parent.id, "pod": full_pod}))
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
            agent: None,
            pod: vec![],
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

    /// Summarises clients with their tasks, plus open questions that wait for the owner.
    pub fn report_status(db: &Db, apps: &Apps, ctx: &Ctx, a: NoArgs) -> anyhow::Result<Value> {
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
                               "max_loops": t.max_loops, "agent": t.agent, "last_check": last_check(db, t.id)})
                    })
                    .collect();
                let running: Vec<Value> = apps::list(apps)
                    .iter()
                    .filter(|x| x.project == c.name)
                    .map(|x| json!({"name": x.name, "url": x.url, "status": x.status}))
                    .collect();
                json!({"id": c.id, "name": c.name, "tasks": ts, "apps": running})
            })
            .collect();
        let open: Vec<Value> = db
            .list_questions(Some(rui_core::QuestionStatus::Open))?
            .iter()
            .filter(|q| q.level == "owner")
            .map(|q| json!({"id": q.id, "task_id": q.task_id, "text": q.text}))
            .collect();
        record(db, ctx, Role::Rui, "report_status", &a)?;
        Ok(json!({"clients": clients, "open_questions": open}))
    }

    fn check_depends(db: &Db, parent: i64, deps: &[i64]) -> anyhow::Result<()> {
        let siblings: Vec<i64> = db.subtasks(parent)?.iter().map(|t| t.id).collect();
        match deps.iter().find(|d| !siblings.contains(d)) {
            Some(d) => anyhow::bail!("depends_on {d} is not a subtask of task {parent}"),
            None => Ok(()),
        }
    }

    /// The agent must be in the parent's pod. A parent with no pod (an old task) accepts any known role.
    fn check_agent(data_dir: &Path, parent: &Task, agent: &str) -> anyhow::Result<()> {
        if agent == "lead" {
            anyhow::bail!("the lead cannot be given a subtask");
        }
        if parent.pod.is_empty() && roster::load(data_dir, agent).is_some() {
            return Ok(());
        }
        if !parent.pod.iter().any(|p| p == agent) {
            anyhow::bail!("agent '{agent}' is not in this task's pod ({})", parent.pod.join(", "));
        }
        Ok(())
    }

    /// Lead creates a member subtask under its own task.
    pub fn create_subtask(db: &Db, data_dir: &Path, ctx: &Ctx, a: CreateSubtaskArgs) -> anyhow::Result<Value> {
        let parent = current_task(db, ctx)?;
        check_agent(data_dir, &parent, &a.agent)?;
        let client = db
            .get_client(parent.client_id)?
            .ok_or_else(|| anyhow::anyhow!("client of task {} not found", parent.id))?;
        let done = a.done_command.clone().or(client.test_command);
        let Some(done) = done.filter(|d| !d.trim().is_empty()) else {
            anyhow::bail!("no done_command given and the client has no test_command");
        };
        let deps = a.depends_on.clone().unwrap_or_default();
        check_depends(db, parent.id, &deps)?;
        let max_loops = roster::load(data_dir, &a.agent).map(|r| r.max_loops).unwrap_or(5);
        let task = db.create_task(&NewTask {
            client_id: parent.client_id,
            parent_id: Some(parent.id),
            title: a.title.clone(),
            body: a.job.clone(),
            status: TaskStatus::Open,
            done_command: Some(done),
            max_loops,
            role: Role::Builder,
            source_kind: parent.source_kind,
            source_ref: Some(format!("task:{}", parent.id)),
            agent: Some(a.agent.clone()),
            pod: vec![],
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

    /// A subtask of the calling Lead's own task.
    fn lead_child(db: &Db, ctx: &Ctx, task_id: i64) -> anyhow::Result<Task> {
        let parent = current_task(db, ctx)?;
        match db.get_task(task_id)? {
            Some(t) if t.parent_id == Some(parent.id) => Ok(t),
            _ => anyhow::bail!("task {task_id} is not a subtask of task {}", parent.id),
        }
    }

    /// Lead changes a subtask's loop limit.
    pub fn set_limit(db: &Db, ctx: &Ctx, a: SetLimitArgs) -> anyhow::Result<Value> {
        let task = lead_child(db, ctx, a.task_id)?;
        if !(1..=20).contains(&a.max_loops) {
            anyhow::bail!("max_loops must be between 1 and 20");
        }
        db.set_max_loops(task.id, a.max_loops)?;
        record(db, ctx, Role::Lead, "set_limit", &a)?;
        Ok(json!({"ok": true, "task_id": task.id, "max_loops": a.max_loops}))
    }

    /// Lead narrows or changes a subtask's done command.
    pub fn set_done_command(db: &Db, ctx: &Ctx, a: SetDoneArgs) -> anyhow::Result<Value> {
        let task = lead_child(db, ctx, a.task_id)?;
        if a.done_command.trim().is_empty() {
            anyhow::bail!("done_command must not be empty");
        }
        db.set_done_command(task.id, a.done_command.trim())?;
        record(db, ctx, Role::Lead, "set_done_command", &a)?;
        Ok(json!({"ok": true, "task_id": task.id}))
    }

    /// The open question a Lead or Rui is allowed to decide: right level, and for a Lead, its own member's.
    fn decidable(db: &Db, ctx: &Ctx, level: &str, id: i64) -> anyhow::Result<rui_core::Question> {
        let q = db.get_question(id)?.ok_or_else(|| anyhow::anyhow!("question {id} not found"))?;
        if q.status != rui_core::QuestionStatus::Open {
            anyhow::bail!("question {id} is already answered");
        }
        if q.level != level {
            anyhow::bail!("question {id} is at level '{}', not '{level}'", q.level);
        }
        if level == "lead" {
            let member = db.get_task(q.task_id)?;
            if member.and_then(|m| m.parent_id) != ctx.task_id {
                anyhow::bail!("question {id} belongs to another task");
            }
        }
        Ok(q)
    }

    /// Lead ("lead") or Rui ("rui") answers a question at their level.
    pub fn rule(db: &Db, ctx: &Ctx, level: &str, a: RuleArgs) -> anyhow::Result<Value> {
        let q = decidable(db, ctx, level, a.question_id)?;
        if a.answer.trim().is_empty() {
            anyhow::bail!("answer must not be empty");
        }
        let why = Some(a.why.trim()).filter(|w| !w.is_empty());
        scheduler::answer_question(db, q.id, a.answer.trim(), why, level)?;
        record(db, ctx, role_of(level), "rule", &a)?;
        Ok(json!({"ok": true}))
    }

    /// Lead ("lead") or Rui ("rui") passes a question up one level.
    pub fn escalate(db: &Db, ctx: &Ctx, level: &str, a: EscalateArgs) -> anyhow::Result<Value> {
        let q = decidable(db, ctx, level, a.question_id)?;
        let who = if level == "lead" { "Lead" } else { "Rui" };
        let up = scheduler::escalate_question(db, q.id, who, a.why.trim())?;
        record(db, ctx, role_of(level), "escalate", &a)?;
        Ok(json!({"ok": true, "level": up.level}))
    }

    fn role_of(level: &str) -> Role {
        if level == "lead" { Role::Lead } else { Role::Rui }
    }

    /// Member logs a progress note (event only).
    pub fn report_progress(db: &Db, ctx: &Ctx, a: ProgressArgs) -> anyhow::Result<Value> {
        let task = current_task(db, ctx)?;
        db.add_event(Some(task.id), "builder", "progress", &json!({"note": a.note}))?;
        record(db, ctx, Role::Builder, "report_progress", &a)?;
        Ok(json!({"ok": true}))
    }

    const STATUSES: [&str; 4] = ["DONE", "DONE_WITH_CONCERNS", "NEEDS_CONTEXT", "BLOCKED"];

    /// Member reports how its work ended (event only).
    pub fn member_status(db: &Db, ctx: &Ctx, a: StatusArgs) -> anyhow::Result<Value> {
        let task = current_task(db, ctx)?;
        if !STATUSES.contains(&a.status.as_str()) {
            anyhow::bail!("status must be one of {}", STATUSES.join(", "));
        }
        db.add_event(Some(task.id), "builder", "status", &json!({"status": a.status, "note": a.note}))?;
        record(db, ctx, Role::Builder, "report_status", &a)?;
        Ok(json!({"ok": true}))
    }

    /// Member asks the Lead; the member waits until the question is ruled.
    pub fn ask_lead(db: &Db, data_dir: &Path, ctx: &Ctx, a: AskLeadArgs) -> anyhow::Result<Value> {
        let task = current_task(db, ctx)?;
        if task.parent_id.is_none() {
            anyhow::bail!("only team members can ask the lead");
        }
        if a.question.trim().is_empty() {
            anyhow::bail!("question must not be empty");
        }
        let q = scheduler::raise_lead_question(db, data_dir, &task, a.question.trim())?;
        record(db, ctx, Role::Builder, "ask_lead", &a)?;
        Ok(serde_json::to_value(q)?)
    }
}

// ---- rmcp handlers ----

/// MCP server for the Rui (chat) role.
#[derive(Clone)]
pub struct RuiServer {
    db: Arc<Db>,
    data_dir: PathBuf,
    apps: Apps,
    #[expect(dead_code, reason = "read by the tool_handler macro")]
    tool_router: ToolRouter<Self>,
}

impl RuiServer {
    /// Creates the server over a shared database, the data dir (projects, roles) and the running apps.
    pub fn new(db: Arc<Db>, data_dir: PathBuf, apps: Apps) -> Self {
        Self { db, data_dir, apps, tool_router: Self::tool_router() }
    }
}

#[tool_router]
impl RuiServer {
    #[tool(description = "Register an existing git repo on disk as a project.")]
    async fn use_repo(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<CreateClientArgs>,
    ) -> CallToolResult {
        finish(logic::create_client(&self.db, &ctx_from(&parts), a))
    }

    #[tool(description = "Staff a task: pick the roles for the pod. A Lead is added automatically and splits the work among them. Omit project for a general task; a known project name reuses it, a new name creates it.")]
    async fn staff_task(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<StaffTaskArgs>,
    ) -> CallToolResult {
        finish(logic::staff_task(&self.db, &self.data_dir, &ctx_from(&parts), a))
    }

    #[tool(description = "Get all clients with their tasks and statuses, plus open questions for the owner.")]
    async fn report_status(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<NoArgs>,
    ) -> CallToolResult {
        finish(logic::report_status(&self.db, &self.apps, &ctx_from(&parts), a))
    }

    #[tool(description = "Start (or restart) a project's apps and wait until they answer. Returns each app's url and status.")]
    async fn run_apps(
        &self,
        Parameters(a): Parameters<ProjectArgs>,
    ) -> CallToolResult {
        finish(self.run_project(&a.project).await)
    }

    #[tool(description = "Stop all running apps of a project.")]
    async fn stop_apps(
        &self,
        Parameters(a): Parameters<ProjectArgs>,
    ) -> CallToolResult {
        apps::stop_project(&self.apps, &a.project);
        finish(Ok(json!({"ok": true})))
    }

    #[tool(description = "Answer an escalated team question yourself. Give the decision and the reason.")]
    async fn rule(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<RuleArgs>,
    ) -> CallToolResult {
        finish(logic::rule(&self.db, &ctx_from(&parts), "rui", a))
    }

    #[tool(description = "Pass an escalated team question to the owner when only they can decide it.")]
    async fn escalate_to_owner(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<EscalateArgs>,
    ) -> CallToolResult {
        finish(logic::escalate(&self.db, &ctx_from(&parts), "rui", a))
    }
}

impl RuiServer {
    async fn run_project(&self, project: &str) -> anyhow::Result<Value> {
        let client = self
            .db
            .get_client_by_name(project)?
            .ok_or_else(|| anyhow::anyhow!("project '{project}' not found"))?;
        let repo = crate::scheduler::expand_home(&client.repo_path);
        let infos = apps::start_project(&self.apps, &client.name, &repo).await;
        Ok(serde_json::to_value(infos)?)
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
    data_dir: PathBuf,
    #[expect(dead_code, reason = "read by the tool_handler macro")]
    tool_router: ToolRouter<Self>,
}

impl LeadServer {
    /// Creates the server over a shared database and the data dir (roles).
    pub fn new(db: Arc<Db>, data_dir: PathBuf) -> Self {
        Self { db, data_dir, tool_router: Self::tool_router() }
    }
}

#[tool_router]
impl LeadServer {
    #[tool(description = "Create a subtask of this task for one pod member; done_command defaults to the client's test command.")]
    async fn create_subtask(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<CreateSubtaskArgs>,
    ) -> CallToolResult {
        finish(logic::create_subtask(&self.db, &self.data_dir, &ctx_from(&parts), a))
    }

    #[tool(description = "Call once all subtasks are created to mark this task as split into them.")]
    async fn mark_split(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<NoArgs>,
    ) -> CallToolResult {
        finish(logic::mark_split(&self.db, &ctx_from(&parts), a))
    }

    #[tool(description = "Answer a team member's question. Give the decision and the reason.")]
    async fn rule(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<RuleArgs>,
    ) -> CallToolResult {
        finish(logic::rule(&self.db, &ctx_from(&parts), "lead", a))
    }

    #[tool(description = "Pass a team member's question up to Rui when you cannot decide it.")]
    async fn escalate(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<EscalateArgs>,
    ) -> CallToolResult {
        finish(logic::escalate(&self.db, &ctx_from(&parts), "lead", a))
    }

    #[tool(description = "Raise or lower a subtask's loop limit (1 to 20).")]
    async fn set_limit(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<SetLimitArgs>,
    ) -> CallToolResult {
        finish(logic::set_limit(&self.db, &ctx_from(&parts), a))
    }

    #[tool(description = "Change a subtask's done command, for example to narrow the criteria.")]
    async fn set_done_command(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<SetDoneArgs>,
    ) -> CallToolResult {
        finish(logic::set_done_command(&self.db, &ctx_from(&parts), a))
    }
}

#[tool_handler]
impl ServerHandler for LeadServer {
    fn get_info(&self) -> ServerConfig {
        info()
    }
}

/// MCP server for team members (builders).
#[derive(Clone)]
pub struct BuilderServer {
    db: Arc<Db>,
    data_dir: PathBuf,
    #[expect(dead_code, reason = "read by the tool_handler macro")]
    tool_router: ToolRouter<Self>,
}

impl BuilderServer {
    /// Creates the server over a shared database and the data dir (roles).
    pub fn new(db: Arc<Db>, data_dir: PathBuf) -> Self {
        Self { db, data_dir, tool_router: Self::tool_router() }
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

    #[tool(description = "Report how your work ended: DONE, DONE_WITH_CONCERNS, NEEDS_CONTEXT or BLOCKED, with a note.")]
    async fn report_status(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<StatusArgs>,
    ) -> CallToolResult {
        finish(logic::member_status(&self.db, &ctx_from(&parts), a))
    }

    #[tool(description = "Ask the Lead a question and wait for the answer. End your turn after calling it.")]
    async fn ask_lead(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(a): Parameters<AskLeadArgs>,
    ) -> CallToolResult {
        finish(logic::ask_lead(&self.db, &self.data_dir, &ctx_from(&parts), a))
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

    /// Data dir with a minimal role file for each name.
    fn data_with_roles(names: &[&str]) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for n in names {
            let dir = d.path().join("roles").join(n);
            std::fs::create_dir_all(&dir).unwrap();
            let text = format!("---\nname: {n}\ndescription: d\nmax_loops: 6\n---\nI am {n}.\n");
            std::fs::write(dir.join("AGENT.md"), text).unwrap();
        }
        d
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

    fn sub_args(agent: &str, title: &str) -> CreateSubtaskArgs {
        CreateSubtaskArgs {
            agent: agent.into(),
            title: title.into(),
            job: "b".into(),
            done_command: Some("true".into()),
            depends_on: None,
        }
    }

    fn staff_args(project: Option<&str>, pod: &[&str]) -> StaffTaskArgs {
        let app = AppSpec { name: "api".into(), dir: ".".into(), command: "run $PORT".into() };
        StaffTaskArgs {
            project: project.map(Into::into),
            title: "T".into(),
            brief: "B".into(),
            pod: pod.iter().map(|p| p.to_string()).collect(),
            apps: Some(vec![app]),
        }
    }

    /// A staffed pod: returns (db, data dir, ctx of the parent, ctx of its first member).
    fn staffed() -> (Db, tempfile::TempDir, Ctx, Ctx) {
        let (db, data) = (Db::open_test(), data_with_roles(&["backend", "tester"]));
        let ctx = Ctx { task_id: None, source_ref: Some("chat:3".into()) };
        let r = logic::staff_task(&db, data.path(), &ctx, staff_args(None, &["backend", "tester"])).unwrap();
        let lead = Ctx { task_id: r["task_id"].as_i64(), source_ref: None };
        let child = logic::create_subtask(&db, data.path(), &lead, sub_args("backend", "api")).unwrap();
        let member = Ctx { task_id: child["id"].as_i64(), source_ref: None };
        (db, data, lead, member)
    }

    fn status_of(db: &Db, ctx: &Ctx) -> TaskStatus {
        db.get_task(ctx.task_id.unwrap()).unwrap().unwrap().status
    }

    fn ask(db: &Db, data: &Path, member: &Ctx) -> i64 {
        let a = AskLeadArgs { question: "which db?".into() };
        logic::ask_lead(db, data, member, a).unwrap()["id"].as_i64().unwrap()
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
    fn staff_task_without_project_uses_general_and_adds_lead() {
        let (db, data) = (Db::open_test(), data_with_roles(&["backend", "tester"]));
        let ctx = Ctx { task_id: None, source_ref: Some("chat:3".into()) };
        let r = logic::staff_task(&db, data.path(), &ctx, staff_args(None, &["backend", "tester"])).unwrap();
        assert_eq!(r["project"], "general");
        assert_eq!(r["pod"], json!(["lead", "backend", "tester"]));
        assert!(data.path().join("projects/general/.git").exists());
        let apps = std::fs::read_to_string(data.path().join("projects/general/.rui/apps.json")).unwrap();
        assert!(apps.contains("run $PORT"));
        let t = db.get_task(r["task_id"].as_i64().unwrap()).unwrap().unwrap();
        assert_eq!((t.role, t.status, t.agent.as_deref()), (Role::Lead, TaskStatus::New, Some("lead")));
        assert_eq!(t.pod, ["lead", "backend", "tester"]);
        assert_eq!((t.body.as_str(), t.source_ref.as_deref()), ("B", Some("chat:3")));
        logic::staff_task(&db, data.path(), &ctx, staff_args(None, &["backend"])).unwrap();
        assert_eq!(db.list_clients().unwrap().len(), 1);
    }

    #[test]
    fn staff_task_with_project_creates_then_reuses_it() {
        let (db, data) = (Db::open_test(), data_with_roles(&["backend"]));
        let ctx = Ctx::default();
        let r = logic::staff_task(&db, data.path(), &ctx, staff_args(Some("Todo App"), &["backend"])).unwrap();
        assert_eq!(r["project"], "todo-app");
        logic::staff_task(&db, data.path(), &ctx, staff_args(Some("todo-app"), &["backend"])).unwrap();
        assert_eq!(db.list_clients().unwrap().len(), 1);
    }

    #[test]
    fn staff_task_rejects_bad_pods() {
        let (db, data) = (Db::open_test(), data_with_roles(&["backend"]));
        let ctx = Ctx::default();
        let err = logic::staff_task(&db, data.path(), &ctx, staff_args(None, &["nope"])).unwrap_err();
        assert!(err.to_string().contains("unknown role"));
        let err = logic::staff_task(&db, data.path(), &ctx, staff_args(None, &["backend", "lead"])).unwrap_err();
        assert!(err.to_string().contains("lead"));
        assert!(logic::staff_task(&db, data.path(), &ctx, staff_args(None, &[])).is_err());
        let mut a = staff_args(None, &["backend"]);
        a.title = " ".into();
        assert!(logic::staff_task(&db, data.path(), &ctx, a).is_err());
        assert!(db.list_clients().unwrap().is_empty());
        assert!(!data.path().join("projects").exists());
    }

    #[test]
    fn report_status_lists_tasks_and_owner_questions() {
        let (db, data, lead, member) = staffed();
        let qid = ask(&db, data.path(), &member);
        let s = logic::report_status(&db, &Apps::default(), &lead, NoArgs {}).unwrap();
        assert_eq!(s["clients"][0]["name"], "general");
        assert_eq!(s["clients"][0]["tasks"][1]["status"], "waiting_answer");
        assert_eq!(s["clients"][0]["tasks"][1]["agent"], "backend");
        assert_eq!(s["open_questions"], json!([]));
        db.set_question_level(qid, "owner", &[]).unwrap();
        let s = logic::report_status(&db, &Apps::default(), &lead, NoArgs {}).unwrap();
        assert_eq!(s["open_questions"][0]["text"], "which db?");
        let id = member.task_id.unwrap();
        db.add_event(Some(id), "scheduler", "done_check", &json!({"passed": true, "output_tail": "ok"})).unwrap();
        let s = logic::report_status(&db, &Apps::default(), &lead, NoArgs {}).unwrap();
        assert_eq!(s["clients"][0]["tasks"][1]["last_check"]["output_tail"], "ok");
    }

    #[test]
    fn create_subtask_sets_agent_role_and_limit() {
        let (db, _data, lead, member) = staffed();
        let t = db.get_task(member.task_id.unwrap()).unwrap().unwrap();
        let pid = lead.task_id.unwrap();
        assert_eq!((t.agent.as_deref(), t.role, t.status), (Some("backend"), Role::Builder, TaskStatus::Open));
        assert_eq!((t.parent_id, t.max_loops, t.done_command.as_deref()), (Some(pid), 6, Some("true")));
        assert_eq!(t.source_ref, Some(format!("task:{pid}")));
    }

    #[test]
    fn create_subtask_rejects_agents_outside_the_pod() {
        let (db, data, lead, _m) = staffed();
        let err = logic::create_subtask(&db, data.path(), &lead, sub_args("frontend", "x")).unwrap_err();
        assert!(err.to_string().contains("not in this task's pod"));
        assert!(logic::create_subtask(&db, data.path(), &lead, sub_args("lead", "x")).is_err());
        assert!(logic::create_subtask(&db, data.path(), &lead, sub_args("tester", "x")).is_ok());
    }

    #[test]
    fn create_subtask_needs_done_command_and_valid_deps() {
        let (db, ctx, _d) = setup(None);
        let data = data_with_roles(&["backend"]);
        let mut a = sub_args("backend", "s");
        a.done_command = None;
        assert!(logic::create_subtask(&db, data.path(), &ctx, a).is_err());
        let first = logic::create_subtask(&db, data.path(), &ctx, sub_args("backend", "a")).unwrap();
        let mut b = sub_args("backend", "b");
        b.depends_on = Some(vec![first["id"].as_i64().unwrap()]);
        let b = logic::create_subtask(&db, data.path(), &ctx, b).unwrap();
        assert_eq!(b["depends_on"], json!([first["id"]]));
        let mut bad = sub_args("backend", "c");
        bad.depends_on = Some(vec![9999]);
        assert!(logic::create_subtask(&db, data.path(), &ctx, bad).is_err());
    }

    #[test]
    fn mark_split_requires_subtasks() {
        let (db, _data, lead, _m) = staffed();
        let r = logic::mark_split(&db, &lead, NoArgs {}).unwrap();
        assert_eq!(r, json!({"ok": true, "subtasks": 1}));
        assert_eq!(status_of(&db, &lead), TaskStatus::Split);
        let (db, ctx, _d) = setup(Some("t"));
        assert!(logic::mark_split(&db, &ctx, NoArgs {}).is_err());
    }

    #[test]
    fn set_limit_and_done_command_edit_own_subtasks() {
        let (db, _data, lead, member) = staffed();
        let id = member.task_id.unwrap();
        logic::set_limit(&db, &lead, SetLimitArgs { task_id: id, max_loops: 9 }).unwrap();
        let a = SetDoneArgs { task_id: id, done_command: " make t ".into() };
        logic::set_done_command(&db, &lead, a).unwrap();
        let t = db.get_task(id).unwrap().unwrap();
        assert_eq!((t.max_loops, t.done_command.as_deref()), (9, Some("make t")));
        for n in [0, 21] {
            assert!(logic::set_limit(&db, &lead, SetLimitArgs { task_id: id, max_loops: n }).is_err());
        }
        assert!(logic::set_limit(&db, &lead, SetLimitArgs { task_id: lead.task_id.unwrap(), max_loops: 3 }).is_err());
        let a = SetDoneArgs { task_id: id, done_command: " ".into() };
        assert!(logic::set_done_command(&db, &lead, a).is_err());
    }

    #[test]
    fn member_status_logs_event_and_validates() {
        let (db, _data, _lead, member) = staffed();
        let a = StatusArgs { status: "DONE_WITH_CONCERNS".into(), note: "n".into() };
        logic::member_status(&db, &member, a).unwrap();
        let ev = db.list_events(member.task_id, 50).unwrap();
        let e = ev.iter().find(|e| e.kind == "status").unwrap();
        assert_eq!(e.payload["status"], "DONE_WITH_CONCERNS");
        let bad = StatusArgs { status: "FINE".into(), note: "n".into() };
        assert!(logic::member_status(&db, &member, bad).is_err());
        let progress = ProgressArgs { note: "p".into() };
        logic::report_progress(&db, &member, progress).unwrap();
        assert!(db.list_events(member.task_id, 50).unwrap().iter().any(|e| e.kind == "progress"));
    }

    #[test]
    fn ladder_walks_lead_then_rui_then_owner() {
        let (db, data, lead, member) = staffed();
        let qid = ask(&db, data.path(), &member);
        let q = db.get_question(qid).unwrap().unwrap();
        assert_eq!((q.level.as_str(), q.asked_by), ("lead", Role::Builder));
        assert_eq!(q.ladder.len(), 1);
        assert_eq!((q.ladder[0].who.as_str(), q.ladder[0].why.as_str()), ("backend 2", "which db?"));
        assert_eq!(status_of(&db, &member), TaskStatus::WaitingAnswer);
        let esc = |why: &str| EscalateArgs { question_id: qid, why: why.into() };
        assert!(logic::escalate(&db, &lead, "rui", esc("x")).is_err());
        logic::escalate(&db, &lead, "lead", esc("not sure")).unwrap();
        let q = db.get_question(qid).unwrap().unwrap();
        assert_eq!(q.level, "rui");
        assert_eq!(q.ladder[1].who, "Lead");
        let none = Ctx::default();
        logic::escalate(&db, &none, "rui", esc("owner call")).unwrap();
        let q = db.get_question(qid).unwrap().unwrap();
        assert_eq!((q.level.as_str(), q.ladder[2].who.as_str()), ("owner", "Rui"));
        assert_eq!(q.status, QuestionStatus::Open);
        assert!(logic::escalate(&db, &none, "rui", esc("again")).is_err());
        assert_eq!(status_of(&db, &member), TaskStatus::WaitingAnswer);
    }

    #[test]
    fn lead_rule_answers_and_reopens_member() {
        let (db, data, lead, member) = staffed();
        let qid = ask(&db, data.path(), &member);
        let a = RuleArgs { question_id: qid, answer: "use sqlite".into(), why: "simple".into() };
        logic::rule(&db, &lead, "lead", a).unwrap();
        let q = db.get_question(qid).unwrap().unwrap();
        assert_eq!((q.status, q.answer.as_deref(), q.why.as_deref()), (QuestionStatus::Answered, Some("use sqlite"), Some("simple")));
        assert_eq!(status_of(&db, &member), TaskStatus::Open);
        let pending = db.pending_instructions(member.task_id.unwrap()).unwrap();
        assert_eq!(pending.len(), 1);
        assert!(pending[0].1.contains("use sqlite"));
        let again = RuleArgs { question_id: qid, answer: "x".into(), why: "y".into() };
        assert!(logic::rule(&db, &lead, "lead", again).is_err());
    }

    #[test]
    fn rule_checks_level_and_ownership() {
        let (db, data, lead, member) = staffed();
        let qid = ask(&db, data.path(), &member);
        let a = || RuleArgs { question_id: qid, answer: "a".into(), why: "w".into() };
        assert!(logic::rule(&db, &Ctx::default(), "rui", a()).is_err());
        let other = Ctx { task_id: Some(member.task_id.unwrap()), source_ref: None };
        assert!(logic::rule(&db, &other, "lead", a()).is_err());
        assert!(logic::rule(&db, &lead, "lead", RuleArgs { answer: " ".into(), ..a() }).is_err());
        assert_eq!(db.get_question(qid).unwrap().unwrap().status, QuestionStatus::Open);
    }

    #[test]
    fn ask_lead_is_for_members_only() {
        let (db, data, lead, _m) = staffed();
        let a = AskLeadArgs { question: "q".into() };
        assert!(logic::ask_lead(&db, data.path(), &lead, a).is_err());
    }

    #[test]
    fn tool_call_events_are_written() {
        let (db, data, _lead, member) = staffed();
        ask(&db, data.path(), &member);
        let ev = db.list_events(member.task_id, 50).unwrap();
        let call = ev.iter().find(|e| e.kind == "tool_call").unwrap();
        assert_eq!(call.agent, "builder");
        assert_eq!(call.payload["tool"], "ask_lead");
        assert_eq!(call.payload["args"]["question"], "which db?");
    }

    #[test]
    fn missing_or_unknown_task_errors() {
        let (db, _, _d) = setup(None);
        let none = Ctx::default();
        let err = logic::mark_split(&db, &none, NoArgs {}).unwrap_err();
        assert!(err.to_string().contains("X-Rui-Task-Id"));
        let bogus = Ctx { task_id: Some(9999), source_ref: None };
        let a = StatusArgs { status: "DONE".into(), note: "r".into() };
        assert!(logic::member_status(&db, &bogus, a).is_err());
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

    async fn tool_names(app: &axum::Router, path: &str) -> Vec<String> {
        let init = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-03-26","capabilities":{},
            "clientInfo":{"name":"t","version":"1"}}});
        let r = post(app, path, init).await;
        assert!(r["result"].is_object(), "initialize failed: {r}");
        let list = json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}});
        let r = post(app, path, list).await;
        let mut names: Vec<String> = r["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("no tools: {r}"))
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        names.sort();
        names
    }

    #[tokio::test]
    async fn endpoints_list_their_tools() {
        let app = mcp_router(Arc::new(Db::open_test()), std::env::temp_dir(), Apps::default());
        let lead = ["create_subtask", "escalate", "mark_split", "rule", "set_done_command", "set_limit"];
        assert_eq!(tool_names(&app, "/mcp/lead").await, lead);
        let builder = ["ask_lead", "report_progress", "report_status"];
        assert_eq!(tool_names(&app, "/mcp/builder").await, builder);
        let rui = ["escalate_to_owner", "report_status", "rule", "run_apps", "staff_task", "stop_apps", "use_repo"];
        assert_eq!(tool_names(&app, "/mcp/rui").await, rui);
    }

    #[tokio::test]
    #[ignore = "needs the real claude CLI and network"]
    async fn real_claude_creates_client() {
        use crate::runner::{AgentRun, run_agent};
        let db = Arc::new(Db::open_test());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(axum::serve(listener, mcp_router(db.clone(), std::env::temp_dir(), Apps::default())).into_future());
        let dir = git_dir();
        let run = AgentRun {
            role: Role::Rui,
            task_id: None,
            cwd: dir.path().to_path_buf(),
            prompt: format!(
                "Use the use_repo tool to create a client named Demo with repo_path {}. Then reply done.",
                dir.path().display()
            ),
            system_prompt: "You are Rui.".into(),
            allowed_tools: vec!["mcp__rui__use_repo".into()],
            builtin_tools: None,
            permission_mode: None,
            mcp_url: format!("http://127.0.0.1:{port}/mcp/rui"),
            mcp_headers: vec![],
            resume: None,
            partial: false,
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

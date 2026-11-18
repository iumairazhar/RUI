//! The Coach: after a top-level task is done, it reads what went wrong along the way and
//! proposes small additions to a playbook, role or skill. The owner approves or dismisses each.
//!
//! Proposals are `coach_proposal` events; the owner's verdict is `coach_proposal_resolved`.
//! Approving only ever appends a line. Proposals that touch done commands, tests, evals or
//! protected paths are refused in code.

use crate::client_state;
use crate::db::Db;
use crate::inbox::InboxItem;
use crate::library::{self, Kind, LibError};
use crate::live::Live;
use crate::prompts;
use crate::protected::{self, Protected};
use crate::runner::AgentRun;
use crate::scheduler::{self, Action, Config};
use crate::scout::{clip, event_line};
use anyhow::{Context, Result, anyhow, bail};
use rui_core::{Event, Role, Task, TaskStatus};
use serde_json::json;
use std::collections::HashSet;
use std::path::Path;

const COACH_PROMPT: &str = include_str!("../prompts/coach.md");
const COACH_TOOLS: [&str; 2] = ["mcp__rui__propose", "mcp__rui__done"];
/// Coach turns sit in the running set under `-(RUN_KEY_BASE + task id)`.
const RUN_KEY_BASE: i64 = 1 << 41;
/// At most this many proposals per finished task.
pub const MAX_PROPOSALS: usize = 3;
const LESSON_KINDS: [&str; 7] =
    ["sent_back", "loop_limit", "protected_rejected", "agent_error", "rollup", "instruction", "approval_delivered"];
/// Words and phrases a proposal may not contain (checked lowercased).
const FORBIDDEN: [&str; 14] = [
    "done_command", "done command", "done-command", "eval file", "evals", "test file", "test config", "protected",
    "skip test", "skip the test", "delete test", "disable test", "weaken test", "edit test",
];

/// The running-set key of the Coach turn for one task.
pub fn run_key(task_id: i64) -> i64 {
    -(RUN_KEY_BASE + task_id)
}

fn is_coach_key(key: i64) -> bool {
    key <= -RUN_KEY_BASE && key > -(2 * RUN_KEY_BASE)
}

/// A lesson: the owner had to step in, work was sent back, or something stuck.
fn is_lesson(e: &Event) -> bool {
    match e.kind.as_str() {
        "rollup" => e.payload["status"] == "stuck",
        "instruction" => e.agent == "owner",
        _ => true,
    }
}

/// Lesson events from the task and its subtasks, oldest first.
pub fn lessons(db: &Db, task_id: i64) -> Result<Vec<Event>> {
    let mut ids = vec![task_id];
    ids.extend(db.subtasks(task_id)?.iter().map(|t| t.id));
    Ok(db.events_for_tasks(&ids, &LESSON_KINDS, 0, 500)?.into_iter().filter(is_lesson).collect())
}

/// Only tasks active this recently get a Coach turn, so old finished work isn't backfilled.
const RECENT_SECS: i64 = 24 * 3600;

/// Done top-level tasks with lessons, recent activity and no `coach_reviewed` event yet.
pub fn due(db: &Db) -> Result<Vec<i64>> {
    let seen: HashSet<i64> = db.events_of_kinds(&["coach_reviewed"])?.iter().filter_map(|e| e.task_id).collect();
    let mut out = Vec::new();
    for t in db.tasks_with_status(TaskStatus::Done)? {
        if t.parent_id.is_none() && !seen.contains(&t.id) && is_recent(db, t.id)? && !lessons(db, t.id)?.is_empty() {
            out.push(t.id);
        }
    }
    Ok(out)
}

/// True when the task last had activity within RECENT_SECS.
fn is_recent(db: &Db, task_id: i64) -> Result<bool> {
    let last = db.last_activity(&[task_id])?.into_values().max().unwrap_or(0);
    Ok(crate::thread::now_secs() - last <= RECENT_SECS)
}

/// At most one Coach turn at a time: the oldest due task.
pub fn plan(db: &Db, running: &HashSet<i64>) -> Result<Vec<Action>> {
    if running.iter().any(|k| is_coach_key(*k)) {
        return Ok(vec![]);
    }
    Ok(due(db)?.into_iter().take(1).map(|task_id| Action::Coach { task_id }).collect())
}

/// Runs one Coach turn, then marks the task reviewed whatever happened, so it runs once.
pub async fn run(db: &Db, cfg: &Config, live: &Live, task_id: i64) -> Result<()> {
    let res = match coach_run(db, cfg, task_id) {
        Ok(run) => scheduler::within(LIMIT, scheduler::run_logged(db, cfg, live, "coach", task_id, run)).await.map(|_| ()),
        Err(e) => Err(e),
    };
    db.add_event(Some(task_id), "coach", "coach_reviewed", &json!({"error": review_error(&res)}))?;
    res
}

/// How long one Coach turn may run.
pub const LIMIT: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// The `coach_reviewed` error: "timeout" for a turn that ran over, else the error text.
pub fn review_error(res: &Result<()>) -> Option<String> {
    let e = res.as_ref().err()?;
    Some(if e.is::<scheduler::TimedOut>() { "timeout".into() } else { format!("{e:#}") })
}

fn coach_run(db: &Db, cfg: &Config, task_id: i64) -> Result<AgentRun> {
    let task = db.get_task(task_id)?.context("task not found")?;
    let client = db.get_client(task.client_id)?.context("client not found")?;
    let system = prompts::fill(COACH_PROMPT, &coach_vars(db, &cfg.data_dir, &task, &client.name)?);
    Ok(AgentRun {
        role: Role::Rui,
        task_id: Some(task_id),
        cwd: cfg.data_dir.clone(),
        prompt: "Review the finished task in your instructions, propose up to 3 changes, then call done.".into(),
        system_prompt: system,
        allowed_tools: COACH_TOOLS.map(String::from).to_vec(),
        builtin_tools: Some(String::new()),
        permission_mode: None,
        permission_prompt_tool: None,
        mcp_url: format!("http://127.0.0.1:{}/mcp/coach", cfg.port),
        grant: crate::run_tokens::RunGrant::new(crate::run_tokens::Server::Coach, Some(task_id)),
        resume: None,
        partial: false,
        seat: None,
    })
}

/// Values for coach.md.
pub fn coach_vars(db: &Db, data_dir: &Path, task: &Task, client: &str) -> Result<Vec<(&'static str, String)>> {
    let lines: Vec<String> = lessons(db, task.id)?.iter().map(event_line).collect();
    let start = lines.len().saturating_sub(40);
    Ok(vec![
        ("title", task.title.clone()),
        ("brief", clip(&task.body, 4000)),
        ("lessons", lines[start..].join("\n")),
        ("client_playbook", client_state::read_playbook(data_dir, client)?),
        ("global_playbook", client_state::read_global_playbook(data_dir)?),
        ("library", library_names(data_dir)),
    ])
}

fn library_names(data_dir: &Path) -> String {
    let roles: Vec<String> = crate::roster::load_all(data_dir).into_iter().map(|r| r.name).collect();
    let skills: Vec<String> = crate::skills::load_all(data_dir).into_iter().map(|s| s.name).collect();
    format!("Roles: {}\nSkills: {}", roles.join(", "), skills.join(", "))
}

/// One proposed change.
pub struct Proposal {
    pub target: String,
    pub text: String,
    pub why: String,
}

/// Records a pending `coach_proposal` on the task after checking it. Returns the event id.
pub fn propose(db: &Db, data_dir: &Path, task_id: i64, p: Proposal) -> Result<i64> {
    let task = db.get_task(task_id)?.with_context(|| format!("unknown task {task_id}"))?;
    let client = db.get_client(task.client_id)?.context("client not found")?;
    check(data_dir, &client.name, &p)?;
    let made = db.events_for_tasks(&[task_id], &["coach_proposal"], 0, i64::MAX)?.len();
    if made >= MAX_PROPOSALS {
        bail!("at most {MAX_PROPOSALS} proposals per task");
    }
    let payload = json!({"target": p.target, "text": p.text.trim(), "why": p.why.trim(), "status": "pending"});
    db.add_event(Some(task_id), "coach", "coach_proposal", &payload)
}

/// Refuses empty or long text, unknown targets, and anything about done commands, tests, evals or protected paths.
pub fn check(data_dir: &Path, client: &str, p: &Proposal) -> Result<()> {
    if p.text.trim().is_empty() || p.why.trim().is_empty() {
        bail!("text and why must not be empty");
    }
    if p.text.chars().count() > 600 {
        bail!("text must be one short line (600 characters at most)");
    }
    check_target(data_dir, &p.target)?;
    let pod = client_state::read_pod(data_dir, client)?;
    if let Some(hit) = forbidden_mention(&format!("{} {}", p.target, p.text), &pod.protected) {
        bail!("proposals must not change done commands, tests, evals or protected paths (found \"{hit}\")");
    }
    Ok(())
}

fn check_target(data_dir: &Path, target: &str) -> Result<()> {
    match target.split_once(':') {
        None if target == "playbook" || target == "global_playbook" => Ok(()),
        Some(("role", name)) => library::item_dir(data_dir, Kind::Role, name).map(|_| ()).map_err(lib_err),
        Some(("skill", name)) => library::item_dir(data_dir, Kind::Skill, name).map(|_| ()).map_err(lib_err),
        _ => bail!("target must be playbook, global_playbook, role:<name> or skill:<name>"),
    }
}

fn lib_err(e: LibError) -> anyhow::Error {
    match e {
        LibError::Bad(m) | LibError::NotFound(m) | LibError::Conflict(m) => anyhow!(m),
        LibError::Other(e) => e,
    }
}

/// The first forbidden phrase, test or eval path, or pod-protected path named in `text`.
pub fn forbidden_mention(text: &str, pod_protected: &[String]) -> Option<String> {
    let lower = text.to_lowercase();
    if let Some(hit) = FORBIDDEN.iter().find(|w| lower.contains(*w)) {
        return Some(hit.to_string());
    }
    if let Some(hit) = pod_protected.iter().find(|p| !p.trim().is_empty() && text.contains(p.trim())) {
        return Some(hit.clone());
    }
    let rules = Protected { explicit: pod_protected.to_vec(), check_patterns: true, config: true, ..Protected::default() };
    let tokens = text.split(|c: char| c.is_whitespace() || "`'\"(),;:".contains(c));
    tokens.map(|t| t.trim_end_matches(['.', '!', '?'])).find(|t| is_path_hit(t, &rules)).map(String::from)
}

fn is_path_hit(token: &str, rules: &Protected) -> bool {
    let word = token.to_lowercase();
    if ["eval", "evals", "conftest.py"].contains(&word.as_str()) {
        return true;
    }
    (token.contains('/') || token.contains('.')) && protected::is_protected_new(token, rules)
}

/// Pending proposals, oldest first.
pub fn pending(db: &Db) -> Result<Vec<Event>> {
    let evs = db.events_of_kinds(&["coach_proposal", "coach_proposal_resolved"])?;
    let done: HashSet<i64> = evs.iter().filter_map(|e| e.payload["event_id"].as_i64()).collect();
    Ok(evs.into_iter().filter(|e| e.kind == "coach_proposal" && !done.contains(&e.id)).collect())
}

/// The pending proposal with this event id, if any.
pub fn find_pending(db: &Db, event_id: i64) -> Result<Option<Event>> {
    Ok(pending(db)?.into_iter().find(|e| e.id == event_id))
}

/// One Inbox item per pending proposal.
pub fn inbox_items(db: &Db) -> Result<Vec<InboxItem>> {
    Ok(pending(db)?.iter().map(inbox_item).collect())
}

fn inbox_item(e: &Event) -> InboxItem {
    let s = |k: &str| e.payload[k].as_str().unwrap_or_default().to_string();
    InboxItem {
        kind: "proposal",
        id: e.id,
        task_id: e.task_id,
        title: format!("Coach suggests a change to {}", s("target")),
        reason: s("why"),
        actions: vec!["approve".into(), "dismiss".into()],
        created_at: e.created_at.clone(),
        detail: Some(s("text")),
        link: None,
    }
}

/// Applies the proposal on "approve", then records the verdict.
pub fn resolve(db: &Db, data_dir: &Path, e: &Event, verdict: &str) -> Result<()> {
    if !["approve", "dismiss"].contains(&verdict) {
        bail!("verdict must be approve or dismiss");
    }
    if verdict == "approve" {
        apply(db, data_dir, e)?;
    }
    db.add_event(e.task_id, "owner", "coach_proposal_resolved", &json!({"event_id": e.id, "verdict": verdict}))?;
    Ok(())
}

/// Appends the proposal's text to its target. Nothing already there is changed.
fn apply(db: &Db, data_dir: &Path, e: &Event) -> Result<()> {
    let s = |k: &str| e.payload[k].as_str().unwrap_or_default().to_string();
    let (target, text, why) = (s("target"), s("text"), s("why"));
    match target.split_once(':') {
        None if target == "playbook" => client_state::append_playbook(data_dir, &task_client(db, e)?, &text),
        None if target == "global_playbook" => client_state::append_global_playbook(data_dir, &text),
        Some(("role", name)) => append_lesson(data_dir, Kind::Role, name, &text, &why),
        Some(("skill", name)) => append_lesson(data_dir, Kind::Skill, name, &text, &why),
        _ => bail!("unknown target {target}"),
    }
}

fn task_client(db: &Db, e: &Event) -> Result<String> {
    let task = db.get_task(e.task_id.context("proposal has no task")?)?.context("task not found")?;
    Ok(db.get_client(task.client_id)?.context("client not found")?.name)
}

/// Adds a bullet under "## Lessons" in a role's AGENT.md or skill's SKILL.md, through the library's versioned save.
fn append_lesson(data_dir: &Path, kind: Kind, name: &str, text: &str, why: &str) -> Result<()> {
    let dir = library::item_dir(data_dir, kind, name).map_err(lib_err)?;
    let new = with_lesson(&std::fs::read_to_string(dir.join(kind.file()))?, text);
    library::validate(kind, name, &new).map_err(lib_err)?;
    library::save(data_dir, kind, name, &new, "added a Coach lesson", Some(why)).map_err(lib_err)
}

/// `old` with one bullet added at the end of its "## Lessons" section (created at the end when missing).
pub fn with_lesson(old: &str, text: &str) -> String {
    let line = client_state::bullet(text);
    let head = "\n## Lessons";
    let Some(start) = old.find(head) else { return format!("{}\n\n## Lessons\n\n{line}", old.trim_end()) };
    let after = start + head.len();
    match old[after..].find("\n## ") {
        Some(rel) => format!("{}\n{line}{}", old[..after + rel].trim_end(), &old[after + rel..]),
        None => format!("{}\n{line}", old.trim_end()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timed_out_turn_is_reviewed_with_a_timeout_error() {
        assert_eq!(review_error(&Err(anyhow::Error::new(scheduler::TimedOut))).as_deref(), Some("timeout"));
        assert_eq!(review_error(&Err(anyhow::anyhow!("boom"))).as_deref(), Some("boom"));
        assert_eq!(review_error(&Ok(())), None);
    }
    use crate::db::NewTask;
    use rui_core::SourceKind;

    fn task(db: &Db, client: i64, parent: Option<i64>, status: TaskStatus) -> Task {
        db.create_task(&NewTask {
            client_id: client, parent_id: parent, title: "Plan".into(), body: "Build it".into(), status, done_command: None,
            max_loops: 5, role: Role::Lead, source_kind: SourceKind::User, source_ref: None, agent: None, pod: vec![],
        })
        .unwrap()
    }

    fn setup() -> (Db, tempfile::TempDir, Task) {
        let db = Db::open_test();
        let data = tempfile::tempdir().unwrap();
        let c = db.create_client("acme", "/tmp/acme", None, &[]).unwrap();
        client_state::ensure(data.path(), "acme", "/tmp/acme", None).unwrap();
        let t = task(&db, c.id, None, TaskStatus::Done);
        (db, data, t)
    }

    fn coach_plans(db: &Db) -> Vec<Action> {
        plan(db, &HashSet::new()).unwrap()
    }

    fn prop(target: &str, text: &str) -> Proposal {
        Proposal { target: target.into(), text: text.into(), why: "It happened twice".into() }
    }

    #[test]
    fn coach_runs_once_and_only_for_done_tasks_with_lessons() {
        let (db, _d, t) = setup();
        assert!(coach_plans(&db).is_empty());
        let open = task(&db, t.client_id, None, TaskStatus::Split);
        db.add_event(Some(open.id), "lead", "sent_back", &json!({})).unwrap();
        db.add_event(Some(t.id), "scout", "instruction", &json!({"text": "report"})).unwrap();
        assert!(coach_plans(&db).is_empty());
        let sub = task(&db, t.client_id, Some(t.id), TaskStatus::Done);
        db.add_event(Some(sub.id), "owner", "instruction", &json!({"text": "use tabs"})).unwrap();
        assert_eq!(coach_plans(&db), vec![Action::Coach { task_id: t.id }]);
        assert!(plan(&db, &HashSet::from([run_key(99)])).unwrap().is_empty());
        db.add_event(Some(t.id), "coach", "coach_reviewed", &json!({})).unwrap();
        assert!(coach_plans(&db).is_empty());
    }

    #[test]
    fn propose_rejects_done_commands_tests_and_unknown_targets() {
        let (db, d, t) = setup();
        let bad = [
            prop("playbook", "Loosen the done_command for UI work"),
            prop("playbook", "Update tests/test_api.py to match"),
            prop("playbook", "Change src/app.test.ts expectations"),
            prop("global_playbook", "Skip evals when slow"),
            prop("role:nobody", "Be careful"),
            prop("skill:nothing", "Be careful"),
            prop("tests", "Be careful"),
            prop("playbook", " "),
        ];
        for p in bad {
            assert!(propose(&db, d.path(), t.id, p).is_err());
        }
        assert!(pending(&db).unwrap().is_empty());
    }

    #[test]
    fn pod_protected_paths_are_refused() {
        assert!(forbidden_mention("Edit config/ci.yml first", &["config/ci.yml".into()]).is_some());
        assert!(forbidden_mention("Run the migration check before splitting database work", &[]).is_none());
        assert!(forbidden_mention("Ask the evaluator role to look at src/main.rs", &[]).is_none());
    }

    #[test]
    fn at_most_three_proposals_per_task() {
        let (db, d, t) = setup();
        for i in 0..3 {
            propose(&db, d.path(), t.id, prop("playbook", &format!("Rule {i}"))).unwrap();
        }
        assert!(propose(&db, d.path(), t.id, prop("playbook", "Rule 4")).is_err());
    }

    #[test]
    fn approve_appends_to_playbook_and_dismiss_leaves_it() {
        let (db, d, t) = setup();
        client_state::append_playbook(d.path(), "acme", "Owner rule").unwrap();
        let a = propose(&db, d.path(), t.id, prop("playbook", "Ask before adding a dependency")).unwrap();
        let b = propose(&db, d.path(), t.id, prop("global_playbook", "Prefer small diffs")).unwrap();
        assert_eq!(inbox_items(&db).unwrap().len(), 2);
        resolve(&db, d.path(), &find_pending(&db, b).unwrap().unwrap(), "dismiss").unwrap();
        assert!(!client_state::global_playbook_path(d.path()).exists());
        resolve(&db, d.path(), &find_pending(&db, a).unwrap().unwrap(), "approve").unwrap();
        let book = client_state::read_playbook(d.path(), "acme").unwrap();
        assert_eq!(book, "- Owner rule\n- Ask before adding a dependency\n");
        assert!(inbox_items(&db).unwrap().is_empty());
        assert!(find_pending(&db, a).unwrap().is_none());
    }

    #[test]
    fn approve_global_and_role_lessons_append_only() {
        let (db, d, t) = setup();
        let role_dir = d.path().join("roles/writer");
        std::fs::create_dir_all(&role_dir).unwrap();
        let agent = "---\nname: writer\ndescription: Writes\n---\n# Writer\n\nBody line.\n";
        std::fs::write(role_dir.join("AGENT.md"), agent).unwrap();
        let g = propose(&db, d.path(), t.id, prop("global_playbook", "Prefer small diffs")).unwrap();
        let r = propose(&db, d.path(), t.id, prop("role:writer", "Cite sources")).unwrap();
        for id in [g, r] {
            resolve(&db, d.path(), &find_pending(&db, id).unwrap().unwrap(), "approve").unwrap();
        }
        assert_eq!(client_state::read_global_playbook(d.path()).unwrap(), "- Prefer small diffs\n");
        let text = std::fs::read_to_string(role_dir.join("AGENT.md")).unwrap();
        assert!(text.starts_with(agent.trim_end()) && text.ends_with("## Lessons\n\n- Cite sources\n"));
        assert_eq!(library::versions(&role_dir).len(), 1);
    }

    #[test]
    fn lessons_go_at_the_end_of_their_section() {
        let old = "x\n## Lessons\n\n- one\n\n## Other\nkeep\n";
        assert_eq!(with_lesson(old, "two"), "x\n## Lessons\n\n- one\n- two\n\n## Other\nkeep\n");
        assert_eq!(with_lesson("x\n## Lessons\n\n- one\n", "two"), "x\n## Lessons\n\n- one\n- two\n");
    }

    #[test]
    fn vars_fill_every_placeholder() {
        let (db, d, t) = setup();
        db.add_event(Some(t.id), "lead", "sent_back", &json!({"remarks": "missing tests"})).unwrap();
        let vars = coach_vars(&db, d.path(), &t, "acme").unwrap();
        assert!(vars.iter().any(|(k, v)| *k == "lessons" && v.contains("sent_back")));
        assert!(prompts::unfilled(&prompts::fill(COACH_PROMPT, &vars)).is_empty());
    }
}

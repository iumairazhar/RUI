//! Stopping and redirecting tasks. Shared by the HTTP handlers and Rui's MCP tools.

use crate::db::Db;
use crate::scheduler::Runs;
use rui_core::{Role, Task, TaskStatus};
use serde_json::json;

/// Why a control call failed.
#[derive(Debug)]
pub enum ControlError {
    NotFound(String),
    Conflict(String),
    Other(anyhow::Error),
}

impl From<anyhow::Error> for ControlError {
    fn from(e: anyhow::Error) -> Self {
        ControlError::Other(e)
    }
}

impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ControlError::NotFound(m) | ControlError::Conflict(m) => write!(f, "{m}"),
            ControlError::Other(e) => write!(f, "{e:#}"),
        }
    }
}

type Res<T> = Result<T, ControlError>;

fn load(db: &Db, id: i64) -> Res<Task> {
    db.get_task(id)?.ok_or_else(|| ControlError::NotFound("task not found".into()))
}

/// Aborts the running turn of a task, if any.
pub fn abort_run(runs: &Runs, id: i64) {
    let handle = runs.lock().ok().and_then(|mut m| m.remove(&id));
    if let Some(h) = handle {
        h.abort();
    }
}

/// Aborts the run, marks the task stuck and logs a `stopped` event.
fn halt(db: &Db, runs: &Runs, id: i64, by: &str, source: Option<&str>, reason: Option<&str>) -> Res<()> {
    abort_run(runs, id);
    db.set_task_status(id, TaskStatus::Stuck)?;
    db.add_event(Some(id), by, "stopped", &json!({"by": by, "source": source, "reason": reason}))?;
    Ok(())
}

/// Stops one task that is neither done nor split.
pub fn stop_one(db: &Db, runs: &Runs, id: i64, by: &str, source: Option<&str>, reason: Option<&str>) -> Res<Task> {
    let task = load(db, id)?;
    if matches!(task.status, TaskStatus::Done | TaskStatus::Split) {
        return Err(ControlError::Conflict("task is already done or split".into()));
    }
    halt(db, runs, id, by, source, reason)?;
    load(db, id)
}

/// Where the owner does a taken-over step by hand.
#[derive(Debug, serde::Serialize)]
pub struct TakeOver {
    pub task: Task,
    pub worktree: Option<String>,
    pub branch: String,
}

/// Stops a subtask's agent without marking it stuck and hands the step to the owner.
/// A stuck team is resumed so the step can finish once it is handed back.
pub fn take_over(db: &Db, runs: &Runs, data_dir: &std::path::Path, id: i64) -> Res<TakeOver> {
    let task = load(db, id)?;
    if task.parent_id.is_none() || task.status == TaskStatus::Done {
        return Err(ControlError::Conflict("only an unfinished subtask can be taken over".into()));
    }
    if crate::scheduler::owner_hold(db, id)? == Some("takeover") {
        return Err(ControlError::Conflict("this step is already taken over".into()));
    }
    abort_run(runs, id);
    db.set_task_status(id, TaskStatus::Open)?;
    resume_chain(db, id, "owner")?;
    let wt = crate::scheduler::subtask_worktree(data_dir, id);
    let worktree = wt.exists().then(|| wt.to_string_lossy().into_owned());
    let branch = task.branch.clone().unwrap_or_else(|| crate::worktree::branch_name(id));
    db.add_event(Some(id), "owner", "taken_over", &json!({"worktree": worktree, "branch": branch}))?;
    Ok(TakeOver { task: load(db, id)?, worktree, branch })
}

/// Stops a task and, when it has subtasks, every subtask that is not done. Returns the stopped ids.
pub fn stop_tree(db: &Db, runs: &Runs, id: i64, by: &str, source: Option<&str>, reason: Option<&str>) -> Res<Vec<i64>> {
    let task = load(db, id)?;
    if task.status == TaskStatus::Done {
        return Err(ControlError::Conflict("task is already done".into()));
    }
    let mut ids = vec![id];
    ids.extend(db.subtasks(id)?.iter().filter(|s| s.status != TaskStatus::Done).map(|s| s.id));
    for i in &ids {
        halt(db, runs, *i, by, source, reason)?;
    }
    Ok(ids)
}

/// Drops a task: stops it with its team, then records that the owner dropped it.
pub fn drop_task(db: &Db, runs: &Runs, id: i64) -> Res<Vec<i64>> {
    let ids = stop_tree(db, runs, id, "owner", Some("Inbox"), Some("dropped"))?;
    // Every teammate is dropped too, so none of them lingers as "Stopped".
    for i in &ids {
        db.add_event(Some(*i), "owner", "dropped", &json!({"by": "owner"}))?;
    }
    Ok(ids)
}

/// Records the owner's verdict on landed work. "changes" sends the text back to the Lead.
pub fn review(db: &Db, runs: &Runs, data_dir: &std::path::Path, id: i64, verdict: &str, text: Option<&str>) -> Res<Task> {
    let task = load(db, id)?;
    if task.status != TaskStatus::Done || task.parent_id.is_some() {
        return Err(ControlError::Conflict("only finished top-level tasks can be reviewed".into()));
    }
    if verdict == "ok" {
        land_approved(db, data_dir, &task)?;
        db.add_event(Some(id), "owner", "reviewed", &json!({"verdict": "ok", "by": "owner"}))?;
        return load(db, id);
    }
    let text = text.map(str::trim).filter(|t| !t.is_empty());
    let text = text.ok_or_else(|| ControlError::Conflict("changes need a text".into()))?;
    db.add_event(Some(id), "owner", "reviewed", &json!({"verdict": "changes"}))?;
    queue_instruction(db, runs, &task, text, "owner")?;
    load(db, id)
}

/// Merges an approved task's branch into the base branch, for repos with no remote.
fn land_approved(db: &Db, data_dir: &std::path::Path, task: &Task) -> Res<()> {
    let Some(branch) = task.branch.as_deref() else { return Ok(()) };
    let client = db.get_client(task.client_id)?.ok_or_else(|| ControlError::NotFound("client not found".into()))?;
    let repo = crate::scheduler::expand_home(&client.repo_path);
    if crate::worktree::has_remote(&repo) {
        return Ok(());
    }
    let message = format!("rui: task {}: {}", task.id, task.title);
    let base = crate::client_state::base_for(data_dir, &client.name, &repo)?;
    match crate::worktree::land(&repo, branch, &base, &message) {
        Err(e) => Err(ControlError::Conflict(format!("could not merge: {e:#}"))),
        Ok(crate::worktree::LandResult::Merged) => {
            db.add_event(Some(task.id), "owner", "landed", &json!({"branch": branch}))?;
            crate::deploy::request(db, data_dir, task)?;
            Ok(())
        }
        Ok(crate::worktree::LandResult::Conflict(files)) => {
            db.add_event(Some(task.id), "owner", "land_failed", &json!({"files": files}))?;
            Err(ControlError::Conflict(format!("could not merge: conflict in {}", files.join(", "))))
        }
    }
}

/// Pushes one task into `todo`, if it is stuck, and remembers it in `changed`.
fn resume_one(db: &Db, t: &Task, todo: TaskStatus, changed: &mut Vec<Task>) -> Res<()> {
    if t.status != TaskStatus::Stuck {
        return Ok(());
    }
    db.set_task_status(t.id, todo)?;
    changed.push(t.clone());
    Ok(())
}

/// A stuck parent goes back to split (or new, with no subtasks) so the team can run again.
fn resume_parent(db: &Db, task: &Task, changed: &mut Vec<Task>) -> Res<()> {
    let Some(parent) = task.parent_id.map(|p| load(db, p)).transpose()? else { return Ok(()) };
    let todo = if db.subtasks(parent.id)?.is_empty() { TaskStatus::New } else { TaskStatus::Split };
    resume_one(db, &parent, todo, changed)
}

/// Resumes what a task needs to run: its stuck parent, its stuck dependencies (all the way
/// down) and its stuck siblings, so the parent is not stuck again by the next roll-up.
/// Done tasks are never touched. Logs one `resumed` event on the task when anything changed.
pub fn resume_chain(db: &Db, id: i64, by: &str) -> Res<Vec<Task>> {
    let task = load(db, id)?;
    let mut changed = Vec::new();
    resume_parent(db, &task, &mut changed)?;
    let mut seen = std::collections::HashSet::from([id]);
    let mut todo = vec![task.clone()];
    if let Some(pid) = task.parent_id {
        todo.extend(db.subtasks(pid)?.into_iter().filter(|s| s.id != id && s.status == TaskStatus::Stuck));
        for s in todo.iter().skip(1) {
            resume_one(db, s, TaskStatus::Open, &mut changed)?;
        }
    }
    while let Some(t) = todo.pop() {
        for dep in t.depends_on.iter().filter(|d| seen.insert(**d)) {
            let dep = load(db, *dep)?;
            resume_one(db, &dep, TaskStatus::Open, &mut changed)?;
            todo.push(dep);
        }
    }
    if !changed.is_empty() {
        let ids: Vec<i64> = changed.iter().map(|t| t.id).collect();
        db.add_event(Some(id), by, "resumed", &json!({"ids": ids, "by": by}))?;
    }
    Ok(changed)
}

/// Checks the range and stores a subtask's new loop limit. Shared by the Lead's tool and the owner's button.
pub fn apply_limit(db: &Db, task_id: i64, max_loops: i64) -> anyhow::Result<()> {
    if !(1..=20).contains(&max_loops) {
        anyhow::bail!("max_loops must be between 1 and 20");
    }
    db.set_max_loops(task_id, max_loops)
}

/// True when the newest cause that stopped the task is its loop limit.
fn stuck_on_limit(db: &Db, task: &Task) -> Res<bool> {
    const CAUSES: [&str; 5] = ["loop_limit", "stopped", "agent_error", "scheduler_error", "land_failed"];
    let evs = db.events_for_tasks(&[task.id], &CAUSES, 0, i64::MAX)?;
    Ok(task.status == TaskStatus::Stuck && evs.last().is_some_and(|e| e.kind == "loop_limit"))
}

/// The owner raises or lowers a subtask's loop limit. A subtask stuck on its limit that now has
/// loops left goes back to open, and its stuck team is resumed.
pub fn set_limit(db: &Db, id: i64, max_loops: i64) -> Res<Task> {
    let task = load(db, id)?;
    if task.parent_id.is_none() {
        return Err(ControlError::Conflict("only a subtask has a loop limit".into()));
    }
    apply_limit(db, id, max_loops).map_err(|e| ControlError::Conflict(e.to_string()))?;
    db.add_event(Some(id), "owner", "limit_set", &json!({"max_loops": max_loops, "by": "owner"}))?;
    if stuck_on_limit(db, &task)? && max_loops > task.loops_used {
        db.set_task_status(id, TaskStatus::Open)?;
        resume_chain(db, id, "owner")?;
    }
    load(db, id)
}

/// "Resuming A and B so C can continue.", or None when nothing was resumed.
fn resume_note(changed: &[Task], task: &Task) -> Option<String> {
    let names: Vec<&str> = changed.iter().map(|t| t.title.as_str()).collect();
    let (last, rest) = names.split_last()?;
    let list = if rest.is_empty() { last.to_string() } else { format!("{} and {last}", rest.join(", ")) };
    Some(format!("Resuming {list} so {} can continue.", task.title))
}

/// Aborts any run, saves the instruction and queues the task again (a Lead gets a new turn).
fn queue_instruction(db: &Db, runs: &Runs, task: &Task, text: &str, by: &str) -> Res<()> {
    abort_run(runs, task.id);
    db.add_event(Some(task.id), by, "instruction", &json!({"text": text}))?;
    db.set_task_status(task.id, if task.role == Role::Lead { TaskStatus::New } else { TaskStatus::Open })?;
    Ok(())
}

/// Saves an instruction, aborts any run and queues the task again, resuming whatever it
/// needs first. A Lead task (a parent, even a split one) gets a new Lead turn that carries it.
/// The note says what was resumed, if anything.
pub fn instruct(db: &Db, runs: &Runs, id: i64, text: &str, by: &str) -> Res<(Task, Option<String>)> {
    let task = load(db, id)?;
    let blocked = match task.status {
        TaskStatus::Done | TaskStatus::WaitingAnswer => true,
        TaskStatus::Split => task.role != Role::Lead,
        _ => false,
    };
    if blocked {
        return Err(ControlError::Conflict("task cannot take instructions in its current status".into()));
    }
    queue_instruction(db, runs, &task, text, by)?;
    let changed = resume_chain(db, id, by)?;
    Ok((load(db, id)?, resume_note(&changed, &task)))
}

/// Wakes the Lead of a finished top-level task with an instruction, like feedback on its PR.
/// Same path as `instruct`, but a done task is allowed: it goes back to new for a Lead turn.
pub fn wake_lead(db: &Db, runs: &Runs, id: i64, text: &str, by: &str) -> Res<Task> {
    let task = load(db, id)?;
    if task.parent_id.is_some() || task.role != Role::Lead {
        return Err(ControlError::Conflict("only a top-level Lead task can be woken".into()));
    }
    queue_instruction(db, runs, &task, text, by)?;
    load(db, id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::NewTask;
    use rui_core::SourceKind;

    fn task(db: &Db, client: i64, parent: Option<i64>, role: Role, status: TaskStatus) -> Task {
        db.create_task(&NewTask {
            client_id: client,
            parent_id: parent,
            title: "t".into(),
            body: String::new(),
            status,
            done_command: Some("true".into()),
            max_loops: 5,
            role,
            source_kind: SourceKind::User,
            source_ref: None,
            agent: None,
            pod: vec![],
        })
        .unwrap()
    }

    fn pod(db: &Db) -> (Task, Vec<Task>) {
        let c = db.create_client("c", "/tmp/c", None, &[]).unwrap();
        let parent = task(db, c.id, None, Role::Lead, TaskStatus::Split);
        let subs = [TaskStatus::Open, TaskStatus::InProgress, TaskStatus::Done]
            .map(|st| task(db, c.id, Some(parent.id), Role::Builder, st))
            .to_vec();
        (parent, subs)
    }

    #[test]
    fn stop_tree_stops_parent_and_unfinished_children() {
        let (db, runs) = (Db::open_test(), Runs::default());
        let (parent, subs) = pod(&db);
        let ids = stop_tree(&db, &runs, parent.id, "rui", Some("Rui chat"), Some("owner changed course")).unwrap();
        assert_eq!(ids, [parent.id, subs[0].id, subs[1].id]);
        let status = |id| db.get_task(id).unwrap().unwrap().status;
        assert_eq!((status(parent.id), status(subs[0].id), status(subs[1].id), status(subs[2].id)),
            (TaskStatus::Stuck, TaskStatus::Stuck, TaskStatus::Stuck, TaskStatus::Done));
        let ev = db.list_events(Some(subs[0].id), 5).unwrap();
        assert!(ev.iter().any(|e| e.kind == "stopped" && e.payload["reason"] == "owner changed course" && e.payload["source"] == "Rui chat"));
        assert!(matches!(stop_tree(&db, &runs, subs[2].id, "rui", None, None), Err(ControlError::Conflict(_))));
        assert!(matches!(stop_tree(&db, &runs, 999, "rui", None, None), Err(ControlError::NotFound(_))));
    }

    #[tokio::test]
    async fn stop_tree_aborts_running_turns() {
        let (db, runs) = (Db::open_test(), Runs::default());
        let (parent, subs) = pod(&db);
        let join = tokio::spawn(tokio::time::sleep(std::time::Duration::from_secs(3600)));
        runs.lock().unwrap().insert(subs[1].id, join.abort_handle());
        stop_tree(&db, &runs, parent.id, "rui", None, None).unwrap();
        assert!(join.await.unwrap_err().is_cancelled());
    }

    #[test]
    fn instruct_on_a_split_parent_queues_a_lead_turn() {
        let (db, runs) = (Db::open_test(), Runs::default());
        let (parent, subs) = pod(&db);
        let (t, note) = instruct(&db, &runs, parent.id, "exam is Thursday", "rui").unwrap();
        assert_eq!(note, None);
        assert_eq!(t.status, TaskStatus::New);
        assert_eq!(db.pending_instructions(parent.id).unwrap().len(), 1);
        let actions = crate::scheduler::plan_tick(&db, &Default::default()).unwrap();
        assert!(actions.iter().any(|a| matches!(a, crate::scheduler::Action::StartLead { task_id, .. } if *task_id == parent.id)));
        assert!(matches!(instruct(&db, &runs, subs[2].id, "x", "rui"), Err(ControlError::Conflict(_))));
    }

    fn titled(db: &Db, client: i64, parent: Option<i64>, role: Role, status: TaskStatus, title: &str) -> Task {
        db.create_task(&NewTask {
            client_id: client,
            parent_id: parent,
            title: title.into(),
            body: String::new(),
            status,
            done_command: None,
            max_loops: 5,
            role,
            source_kind: SourceKind::User,
            source_ref: None,
            agent: None,
            pod: vec![],
        })
        .unwrap()
    }

    #[test]
    fn instruct_resumes_stopped_lead_and_dependency() {
        let (db, runs) = (Db::open_test(), Runs::default());
        let c = db.create_client("c", "/tmp/c", None, &[]).unwrap();
        let lead = titled(&db, c.id, None, Role::Lead, TaskStatus::Stuck, "Pod Lead");
        let research = titled(&db, c.id, Some(lead.id), Role::Builder, TaskStatus::Stuck, "Researcher 01");
        let writer = titled(&db, c.id, Some(lead.id), Role::Builder, TaskStatus::Open, "Technical Writer 01");
        let done = titled(&db, c.id, Some(lead.id), Role::Builder, TaskStatus::Done, "Done one");
        db.add_task_dep(writer.id, research.id).unwrap();
        let (t, note) = instruct(&db, &runs, writer.id, "go on", "owner").unwrap();
        let status = |id| db.get_task(id).unwrap().unwrap().status;
        assert_eq!(t.status, TaskStatus::Open);
        assert_eq!((status(lead.id), status(research.id), status(done.id)), (TaskStatus::Split, TaskStatus::Open, TaskStatus::Done));
        assert_eq!(note.unwrap(), "Resuming Pod Lead and Researcher 01 so Technical Writer 01 can continue.");
        let ev = db.events_of_kinds(&["resumed"]).unwrap();
        assert_eq!((ev.len(), ev[0].task_id, ev[0].payload["ids"].clone()), (1, Some(writer.id), json!([lead.id, research.id])));
    }

    #[test]
    fn drop_marks_the_whole_team_dropped() {
        let (db, runs) = (Db::open_test(), Runs::default());
        let c = db.create_client("c", "/tmp/c", None, &[]).unwrap();
        let lead = titled(&db, c.id, None, Role::Lead, TaskStatus::Stuck, "Pod Lead");
        let research = titled(&db, c.id, Some(lead.id), Role::Builder, TaskStatus::Stuck, "Researcher 01");
        drop_task(&db, &runs, lead.id).unwrap();
        let dropped: Vec<Option<i64>> = db.events_of_kinds(&["dropped"]).unwrap().iter().map(|e| e.task_id).collect();
        assert_eq!(dropped, vec![Some(lead.id), Some(research.id)]);
    }

    #[test]
    fn review_and_drop() {
        let (db, runs) = (Db::open_test(), Runs::default());
        let (parent, subs) = pod(&db);
        assert!(matches!(review(&db, &runs, std::path::Path::new("/nonexistent-data"), parent.id, "ok", None), Err(ControlError::Conflict(_))));
        db.set_task_status(parent.id, TaskStatus::Done).unwrap();
        assert!(matches!(review(&db, &runs, std::path::Path::new("/nonexistent-data"), parent.id, "changes", Some(" ")), Err(ControlError::Conflict(_))));
        review(&db, &runs, std::path::Path::new("/nonexistent-data"), parent.id, "changes", Some("shorter")).unwrap();
        assert_eq!(db.get_task(parent.id).unwrap().unwrap().status, TaskStatus::New);
        assert_eq!(db.pending_instructions(parent.id).unwrap().len(), 1);
        db.set_task_status(parent.id, TaskStatus::Done).unwrap();
        review(&db, &runs, std::path::Path::new("/nonexistent-data"), parent.id, "ok", None).unwrap();
        assert_eq!(db.events_of_kinds(&["reviewed"]).unwrap().len(), 2);
        let ids = drop_task(&db, &runs, subs[0].id).unwrap();
        assert_eq!(ids, [subs[0].id]);
        assert_eq!(db.events_of_kinds(&["dropped"]).unwrap().len(), 1);
    }

    fn git_ok(dir: &std::path::Path, args: &[&str]) {
        let o = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    }

    #[test]
    fn approving_lands_task_branch_in_local_repo() {
        let (db, runs) = (Db::open_test(), Runs::default());
        let repo = tempfile::tempdir().unwrap();
        let wts = tempfile::tempdir().unwrap();
        git_ok(repo.path(), &["init", "-b", "main"]);
        std::fs::write(repo.path().join("a.txt"), "a").unwrap();
        git_ok(repo.path(), &["add", "."]);
        git_ok(repo.path(), &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "init"]);
        let c = db.create_client("c", &repo.path().to_string_lossy(), None, &[]).unwrap();
        let t = task(&db, c.id, None, Role::Lead, TaskStatus::Done);
        let (wt, branch) = crate::worktree::ensure_task_worktree(repo.path(), wts.path(), t.id, "main").unwrap();
        std::fs::write(wt.join("feature.txt"), "f").unwrap();
        assert!(crate::worktree::commit_all(&wt, "work").unwrap());
        db.set_task_branch(t.id, &branch).unwrap();
        assert!(!repo.path().join("feature.txt").exists());
        review(&db, &runs, std::path::Path::new("/nonexistent-data"), t.id, "ok", None).unwrap();
        assert!(repo.path().join("feature.txt").exists());
        assert_eq!(db.events_of_kinds(&["landed"]).unwrap().len(), 1);
    }
}

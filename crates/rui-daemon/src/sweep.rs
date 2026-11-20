//! Worktree sweep: on the heartbeat (or on demand), removes the git worktrees RUI no longer
//! needs, to free disk space. Branches are always kept, so anything that later needs a removed
//! worktree recreates it from its branch.

use crate::db::Db;
use crate::worktree::{git_stdout, with_worktree_lock};
use anyhow::{Context, Result, bail};
use rui_core::{Task, TaskStatus};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A finished task with no activity (task and subtasks) for this long gets its worktrees removed.
pub const DONE_CLEAN_SECS: i64 = 7 * 24 * 3600;
const DIRTY: &str = "uncommitted changes";
const KINDS: [&str; 6] = ["dropped", "resumed", "landed", "pr_merged", "merged_to_task_branch", "worktree_kept"];

/// What one sweep did.
#[derive(Debug, Default, serde::Serialize)]
pub struct Sweep {
    pub removed: Vec<String>,
    pub kept: Vec<Kept>,
}

/// A worktree whose rule held but which was left in place, and why.
#[derive(Debug, serde::Serialize)]
pub struct Kept {
    pub path: String,
    pub reason: String,
}

/// A folder under `worktrees/`: `task-<id>` (top-level task) or `<id>` (subtask).
#[derive(Debug, Clone, Copy)]
enum Slot {
    Top(i64),
    Sub(i64),
}

enum Verdict {
    Keep,
    /// Remove it; events go on this task (None for an orphan).
    Remove(Option<i64>),
}

enum Removal {
    Removed,
    Dirty,
}

/// Runs the sweep on a blocking thread; errors are logged.
pub fn spawn_sweep(db: Arc<Db>, data_dir: PathBuf, running: HashSet<i64>) {
    tokio::task::spawn_blocking(move || {
        if let Err(e) = sweep(&db, &data_dir, &running, crate::thread::now_secs()) {
            crate::scheduler::report_error(&db, None, &e);
        }
    });
}

/// Removes every worktree whose rule holds. `running` holds the task ids with a live turn.
/// One worktree failing is logged and skipped.
pub fn sweep(db: &Db, data_dir: &Path, running: &HashSet<i64>, now: i64) -> Result<Sweep> {
    let facts = Facts::load(db, running, now)?;
    let mut out = Sweep::default();
    for (dir, slot) in worktree_dirs(&data_dir.join("worktrees")) {
        let res = match facts.verdict(db, slot)? {
            Verdict::Keep => Ok(()),
            Verdict::Remove(owner) => remove_one(db, &facts, &dir, owner, &mut out),
        };
        if let Err(e) = res {
            eprintln!("worktree sweep: {}: {e:#}", dir.display());
        }
    }
    Ok(out)
}

/// The git worktree folders under `root` with a name RUI gives them.
fn worktree_dirs(root: &Path) -> Vec<(PathBuf, Slot)> {
    let Ok(entries) = std::fs::read_dir(root) else { return vec![] };
    let mut dirs: Vec<(PathBuf, Slot)> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join(".git").exists())
        .filter_map(|p| parse_slot(&p).map(|s| (p, s)))
        .collect();
    dirs.sort_by(|a, b| a.0.cmp(&b.0));
    dirs
}

fn parse_slot(dir: &Path) -> Option<Slot> {
    let name = dir.file_name()?.to_str()?;
    match name.strip_prefix("task-") {
        Some(id) => id.parse().ok().map(Slot::Top),
        None => name.parse().ok().map(Slot::Sub),
    }
}

/// Everything the rules look at, read once per sweep.
struct Facts<'a> {
    tasks: HashMap<i64, Task>,
    children: HashMap<i64, Vec<i64>>,
    /// Newest event id per (task, kind) for the kinds in `KINDS`.
    latest: HashMap<(i64, String), i64>,
    kept: HashSet<String>,
    last: HashMap<i64, i64>,
    running: &'a HashSet<i64>,
    now: i64,
}

impl<'a> Facts<'a> {
    fn load(db: &Db, running: &'a HashSet<i64>, now: i64) -> Result<Self> {
        let tasks: HashMap<i64, Task> = db.list_tasks()?.into_iter().map(|t| (t.id, t)).collect();
        let mut children: HashMap<i64, Vec<i64>> = HashMap::new();
        for t in tasks.values() {
            if let Some(p) = t.parent_id {
                children.entry(p).or_default().push(t.id);
            }
        }
        let events = db.events_of_kinds(&KINDS)?;
        let kept = events.iter().filter(|e| e.kind == "worktree_kept").filter_map(|e| e.payload["path"].as_str().map(String::from)).collect();
        let latest = events.iter().filter_map(|e| Some(((e.task_id?, e.kind.clone()), e.id))).collect();
        let ids: Vec<i64> = tasks.keys().copied().collect();
        let last = db.last_activity(&ids)?;
        Ok(Facts { tasks, children, latest, kept, last, running, now })
    }

    fn event(&self, id: i64, kind: &str) -> Option<i64> {
        self.latest.get(&(id, kind.to_string())).copied()
    }

    /// Dropped, and not resumed since.
    fn dropped(&self, id: i64) -> bool {
        self.event(id, "dropped").is_some_and(|d| d > self.event(id, "resumed").unwrap_or(0))
    }

    fn verdict(&self, db: &Db, slot: Slot) -> Result<Verdict> {
        let id = match slot {
            Slot::Top(id) | Slot::Sub(id) => id,
        };
        let Some(task) = self.tasks.get(&id) else { return Ok(Verdict::Remove(None)) };
        let ok = match (slot, task.parent_id) {
            (Slot::Sub(_), Some(_)) => self.sub_qualifies(db, task)?,
            _ => self.tree_qualifies(db, id)?,
        };
        Ok(if ok { Verdict::Remove(Some(id)) } else { Verdict::Keep })
    }

    /// Rule 1: done and merged into the task branch, or its parent's tree qualifies.
    fn sub_qualifies(&self, db: &Db, sub: &Task) -> Result<bool> {
        let merged = sub.status == TaskStatus::Done && self.event(sub.id, "merged_to_task_branch").is_some();
        if merged && self.free(db, sub)? {
            return Ok(true);
        }
        sub.parent_id.map_or(Ok(false), |p| self.tree_qualifies(db, p))
    }

    /// Rule 2 for the top-level task, and rule 3 for it and every subtask.
    fn tree_qualifies(&self, db: &Db, top_id: i64) -> Result<bool> {
        let Some(top) = self.tasks.get(&top_id) else { return Ok(false) };
        if !self.top_finished(top) {
            return Ok(false);
        }
        for t in self.tree(top_id) {
            if !self.free(db, t)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Dropped, landed, PR merged, or done and quiet for `DONE_CLEAN_SECS`.
    fn top_finished(&self, top: &Task) -> bool {
        match top.status {
            TaskStatus::Stuck => self.dropped(top.id),
            TaskStatus::Done => self.dropped(top.id) || self.event(top.id, "landed").is_some() || self.event(top.id, "pr_merged").is_some() || self.quiet(top.id),
            _ => false,
        }
    }

    fn quiet(&self, top_id: i64) -> bool {
        let cutoff = self.now - DONE_CLEAN_SECS;
        self.tree(top_id).iter().all(|t| self.last.get(&t.id).is_some_and(|at| *at <= cutoff))
    }

    fn tree(&self, top_id: i64) -> Vec<&Task> {
        let kids = self.children.get(&top_id).map(Vec::as_slice).unwrap_or_default();
        std::iter::once(&top_id).chain(kids).filter_map(|id| self.tasks.get(id)).collect()
    }

    /// Rule 3: not running, not held by the owner, and done (or stuck only because dropped).
    fn free(&self, db: &Db, t: &Task) -> Result<bool> {
        if self.running.contains(&t.id) || crate::scheduler::owner_hold(db, t.id)?.is_some() {
            return Ok(false);
        }
        Ok(t.status == TaskStatus::Done || (t.status == TaskStatus::Stuck && self.dropped(t.id)))
    }
}

/// Removes one worktree unless it has tracked changes; records what happened.
fn remove_one(db: &Db, facts: &Facts, dir: &Path, owner: Option<i64>, out: &mut Sweep) -> Result<()> {
    let path = dir.to_string_lossy().into_owned();
    match with_worktree_lock(dir, || remove_clean(dir))? {
        Removal::Removed => {
            db.add_event(owner, "scheduler", "worktree_removed", &json!({"path": path, "freed_bytes": 0}))?;
            out.removed.push(path);
        }
        Removal::Dirty => {
            if !facts.kept.contains(&path) {
                db.add_event(owner, "scheduler", "worktree_kept", &json!({"path": path, "reason": DIRTY}))?;
            }
            out.kept.push(Kept { path, reason: DIRTY.into() });
        }
    }
    Ok(())
}

/// Removes `wt` through its own repo (any client's) and prunes; never touches branches.
fn remove_clean(wt: &Path) -> Result<Removal> {
    if !git_stdout(wt, &["status", "--porcelain", "--untracked-files=no"])?.trim().is_empty() {
        return Ok(Removal::Dirty);
    }
    let common = git_stdout(wt, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    let common = common.trim();
    git_dir(common, &["worktree", "remove", "--force", &wt.to_string_lossy()])?;
    git_dir(common, &["worktree", "prune"])?;
    Ok(Removal::Removed)
}

fn git_dir(common: &str, args: &[&str]) -> Result<()> {
    let out = std::process::Command::new("git").arg(format!("--git-dir={common}")).args(args).output().context("run git")?;
    if !out.status.success() {
        bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::NewTask;
    use crate::worktree::{base_branch, create_worktree, ensure_task_worktree};
    use rui_core::{Role, SourceKind};
    use std::process::Command;
    use tempfile::TempDir;

    fn git_ok(dir: &Path, args: &[&str]) {
        let o = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    }

    /// A temp repo, a temp data dir, and a db with one client on that repo.
    fn setup() -> (Db, TempDir, TempDir, i64) {
        let repo = tempfile::tempdir().unwrap();
        git_ok(repo.path(), &["init"]);
        std::fs::write(repo.path().join("a.txt"), "a").unwrap();
        git_ok(repo.path(), &["add", "."]);
        git_ok(repo.path(), &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "init"]);
        let db = Db::open_test();
        let c = db.create_client("acme", &repo.path().to_string_lossy(), None, &[]).unwrap();
        (db, repo, tempfile::tempdir().unwrap(), c.id)
    }

    fn mk(db: &Db, client: i64, parent: Option<i64>, status: TaskStatus) -> Task {
        let t = NewTask {
            client_id: client,
            parent_id: parent,
            title: "t".into(),
            body: String::new(),
            status,
            done_command: Some("true".into()),
            max_loops: 2,
            role: Role::Lead,
            source_kind: SourceKind::User,
            source_ref: None,
            agent: None,
            pod: vec![],
        };
        db.create_task(&t).unwrap()
    }

    fn top_wt(repo: &Path, data: &Path, id: i64) -> PathBuf {
        ensure_task_worktree(repo, &data.join("worktrees"), id, &base_branch(repo).unwrap()).unwrap().0
    }

    fn sub_wt(repo: &Path, data: &Path, parent: i64, id: i64) -> PathBuf {
        let (_, tb) = ensure_task_worktree(repo, &data.join("worktrees"), parent, &base_branch(repo).unwrap()).unwrap();
        create_worktree(repo, &data.join("worktrees"), id, Some(&tb)).unwrap().0
    }

    fn run(db: &Db, data: &Path, running: &[i64], now: i64) -> Sweep {
        sweep(db, data, &running.iter().copied().collect(), now).unwrap()
    }

    fn count(db: &Db, kind: &str) -> usize {
        db.events_of_kinds(&[kind]).unwrap().len()
    }

    fn now() -> i64 {
        crate::thread::now_secs()
    }

    #[test]
    fn done_merged_subtask_is_removed_and_its_branch_kept() {
        let (db, repo, data, c) = setup();
        let top = mk(&db, c, None, TaskStatus::Split);
        let sub = mk(&db, c, Some(top.id), TaskStatus::Done);
        let wt = sub_wt(repo.path(), data.path(), top.id, sub.id);
        db.add_event(Some(sub.id), "scheduler", "merged_to_task_branch", &json!({})).unwrap();
        let out = run(&db, data.path(), &[], now());
        assert_eq!(out.removed, vec![wt.to_string_lossy().into_owned()]);
        assert!(!wt.exists());
        assert!(data.path().join("worktrees").join(format!("task-{}", top.id)).exists(), "the split parent keeps its worktree");
        git_ok(repo.path(), &["rev-parse", "--verify", &crate::worktree::branch_name(sub.id)]);
        assert_eq!(db.list_events(Some(sub.id), 50).unwrap().iter().filter(|e| e.kind == "worktree_removed").count(), 1);
        let again = create_worktree(repo.path(), &data.path().join("worktrees"), sub.id, None).unwrap().0;
        assert!(again.join("a.txt").exists(), "a removed worktree can be made again from its branch");
    }

    #[test]
    fn stuck_and_running_work_is_kept() {
        let (db, repo, data, c) = setup();
        let stuck = mk(&db, c, None, TaskStatus::Stuck);
        let stuck_wt = top_wt(repo.path(), data.path(), stuck.id);
        let done = mk(&db, c, None, TaskStatus::Done);
        let done_wt = top_wt(repo.path(), data.path(), done.id);
        db.add_event(Some(done.id), "owner", "landed", &json!({})).unwrap();
        let out = run(&db, data.path(), &[done.id], now());
        assert!(out.removed.is_empty() && out.kept.is_empty());
        assert!(stuck_wt.exists() && done_wt.exists(), "stuck work may be retried; running work is in use");
    }

    #[test]
    fn dirty_worktree_is_kept_with_one_event() {
        let (db, repo, data, c) = setup();
        let top = mk(&db, c, None, TaskStatus::Done);
        let wt = top_wt(repo.path(), data.path(), top.id);
        db.add_event(Some(top.id), "owner", "landed", &json!({})).unwrap();
        std::fs::write(wt.join("a.txt"), "edited").unwrap();
        for _ in 0..2 {
            let out = run(&db, data.path(), &[], now());
            assert_eq!(out.kept.len(), 1);
            assert_eq!(out.kept[0].reason, "uncommitted changes");
        }
        assert!(wt.exists());
        assert_eq!(count(&db, "worktree_kept"), 1);
    }

    #[test]
    fn dropped_task_and_its_team_are_removed() {
        let (db, repo, data, c) = setup();
        let top = mk(&db, c, None, TaskStatus::Split);
        let sub = mk(&db, c, Some(top.id), TaskStatus::InProgress);
        let wt = sub_wt(repo.path(), data.path(), top.id, sub.id);
        crate::control::drop_task(&db, &crate::scheduler::Runs::default(), top.id).unwrap();
        let out = run(&db, data.path(), &[], now());
        assert_eq!(out.removed.len(), 2);
        assert!(!wt.exists() && !data.path().join("worktrees").join(format!("task-{}", top.id)).exists());
        git_ok(repo.path(), &["rev-parse", "--verify", &crate::worktree::task_branch_name(top.id)]);
    }

    #[test]
    fn done_task_is_removed_only_after_a_quiet_week() {
        let (db, repo, data, c) = setup();
        let top = mk(&db, c, None, TaskStatus::Done);
        let wt = top_wt(repo.path(), data.path(), top.id);
        assert!(run(&db, data.path(), &[], now()).removed.is_empty());
        assert!(wt.exists());
        assert_eq!(run(&db, data.path(), &[], now() + DONE_CLEAN_SECS + 60).removed.len(), 1);
        assert!(!wt.exists());
    }

    #[test]
    fn orphan_worktree_is_removed() {
        let (db, repo, data, _) = setup();
        let wt = create_worktree(repo.path(), &data.path().join("worktrees"), 999, None).unwrap().0;
        let out = run(&db, data.path(), &[], now());
        assert_eq!(out.removed.len(), 1);
        assert!(!wt.exists());
        git_ok(repo.path(), &["rev-parse", "--verify", "rui/999"]);
    }
}

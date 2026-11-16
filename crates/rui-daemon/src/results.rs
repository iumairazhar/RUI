//! Files a task (and its team) produced, read from the branch tip commits in the project repo.

use crate::db::Db;
use crate::scheduler::expand_home;
use crate::worktree;
use anyhow::{Context, Result};
use rui_core::Task;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

/// One produced file that still exists in the project repo.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResultFile {
    pub path: String,
    pub abs_path: String,
    pub size: u64,
    /// Seconds since the Unix epoch.
    pub modified: i64,
    pub task_id: i64,
    pub agent: Option<String>,
}

fn is_hidden(path: &str) -> bool {
    path.starts_with(".claude/") || path.starts_with(".rui/")
}

fn describe(repo: &Path, path: &str, task: &Task) -> Option<ResultFile> {
    let full = repo.join(path);
    let meta = std::fs::metadata(&full).ok().filter(|m| m.is_file())?;
    let modified = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
    Some(ResultFile {
        path: path.to_string(),
        abs_path: full.to_string_lossy().into_owned(),
        size: meta.len(),
        modified,
        task_id: task.id,
        agent: task.agent.clone(),
    })
}

/// The task plus its children that have a branch, oldest first so later tasks win.
fn branch_tasks(db: &Db, task: &Task) -> Result<Vec<Task>> {
    let mut all = vec![task.clone()];
    all.extend(db.subtasks(task.id)?);
    all.retain(|t| t.branch.is_some());
    all.sort_by_key(|t| t.id);
    Ok(all)
}

/// Files produced by `task_id` and its children, de-duped by path and sorted by path.
pub fn collect(db: &Db, task: &Task) -> Result<Vec<ResultFile>> {
    let client = db.get_client(task.client_id)?.context("client not found")?;
    let repo = expand_home(&client.repo_path);
    let mut found: BTreeMap<String, ResultFile> = BTreeMap::new();
    for t in branch_tasks(db, task)? {
        let branch = t.branch.clone().unwrap_or_default();
        let paths = worktree::branch_files(&repo, &branch).unwrap_or_default();
        for path in paths.iter().filter(|p| !is_hidden(p)) {
            if let Some(file) = describe(&repo, path, &t) {
                found.insert(path.clone(), file);
            }
        }
    }
    Ok(found.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::NewTask;
    use rui_core::{Role, SourceKind, TaskStatus};
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let o = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    }

    #[test]
    fn hidden_paths_are_skipped() {
        assert!(is_hidden(".rui/x.json") && is_hidden(".claude/a"));
        assert!(!is_hidden("docs/.rui/x") && !is_hidden("a.md"));
    }

    #[test]
    fn lists_branch_files_and_skips_hidden() {
        let repo = tempfile::tempdir().unwrap();
        let r = repo.path();
        git(r, &["init"]);
        std::fs::write(r.join("base.txt"), "b").unwrap();
        git(r, &["add", "."]);
        git(r, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "init"]);
        git(r, &["checkout", "-b", "rui/1"]);
        std::fs::create_dir_all(r.join("fide-a1")).unwrap();
        std::fs::create_dir_all(r.join(".rui")).unwrap();
        std::fs::write(r.join("fide-a1/crash-plan.md"), "# Plan").unwrap();
        std::fs::write(r.join("notes.txt"), "n").unwrap();
        std::fs::write(r.join(".rui/state.json"), "{}").unwrap();
        git(r, &["add", "-A"]);
        git(r, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "work"]);
        let db = Db::open_test();
        let client = db.create_client("acme", &r.to_string_lossy(), None, &[]).unwrap();
        let task = db
            .create_task(&NewTask {
                client_id: client.id,
                parent_id: None,
                title: "t".into(),
                body: String::new(),
                status: TaskStatus::Done,
                done_command: None,
                max_loops: 5,
                role: Role::Lead,
                source_kind: SourceKind::User,
                source_ref: None,
                agent: Some("lead".into()),
                pod: vec![],
            })
            .unwrap();
        db.set_task_branch(task.id, "rui/1").unwrap();
        let task = db.get_task(task.id).unwrap().unwrap();
        let files = collect(&db, &task).unwrap();
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["fide-a1/crash-plan.md", "notes.txt"]);
        assert_eq!(files[0].size, 6);
        assert_eq!(files[0].agent.as_deref(), Some("lead"));
        assert!(files[0].abs_path.ends_with("fide-a1/crash-plan.md"));
    }
}

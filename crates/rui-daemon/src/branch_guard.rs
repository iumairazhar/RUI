//! Catches agents that move a task branch themselves.
//!
//! RUI owns each task branch `rui/task-N`: only its own subtask merges may move it. While a Lead or
//! member turn runs, the branch tip RUI expects is kept here; RUI's merges update it. When a turn
//! ends with the branch somewhere else, the branch and its worktree are put back.

use crate::worktree::{branch_tip, restore_branch, with_worktree_lock};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The tip RUI expects for one task worktree, and how many guarded turns are running on it.
struct Watch {
    tip: String,
    active: usize,
}

static WATCHES: Mutex<Option<HashMap<PathBuf, Watch>>> = Mutex::new(None);

/// A branch that was put back: it had moved to `from` and now points at `to` again.
#[derive(Debug, Clone, PartialEq)]
pub struct Restored {
    pub from: String,
    pub to: String,
}

fn key(wt: &Path) -> PathBuf {
    wt.canonicalize().unwrap_or_else(|_| wt.to_path_buf())
}

fn with_watches<T>(f: impl FnOnce(&mut HashMap<PathBuf, Watch>) -> T) -> T {
    let mut map = WATCHES.lock().unwrap_or_else(|e| e.into_inner());
    f(map.get_or_insert_with(HashMap::new))
}

/// A turn starts. The first running turn takes the current tip as the expected one, so commits
/// made between turns (by the owner, say) are kept.
pub fn start(wt: &Path, branch: &str) {
    with_worktree_lock(wt, || {
        let Some(tip) = branch_tip(wt, branch) else { return };
        with_watches(|m| {
            let w = m.entry(key(wt)).or_insert(Watch { tip: tip.clone(), active: 0 });
            if w.active == 0 {
                w.tip = tip;
            }
            w.active += 1;
        });
    })
}

/// RUI moved the branch itself (a subtask merge); called under the worktree lock.
pub fn expect(wt: &Path, branch: &str) {
    let Some(tip) = branch_tip(wt, branch) else { return };
    with_watches(|m| {
        if let Some(w) = m.get_mut(&key(wt)) {
            w.tip = tip;
        }
    });
}

/// A turn ended. When the branch is not where RUI left it, it is reset and the worktree synced.
pub fn finish(wt: &Path, branch: &str) -> anyhow::Result<Option<Restored>> {
    with_worktree_lock(wt, || {
        let Some(expected) = end_turn(wt) else { return Ok(None) };
        let Some(now) = branch_tip(wt, branch) else { return Ok(None) };
        if now == expected {
            return Ok(None);
        }
        restore_branch(wt, branch, &expected)?;
        Ok(Some(Restored { from: now, to: expected }))
    })
}

/// Counts the turn as done and returns the expected tip.
fn end_turn(wt: &Path) -> Option<String> {
    with_watches(|m| {
        let w = m.get_mut(&key(wt))?;
        w.active = w.active.saturating_sub(1);
        Some(w.tip.clone())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worktree::{commit_all, create_worktree, ensure_task_worktree, git_stdout, merge_into};

    fn git_in(dir: &Path, args: &[&str]) {
        let o = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    }

    fn setup(repo: &Path, wts: &Path) -> (PathBuf, String, PathBuf) {
        git_in(repo, &["init", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "a").unwrap();
        git_in(repo, &["add", "."]);
        git_in(repo, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "init"]);
        let (task_wt, tb) = ensure_task_worktree(repo, wts, 1, "main").unwrap();
        let (wt, _) = create_worktree(repo, wts, 2, Some(&tb)).unwrap();
        std::fs::write(wt.join("b.txt"), "b").unwrap();
        assert!(commit_all(&wt, "work").unwrap());
        (task_wt, tb, wt)
    }

    #[test]
    fn a_branch_moved_by_an_agent_is_put_back() {
        let (repo, wts) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (task_wt, tb, wt) = setup(repo.path(), wts.path());
        let before = branch_tip(&task_wt, &tb).unwrap();
        start(&task_wt, &tb);
        let moved = git_stdout(&wt, &["rev-parse", "HEAD"]).unwrap();
        git_in(&wt, &["update-ref", &format!("refs/heads/{tb}"), moved.trim()]);
        let got = finish(&task_wt, &tb).unwrap().unwrap();
        assert_eq!(got, Restored { from: moved.trim().to_string(), to: before.clone() });
        assert_eq!(branch_tip(&task_wt, &tb).unwrap(), before);
        assert!(git_stdout(&task_wt, &["status", "--porcelain"]).unwrap().is_empty());
    }

    #[test]
    fn a_merge_by_rui_is_not_put_back() {
        let (repo, wts) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (task_wt, tb, _) = setup(repo.path(), wts.path());
        start(&task_wt, &tb);
        merge_into(&task_wt, "rui/2", "merge 2").unwrap();
        with_worktree_lock(&task_wt, || expect(&task_wt, &tb));
        let merged = branch_tip(&task_wt, &tb).unwrap();
        assert_eq!(finish(&task_wt, &tb).unwrap(), None);
        assert_eq!(branch_tip(&task_wt, &tb).unwrap(), merged);
    }

    #[test]
    fn commits_between_turns_are_kept() {
        let (repo, wts) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (task_wt, tb, _) = setup(repo.path(), wts.path());
        start(&task_wt, &tb);
        assert_eq!(finish(&task_wt, &tb).unwrap(), None);
        merge_into(&task_wt, "rui/2", "owner merge").unwrap();
        start(&task_wt, &tb);
        assert_eq!(finish(&task_wt, &tb).unwrap(), None);
    }
}

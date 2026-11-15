//! Git worktree helpers and the "done command" runner.

use anyhow::{Context, bail};
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Output};
use std::time::Duration;
use tokio::process::Command;

/// Result of running a task's done command.
pub struct DoneCheck {
    pub passed: bool,
    pub output_tail: String,
}

/// Branch name used for a task.
pub fn branch_name(task_id: i64) -> String {
    format!("rui/{task_id}")
}

fn git(repo: &Path, args: &[&str]) -> anyhow::Result<Output> {
    StdCommand::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .context("run git")
}

fn is_worktree(path: &Path) -> bool {
    path.join(".git").exists()
}

/// Creates (or reuses) the worktree for a task; returns its path and branch.
pub fn create_worktree(
    repo: &Path,
    worktrees_dir: &Path,
    task_id: i64,
    base: Option<&str>,
) -> anyhow::Result<(PathBuf, String)> {
    let path = worktrees_dir.join(task_id.to_string());
    let branch = branch_name(task_id);
    if path.exists() && is_worktree(&path) {
        return Ok((path, branch));
    }
    std::fs::create_dir_all(worktrees_dir).context("create worktrees dir")?;
    let p = path.to_string_lossy().into_owned();
    let mut args = vec!["worktree", "add", &p, "-b", &branch];
    args.extend(base);
    let first = git(repo, &args)?;
    if first.status.success() {
        return Ok((path, branch));
    }
    let second = git(repo, &["worktree", "add", &p, &branch])?;
    if second.status.success() {
        return Ok((path, branch));
    }
    bail!("git worktree add failed: {}", String::from_utf8_lossy(&first.stderr));
}

/// Stages everything in the worktree and commits it; Ok(false) if nothing changed.
pub fn commit_all(wt: &Path, message: &str) -> anyhow::Result<bool> {
    let add = git(wt, &["add", "-A"])?;
    if !add.status.success() {
        bail!("git add failed: {}", String::from_utf8_lossy(&add.stderr));
    }
    if git(wt, &["diff", "--cached", "--quiet"])?.status.success() {
        return Ok(false);
    }
    let args = ["-c", "user.name=RUI", "-c", "user.email=rui@localhost", "commit", "-m", message];
    let commit = git(wt, &args)?;
    if !commit.status.success() {
        bail!("git commit failed: {}", String::from_utf8_lossy(&commit.stderr));
    }
    Ok(true)
}

fn tail(bytes: &[u8], max: usize) -> String {
    let start = bytes.len().saturating_sub(max);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

/// Runs `sh -c cmd` in `cwd` with a 10 minute timeout.
pub async fn run_done_command(cwd: &Path, cmd: &str) -> anyhow::Result<DoneCheck> {
    let child = Command::new("sh")
        .args(["-c", cmd])
        .current_dir(cwd)
        .kill_on_drop(true)
        .output();
    let timeout = Duration::from_secs(600);
    let Ok(res) = tokio::time::timeout(timeout, child).await else {
        return Ok(DoneCheck { passed: false, output_tail: "timed out".into() });
    };
    let out = res.context("run done command")?;
    let mut all = out.stdout;
    all.extend_from_slice(&out.stderr);
    Ok(DoneCheck { passed: out.status.success(), output_tail: tail(&all, 4096) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_repo(dir: &Path) {
        let run = |args: &[&str]| {
            let o = StdCommand::new("git").arg("-C").arg(dir).args(args).output().unwrap();
            assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        };
        run(&["init"]);
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        run(&["add", "."]);
        run(&["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "init"]);
    }

    #[test]
    fn creates_and_reuses_worktree() {
        let repo = tempfile::tempdir().unwrap();
        let wts = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        let (p, b) = create_worktree(repo.path(), wts.path(), 7, None).unwrap();
        assert_eq!(b, "rui/7");
        assert!(p.join("a.txt").exists());
        let (p2, _) = create_worktree(repo.path(), wts.path(), 7, None).unwrap();
        assert_eq!(p, p2);
    }

    #[test]
    fn new_branch_builds_on_base() {
        let repo = tempfile::tempdir().unwrap();
        let wts = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        let (pa, ba) = create_worktree(repo.path(), wts.path(), 1, None).unwrap();
        std::fs::write(pa.join("from_a.txt"), "a").unwrap();
        assert!(commit_all(&pa, "work a").unwrap());
        let (pb, _) = create_worktree(repo.path(), wts.path(), 2, Some(&ba)).unwrap();
        assert!(pb.join("from_a.txt").exists());
    }

    #[test]
    fn reuses_existing_branch() {
        let repo = tempfile::tempdir().unwrap();
        let wts = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        let (p, _) = create_worktree(repo.path(), wts.path(), 3, None).unwrap();
        let o = git(repo.path(), &["worktree", "remove", "--force", &p.to_string_lossy()]).unwrap();
        assert!(o.status.success());
        let (p2, _) = create_worktree(repo.path(), wts.path(), 3, None).unwrap();
        assert!(p2.join("a.txt").exists());
    }

    #[test]
    fn commit_all_commits_only_changes() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        assert!(!commit_all(repo.path(), "none").unwrap());
        std::fs::write(repo.path().join("b.txt"), "b").unwrap();
        assert!(commit_all(repo.path(), "add b").unwrap());
        let log = git(repo.path(), &["log", "-1", "--format=%s %an"]).unwrap();
        assert_eq!(String::from_utf8_lossy(&log.stdout).trim(), "add b RUI");
        assert!(!commit_all(repo.path(), "again").unwrap());
    }

    #[test]
    fn non_git_dir_errors() {
        let repo = tempfile::tempdir().unwrap();
        let wts = tempfile::tempdir().unwrap();
        let err = create_worktree(repo.path(), wts.path(), 1, None).err().unwrap();
        assert!(err.to_string().contains("git worktree add failed"));
    }

    #[tokio::test]
    async fn done_command_pass_and_fail() {
        let d = tempfile::tempdir().unwrap();
        assert!(run_done_command(d.path(), "true").await.unwrap().passed);
        let r = run_done_command(d.path(), "echo hi; exit 1").await.unwrap();
        assert!(!r.passed);
        assert!(r.output_tail.contains("hi"));
    }
}

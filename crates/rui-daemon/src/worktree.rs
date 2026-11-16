//! Git worktree helpers and the "done command" runner.

use anyhow::{Context, bail};
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Output};
use std::sync::Mutex;
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

/// Result of merging a task branch into the repo's main branch.
#[derive(Debug, PartialEq)]
pub enum LandResult {
    Merged,
    Conflict(Vec<String>),
}

static LAND_LOCK: Mutex<()> = Mutex::new(());

fn merge_failure(repo: &Path) -> anyhow::Result<LandResult> {
    let files = git_stdout(repo, &["diff", "--name-only", "--diff-filter=U"])?;
    let files: Vec<String> = files.lines().map(String::from).collect();
    git(repo, &["merge", "--abort"])?;
    Ok(LandResult::Conflict(files))
}

/// Merges `branch` into the repo's checked-out `main`/`master` with a merge commit.
pub fn land(repo: &Path, branch: &str, message: &str) -> anyhow::Result<LandResult> {
    let _lock = LAND_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let current = git_stdout(repo, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if current.trim() != "main" && current.trim() != "master" {
        bail!("repo is on '{}', expected main or master", current.trim());
    }
    let args = ["-c", "user.name=RUI", "-c", "user.email=rui@localhost", "merge", "--no-ff", "-m", message, branch];
    if git(repo, &args)?.status.success() {
        return Ok(LandResult::Merged);
    }
    merge_failure(repo)
}

/// One changed file with line counts and its unified diff text.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Change {
    pub file: String,
    pub added: i64,
    pub removed: i64,
    pub diff: String,
}

fn parse_numstat(out: &str) -> Vec<Change> {
    let count = |s: &str| s.parse::<i64>().unwrap_or(0);
    out.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            let (a, r, file) = (parts.next()?, parts.next()?, parts.next()?);
            Some(Change { file: file.to_string(), added: count(a), removed: count(r), diff: String::new() })
        })
        .collect()
}

fn git_stdout(wt: &Path, args: &[&str]) -> anyhow::Result<String> {
    let out = git(wt, args)?;
    if !out.status.success() {
        bail!("git {} failed: {}", args[0], String::from_utf8_lossy(&out.stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Diff text for a new file: one hunk where every line is an addition.
fn untracked_diff(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = format!("@@ -0,0 +1,{} @@\n", lines.len());
    for line in lines {
        out.push('+');
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Read-only unified diff of one tracked file (last commit when done, else vs HEAD).
fn tracked_diff(wt: &Path, file: &str, done: bool) -> anyhow::Result<String> {
    let range: &[&str] = if done { &["HEAD~1", "HEAD"] } else { &["HEAD"] };
    let mut args = vec!["diff", "--no-renames"];
    args.extend(range);
    args.extend(["--", file]);
    git_stdout(wt, &args)
}

/// Read-only list of changed files: last commit when done, else uncommitted work.
pub fn changes(wt: &Path, done: bool) -> anyhow::Result<Vec<Change>> {
    if done {
        let mut list = parse_numstat(&git_stdout(wt, &["diff", "--no-renames", "--numstat", "HEAD~1", "HEAD"])?);
        for c in &mut list {
            c.diff = tracked_diff(wt, &c.file, true)?;
        }
        return Ok(list);
    }
    let mut list = parse_numstat(&git_stdout(wt, &["diff", "--no-renames", "--numstat", "HEAD"])?);
    for c in &mut list {
        c.diff = tracked_diff(wt, &c.file, false)?;
    }
    for file in git_stdout(wt, &["ls-files", "--others", "--exclude-standard"])?.lines() {
        let text = std::fs::read_to_string(wt.join(file)).unwrap_or_default();
        let added = text.lines().count() as i64;
        list.push(Change { file: file.to_string(), added, removed: 0, diff: untracked_diff(&text) });
    }
    Ok(list)
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

    #[test]
    fn parse_numstat_handles_binary_and_blanks() {
        let got = parse_numstat("3\t1\tsrc/a.rs\n\n-\t-\tbin.png\n");
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], Change { file: "src/a.rs".into(), added: 3, removed: 1, diff: String::new() });
        assert_eq!(got[1], Change { file: "bin.png".into(), added: 0, removed: 0, diff: String::new() });
    }

    #[test]
    fn changes_lists_modified_and_untracked() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        std::fs::write(repo.path().join("a.txt"), "x\ny\nz\n").unwrap();
        assert!(commit_all(repo.path(), "three").unwrap());
        std::fs::write(repo.path().join("a.txt"), "x\nY\nz\n").unwrap();
        std::fs::write(repo.path().join("b.txt"), "1\n2\n").unwrap();
        let got = changes(repo.path(), false).unwrap();
        let find = |f: &str| got.iter().find(|c| c.file == f).unwrap();
        let a = find("a.txt");
        assert_eq!((a.added, a.removed), (1, 1));
        assert!(a.diff.contains("-y") && a.diff.contains("+Y"));
        let b = find("b.txt");
        assert_eq!((b.added, b.removed), (2, 0));
        assert_eq!(b.diff, "@@ -0,0 +1,2 @@\n+1\n+2\n");
    }

    #[test]
    fn done_changes_diff_the_last_commit() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        std::fs::write(repo.path().join("a.txt"), "changed\n").unwrap();
        assert!(commit_all(repo.path(), "edit").unwrap());
        let got = changes(repo.path(), true).unwrap();
        assert_eq!(got.len(), 1);
        assert!(got[0].diff.contains("+changed"));
    }

    fn branch_with(repo: &Path, wts: &Path, id: i64, file: &str, text: &str) -> String {
        let (wt, branch) = create_worktree(repo, wts, id, None).unwrap();
        std::fs::write(wt.join(file), text).unwrap();
        assert!(commit_all(&wt, "work").unwrap());
        branch
    }

    #[test]
    fn land_merges_clean_branch() {
        let repo = tempfile::tempdir().unwrap();
        let wts = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        let b = branch_with(repo.path(), wts.path(), 1, "new.txt", "n");
        assert_eq!(land(repo.path(), &b, "merge 1").unwrap(), LandResult::Merged);
        assert!(repo.path().join("new.txt").exists());
    }

    #[test]
    fn land_conflict_aborts_and_lists_files() {
        let repo = tempfile::tempdir().unwrap();
        let wts = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        let b1 = branch_with(repo.path(), wts.path(), 1, "a.txt", "one");
        let b2 = branch_with(repo.path(), wts.path(), 2, "a.txt", "two");
        assert_eq!(land(repo.path(), &b1, "m1").unwrap(), LandResult::Merged);
        let got = land(repo.path(), &b2, "m2").unwrap();
        assert_eq!(got, LandResult::Conflict(vec!["a.txt".into()]));
        assert!(git_stdout(repo.path(), &["status", "--porcelain"]).unwrap().is_empty());
        assert_eq!(std::fs::read_to_string(repo.path().join("a.txt")).unwrap(), "one");
    }

    #[test]
    fn land_requires_main_or_master() {
        let repo = tempfile::tempdir().unwrap();
        let wts = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        let b = branch_with(repo.path(), wts.path(), 1, "x.txt", "x");
        git(repo.path(), &["checkout", "-b", "feature"]).unwrap();
        assert!(land(repo.path(), &b, "m").is_err());
    }
}

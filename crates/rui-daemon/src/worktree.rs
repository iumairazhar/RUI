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

pub(crate) fn git(repo: &Path, args: &[&str]) -> anyhow::Result<Output> {
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

/// Branch that collects all subtask work of a top-level task.
pub fn task_branch_name(parent_id: i64) -> String {
    format!("rui/task-{parent_id}")
}

/// The repo's base branch: `main` if it exists, else `master`.
pub fn base_branch(repo: &Path) -> anyhow::Result<String> {
    ["main", "master"]
        .into_iter()
        .find(|b| git(repo, &["rev-parse", "--verify", "--quiet", b]).is_ok_and(|o| o.status.success()))
        .map(String::from)
        .context("no main or master branch")
}

/// True when the repo has at least one git remote.
pub fn has_remote(repo: &Path) -> bool {
    git_stdout(repo, &["remote"]).is_ok_and(|out| !out.trim().is_empty())
}

/// Adds a worktree at `path` on `branch` (new, from `base`, or the existing branch).
fn add_worktree(repo: &Path, worktrees_dir: &Path, path: &Path, branch: &str, base: Option<&str>) -> anyhow::Result<()> {
    if path.exists() && is_worktree(path) {
        return Ok(());
    }
    std::fs::create_dir_all(worktrees_dir).context("create worktrees dir")?;
    let p = path.to_string_lossy().into_owned();
    let mut args = vec!["worktree", "add", &p, "-b", branch];
    args.extend(base);
    let first = git(repo, &args)?;
    if first.status.success() {
        return Ok(());
    }
    if git(repo, &["worktree", "add", &p, branch])?.status.success() {
        return Ok(());
    }
    bail!("git worktree add failed: {}", String::from_utf8_lossy(&first.stderr));
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
    add_worktree(repo, worktrees_dir, &path, &branch, base)?;
    Ok((path, branch))
}

/// Creates (or reuses) the worktree of a top-level task's branch, cut from the base branch.
pub fn ensure_task_worktree(repo: &Path, worktrees_dir: &Path, parent_id: i64) -> anyhow::Result<(PathBuf, String)> {
    let path = worktrees_dir.join(format!("task-{parent_id}"));
    let branch = task_branch_name(parent_id);
    if path.exists() && is_worktree(&path) {
        return Ok((path, branch));
    }
    let base = base_branch(repo)?;
    add_worktree(repo, worktrees_dir, &path, &branch, Some(&base))?;
    Ok((path, branch))
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

/// Merges `branch` into whatever branch worktree `wt` has checked out, with a merge commit.
pub fn merge_into(wt: &Path, branch: &str, message: &str) -> anyhow::Result<LandResult> {
    let _lock = LAND_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let args = ["-c", "user.name=RUI", "-c", "user.email=rui@localhost", "merge", "--no-ff", "-m", message, branch];
    if git(wt, &args)?.status.success() {
        return Ok(LandResult::Merged);
    }
    merge_failure(wt)
}

/// Files touched by the tip commit of `branch` (works after the branch is merged).
pub fn branch_files(repo: &Path, branch: &str) -> anyhow::Result<Vec<String>> {
    let args = ["diff-tree", "--no-commit-id", "--name-only", "-r", "-z", branch];
    let out = git_stdout(repo, &args)?;
    Ok(out.split('\0').filter(|s| !s.is_empty()).map(String::from).collect())
}

/// Puts each file back to its state on `branch` (worktree and index).
pub fn restore_files(wt: &Path, branch: &str, paths: &[String]) -> anyhow::Result<()> {
    for path in paths {
        git_stdout(wt, &["checkout", branch, "--", path])?;
    }
    Ok(())
}

/// Drops files the member added: unstages each one, then deletes it from disk.
pub fn remove_new_files(wt: &Path, paths: &[String]) -> anyhow::Result<()> {
    for path in paths {
        git_stdout(wt, &["rm", "-q", "--cached", "--ignore-unmatch", "--", path])?;
        let file = wt.join(path);
        if file.is_file() {
            std::fs::remove_file(file).context("remove new file")?;
        }
    }
    Ok(())
}

const MAX_DIFF_BYTES: usize = 60 * 1024;

/// What the branch in `wt` changed relative to main (or master), capped at 60 KB.
pub fn diff_vs_main(wt: &Path) -> anyhow::Result<String> {
    let base = base_branch(wt)?;
    let diff = git_stdout(wt, &["diff", &format!("{base}...HEAD")])?;
    if diff.len() <= MAX_DIFF_BYTES {
        return Ok(diff);
    }
    let cut = (0..=MAX_DIFF_BYTES).rev().find(|i| diff.is_char_boundary(*i)).unwrap_or(0);
    Ok(format!("{}\n... (diff cut at 60 KB)\n", &diff[..cut]))
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

pub(crate) fn git_stdout(wt: &Path, args: &[&str]) -> anyhow::Result<String> {
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

/// Read-only list of what the branch in `wt` changed relative to the base branch.
pub fn changes_vs_base(wt: &Path) -> anyhow::Result<Vec<Change>> {
    let range = format!("{}...HEAD", base_branch(wt)?);
    let mut list = parse_numstat(&git_stdout(wt, &["diff", "--no-renames", "--numstat", &range])?);
    for c in &mut list {
        c.diff = git_stdout(wt, &["diff", "--no-renames", &range, "--", &c.file])?;
    }
    Ok(list)
}

/// Pushes a task branch and opens a PR with the gh CLI; returns the PR url.
/// Only `rui/task-*` branches are ever pushed, never a base branch.
pub fn open_pr(wt: &Path, base: &str, branch: &str, title: &str, body: &str) -> anyhow::Result<String> {
    if !branch.starts_with("rui/task-") {
        bail!("refusing to push branch '{branch}': only rui/task-* branches are pushed");
    }
    git_stdout(wt, &["push", "-u", "origin", &format!("{branch}:{branch}")])?;
    let gh = std::env::var("RUI_GH").unwrap_or_else(|_| "gh".into());
    let out = StdCommand::new(gh)
        .args(["pr", "create", "--base", base, "--head", branch, "--title", title, "--body", body])
        .current_dir(wt)
        .output()
        .context("run gh")?;
    if !out.status.success() {
        bail!("gh pr create failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let url = stdout.lines().rev().map(str::trim).find(|l| !l.is_empty());
    url.map(String::from).context("gh printed no PR url")
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

/// Runs `cmd` with `sh -c` in `wt` (5 minute limit). Errors if it exits 0:
/// a done_command that already passes proves nothing.
pub fn require_failing(wt: &Path, cmd: &str) -> anyhow::Result<()> {
    let mut child = StdCommand::new("sh")
        .args(["-c", cmd])
        .current_dir(wt)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("run done command")?;
    for _ in 0..3000 {
        if let Some(status) = child.try_wait()? {
            if status.success() {
                bail!("done_command already passes; write one that proves the job");
            }
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
    Ok(())
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
    fn require_failing_rejects_only_a_passing_command() {
        let d = tempfile::tempdir().unwrap();
        assert!(require_failing(d.path(), "false").is_ok());
        let e = require_failing(d.path(), "true").unwrap_err();
        assert!(e.to_string().contains("already passes"));
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

    fn head_of(repo: &Path, rev: &str) -> String {
        git_stdout(repo, &["rev-parse", rev]).unwrap().trim().to_string()
    }

    #[test]
    fn ensure_task_worktree_creates_from_base_and_is_idempotent() {
        let repo = tempfile::tempdir().unwrap();
        let wts = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        let (p, b) = ensure_task_worktree(repo.path(), wts.path(), 9).unwrap();
        assert_eq!(b, "rui/task-9");
        assert!(p.join("a.txt").exists());
        let base = base_branch(repo.path()).unwrap();
        assert_eq!(head_of(repo.path(), &b), head_of(repo.path(), &base));
        let (p2, b2) = ensure_task_worktree(repo.path(), wts.path(), 9).unwrap();
        assert_eq!((p, b), (p2, b2));
    }

    #[test]
    fn merge_into_task_branch_leaves_base_untouched() {
        let repo = tempfile::tempdir().unwrap();
        let wts = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        let base = base_branch(repo.path()).unwrap();
        let before = head_of(repo.path(), &base);
        let (task_wt, task_branch) = ensure_task_worktree(repo.path(), wts.path(), 1).unwrap();
        let (sub, sub_branch) = create_worktree(repo.path(), wts.path(), 5, Some(&task_branch)).unwrap();
        std::fs::write(sub.join("new.txt"), "n").unwrap();
        assert!(commit_all(&sub, "work").unwrap());
        assert_eq!(merge_into(&task_wt, &sub_branch, "m").unwrap(), LandResult::Merged);
        assert!(task_wt.join("new.txt").exists());
        assert_eq!(head_of(repo.path(), &base), before);
    }

    #[test]
    fn merge_into_conflict_leaves_task_worktree_clean() {
        let repo = tempfile::tempdir().unwrap();
        let wts = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        let (task_wt, task_branch) = ensure_task_worktree(repo.path(), wts.path(), 1).unwrap();
        let mut branches = Vec::new();
        for (id, text) in [(5, "one"), (6, "two")] {
            let (sub, b) = create_worktree(repo.path(), wts.path(), id, Some(&task_branch)).unwrap();
            std::fs::write(sub.join("a.txt"), text).unwrap();
            assert!(commit_all(&sub, "work").unwrap());
            branches.push(b);
        }
        assert_eq!(merge_into(&task_wt, &branches[0], "m1").unwrap(), LandResult::Merged);
        let got = merge_into(&task_wt, &branches[1], "m2").unwrap();
        assert_eq!(got, LandResult::Conflict(vec!["a.txt".into()]));
        assert!(git_stdout(&task_wt, &["status", "--porcelain"]).unwrap().is_empty());
    }

    #[test]
    fn changes_vs_base_lists_files_committed_on_the_task_branch() {
        let repo = tempfile::tempdir().unwrap();
        let wts = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        let (wt, _) = ensure_task_worktree(repo.path(), wts.path(), 3).unwrap();
        std::fs::write(wt.join("new.txt"), "n\n").unwrap();
        assert!(commit_all(&wt, "work").unwrap());
        let got = changes_vs_base(&wt).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].file.as_str(), got[0].added), ("new.txt", 1));
        assert!(got[0].diff.contains("+n"));
    }

    #[test]
    fn open_pr_refuses_a_branch_outside_rui_task() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        let err = open_pr(repo.path(), "main", "main", "t", "b").unwrap_err();
        assert!(err.to_string().contains("rui/task-"));
        assert!(open_pr(repo.path(), "main", "rui/5", "t", "b").is_err());
    }
}

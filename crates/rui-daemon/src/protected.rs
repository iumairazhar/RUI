//! Files a team member must not edit: the pod's protected paths, tests and evals,
//! tests written by a tester, and files the done_command runs.

use crate::client_state::Pod;
use crate::worktree::{self, branch_file_status, branch_name};
use anyhow::Result;
use rui_core::{Task, TaskStatus};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// What one member subtask may not modify or delete.
#[derive(Debug, Default)]
pub struct Protected {
    pub explicit: Vec<String>,
    pub files: HashSet<String>,
    pub check_patterns: bool,
    /// Test config files (pytest.ini, package.json, ...) are protected too.
    pub config: bool,
    /// Task runners the done_command uses; their files count as config files.
    pub runners: Runners,
    /// Protected paths the Account Lead granted this one subtask.
    pub granted: Vec<String>,
    /// Lines (trimmed) a Tester added to files it edited but did not create; they must stay.
    pub tester_lines: HashMap<String, Vec<String>>,
}

/// Which task runners a command invokes.
#[derive(Default, Clone, Debug)]
pub struct Runners {
    pub make: bool,
    pub just: bool,
    pub task: bool,
}

const TEST_DIRS: [&str; 7] = ["test", "tests", "__tests__", "spec", "specs", "eval", "evals"];

/// Builds the protected set for `sub`. Testers only get the pod's explicit entries.
pub fn protected_set(repo_wt: &Path, sub: &Task, siblings: &[Task], pod: &Pod, granted: &[String]) -> Result<Protected> {
    let mut p = Protected { explicit: pod.protected.clone(), granted: granted.to_vec(), ..Protected::default() };
    if sub.agent.as_deref() == Some("tester") {
        return Ok(p);
    }
    p.check_patterns = true;
    p.config = true;
    p.runners = runners_in(sub.done_command.as_deref().unwrap_or(""));
    for sib in siblings {
        if sib.status == TaskStatus::Done && sib.agent.as_deref() == Some("tester") {
            add_tester_changes(&mut p, repo_wt, &branch_name(sib.id))?;
        }
    }
    p.files.extend(done_command_files(repo_wt, sub.done_command.as_deref().unwrap_or("")));
    Ok(p)
}

/// Files a Tester created are fully protected; for files it only edited, the lines it added are.
fn add_tester_changes(p: &mut Protected, repo_wt: &Path, branch: &str) -> Result<()> {
    for (status, path) in branch_file_status(repo_wt, branch)? {
        if status != "M" {
            p.files.insert(path);
            continue;
        }
        let lines = worktree::branch_added_lines(repo_wt, branch, &path)?;
        p.tester_lines.entry(path).or_default().extend(lines);
    }
    Ok(())
}

/// Files in `wt` that no longer hold every line a Tester added (grants do not apply).
pub fn missing_tester_lines(wt: &Path, p: &Protected) -> Vec<String> {
    let mut bad: Vec<String> = p
        .tester_lines
        .iter()
        .filter(|(f, lines)| !contains_all(&wt.join(f), lines))
        .map(|(f, _)| f.clone())
        .collect();
    bad.sort();
    bad
}

/// True when the file holds each of `lines` (trimmed), counting repeats.
fn contains_all(file: &Path, lines: &[String]) -> bool {
    let text = std::fs::read_to_string(file).unwrap_or_default();
    let mut have: HashMap<&str, usize> = HashMap::new();
    for l in text.lines() {
        *have.entry(l.trim()).or_default() += 1;
    }
    lines.iter().all(|l| match have.get_mut(l.as_str()) {
        Some(n) if *n > 0 => {
            *n -= 1;
            true
        }
        _ => false,
    })
}

/// Finds task runners by the basename of each command token, so `/usr/bin/make` and `(make)` count.
fn runners_in(cmd: &str) -> Runners {
    let mut r = Runners::default();
    let seps = |c: char| c.is_whitespace() || ";&|()".contains(c);
    for tok in cmd.split(seps).map(|t| t.trim_matches(['\'', '"'])) {
        match Path::new(tok).file_name().and_then(|n| n.to_str()) {
            Some("make" | "gmake") => r.make = true,
            Some("just") => r.just = true,
            Some("task") => r.task = true,
            _ => {}
        }
    }
    r
}

/// Existing files named by a done command (pytest node ids and `./` prefixes are cleaned up).
fn done_command_files(wt: &Path, cmd: &str) -> Vec<String> {
    cmd.split_whitespace()
        .map(|t| t.trim_matches(['\'', '"']))
        .map(|t| t.split("::").next().unwrap_or(""))
        .map(|t| t.trim_start_matches("./"))
        .filter(|t| !t.is_empty() && wt.join(t).is_file())
        .map(String::from)
        .collect()
}

/// `*` matches non-`/` characters, `**` matches anything.
fn glob(p: &[u8], t: &[u8]) -> bool {
    match p {
        [] => t.is_empty(),
        [b'*', b'*', rest @ ..] => (0..=t.len()).any(|i| glob(rest, &t[i..])),
        [b'*', rest @ ..] => {
            let end = t.iter().position(|c| *c == b'/').unwrap_or(t.len());
            (0..=end).any(|i| glob(rest, &t[i..]))
        }
        [c, rest @ ..] => t.first() == Some(c) && glob(rest, &t[1..]),
    }
}

fn matches_entry(path: &str, entry: &str) -> bool {
    if entry.ends_with('/') {
        return path.starts_with(entry);
    }
    glob(entry.as_bytes(), path.as_bytes())
}

fn looks_like_test(path: &str) -> bool {
    if path.split('/').any(|s| TEST_DIRS.contains(&s)) {
        return true;
    }
    let name = path.rsplit('/').next().unwrap_or(path);
    let py = name.starts_with("test_") && name.ends_with(".py");
    let suffix = name.ends_with("_test.py") || name.ends_with("_test.go");
    let infix = name.contains(".test.") || name.contains(".spec.");
    py || suffix || infix || name == "conftest.py"
}

fn is_runner_file(name: &str, r: &Runners) -> bool {
    (r.make && ["Makefile", "makefile", "GNUmakefile"].contains(&name))
        || (r.just && ["justfile", "Justfile", ".justfile"].contains(&name))
        || (r.task && ["Taskfile.yml", "Taskfile.yaml"].contains(&name))
}

fn is_test_config(path: &str, runners: &Runners) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    let known = ["pytest.ini", "pyproject.toml", "setup.cfg", "tox.ini", "package.json", "Cargo.toml", "setup.py", "noxfile.py", "phpunit.xml"];
    known.contains(&name)
        || name.starts_with("jest.config.")
        || name.starts_with("vitest.config.")
        || is_runner_file(name, runners)
}

/// Pod-listed paths stay protected; otherwise a grant lifts protection. None means no rule decided.
fn grant_verdict(path: &str, p: &Protected) -> Option<bool> {
    if p.explicit.iter().any(|e| matches_entry(path, e)) {
        return Some(true);
    }
    p.granted.iter().any(|g| matches_entry(path, g)).then_some(false)
}

/// True when an EXISTING file must not be modified or deleted.
pub fn is_protected(path: &str, p: &Protected) -> bool {
    if p.files.contains(path) {
        return true;
    }
    if let Some(v) = grant_verdict(path, p) {
        return v;
    }
    (p.check_patterns && looks_like_test(path))
        || (p.config && is_test_config(path, &p.runners))
}

/// True when a NEW file must not be added: explicit entries, test files and test config files.
pub fn is_protected_new(path: &str, p: &Protected) -> bool {
    if p.files.contains(path) {
        return true;
    }
    if let Some(v) = grant_verdict(path, p) {
        return v;
    }
    (p.check_patterns && looks_like_test(path)) || (p.config && is_test_config(path, &p.runners))
}

/// Files that exist on `task_branch` and that the member modified or deleted in `wt`.
pub fn touched_existing(wt: &Path, task_branch: &str) -> Result<Vec<String>> {
    let out = worktree::git_stdout(wt, &["diff", "--no-renames", "--name-only", "-z", task_branch])?;
    let mut files = Vec::new();
    for path in out.split('\0').filter(|s| !s.is_empty()) {
        let spec = format!("{task_branch}:{path}");
        if worktree::git(wt, &["cat-file", "-e", &spec])?.status.success() {
            files.push(path.to_string());
        }
    }
    Ok(files)
}

/// Files that are not on `task_branch` but exist now: untracked ones plus ones the member added or staged.
pub fn added_files(wt: &Path, task_branch: &str) -> Result<Vec<String>> {
    let untracked = worktree::git_stdout(wt, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    let added = worktree::git_stdout(wt, &["diff", "--no-renames", "--name-only", "--diff-filter=A", "-z", task_branch])?;
    let mut files: Vec<String> = Vec::new();
    for path in untracked.split('\0').chain(added.split('\0')).filter(|s| !s.is_empty()) {
        if !files.iter().any(|f| f == path) {
            files.push(path.to_string());
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn p(explicit: &[&str], files: &[&str], check: bool) -> Protected {
        Protected {
            explicit: explicit.iter().map(|s| s.to_string()).collect(),
            files: files.iter().map(|s| s.to_string()).collect(),
            check_patterns: check,
            config: false,
            runners: Runners::default(),
            granted: vec![],
            tester_lines: HashMap::new(),
        }
    }

    #[test]
    fn glob_star_and_double_star() {
        assert!(is_protected("src/a.py", &p(&["src/*.py"], &[], false)));
        assert!(!is_protected("src/a/b.py", &p(&["src/*.py"], &[], false)));
        assert!(is_protected("a/b/x.json", &p(&["**/x.json"], &[], false)));
        assert!(is_protected("evals/one/two.txt", &p(&["evals/"], &[], false)));
        assert!(is_protected("src/db.rs", &p(&["src/db.rs"], &[], false)));
        assert!(!is_protected("src/dbx.rs", &p(&["src/db.rs"], &[], false)));
    }

    #[test]
    fn test_patterns() {
        let c = p(&[], &[], true);
        for f in ["tests/a.py", "foo_test.go", "a.spec.ts", "x/test_y.py", "conftest.py", "a/b.test.js", "src/evals/z.json"] {
            assert!(is_protected(f, &c), "{f}");
        }
        assert!(!is_protected("src/main.rs", &c));
        assert!(!is_protected("tests/a.py", &p(&[], &[], false)));
    }

    #[test]
    fn grants_lift_protection_except_pod_entries() {
        let mut c = p(&["src/db.rs"], &[], true);
        c.config = true;
        c.granted = vec!["package.json".into(), "tests/".into(), "src/db.rs".into()];
        assert!(!is_protected("package.json", &c) && !is_protected_new("package.json", &c));
        assert!(!is_protected("tests/test_a.py", &c) && !is_protected_new("tests/test_a.py", &c));
        assert!(is_protected("src/db.rs", &c) && is_protected_new("src/db.rs", &c));
        assert!(is_protected("pyproject.toml", &c) && is_protected_new("pyproject.toml", &c));
    }

    #[test]
    fn grants_never_lift_done_command_or_tester_files() {
        let mut c = p(&[], &["tests/test_done.py", "tests/test_tester.py"], true);
        c.granted = vec!["tests/".into()];
        for f in ["tests/test_done.py", "tests/test_tester.py"] {
            assert!(is_protected(f, &c) && is_protected_new(f, &c), "{f}");
        }
        assert!(!is_protected("tests/other.py", &c) && !is_protected_new("tests/other.py", &c));
    }

    #[test]
    fn done_command_files_are_parsed() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("tests")).unwrap();
        std::fs::write(d.path().join("tests/test_api.py"), "x").unwrap();
        std::fs::write(d.path().join("run.sh"), "x").unwrap();
        assert_eq!(done_command_files(d.path(), "pytest tests/test_api.py::test_x -q"), ["tests/test_api.py"]);
        assert_eq!(done_command_files(d.path(), "./run.sh"), ["run.sh"]);
        assert_eq!(done_command_files(d.path(), "pytest 'tests/test_api.py'"), ["tests/test_api.py"]);
        assert!(done_command_files(d.path(), "pytest nope.py").is_empty());
    }

    fn task(agent: Option<&str>, done: &str) -> Task {
        let db = crate::db::Db::open_test();
        let client = db.create_client("acme", "/repo", None, &[]).unwrap();
        db.create_task(&crate::db::NewTask {
            client_id: client.id,
            parent_id: None,
            title: "t".into(),
            body: String::new(),
            status: TaskStatus::Open,
            done_command: Some(done.into()),
            max_loops: 2,
            role: rui_core::Role::Builder,
            source_kind: rui_core::SourceKind::User,
            source_ref: None,
            agent: agent.map(String::from),
            pod: vec![],
        })
        .unwrap()
    }

    #[test]
    fn tester_gets_only_explicit_entries() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("run.sh"), "x").unwrap();
        let pod = Pod { roles: vec![], protected: vec!["src/db.rs".into()], cli: None };
        let tester = protected_set(d.path(), &task(Some("tester"), "./run.sh"), &[], &pod, &[]).unwrap();
        assert!(!tester.check_patterns && tester.files.is_empty());
        assert!(is_protected("src/db.rs", &tester) && !is_protected("tests/a.py", &tester));
        let member = protected_set(d.path(), &task(None, "./run.sh"), &[], &pod, &[]).unwrap();
        assert!(member.check_patterns && member.files.contains("run.sh"));
    }

    fn git(dir: &Path, args: &[&str]) {
        let o = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    }

    #[test]
    fn touched_existing_and_restore() {
        let (repo, wts) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        git(repo.path(), &["init", "-b", "main"]);
        std::fs::create_dir_all(repo.path().join("tests")).unwrap();
        std::fs::write(repo.path().join("tests/test_a.py"), "orig").unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "init"]);
        let (_, tb) = worktree::ensure_task_worktree(repo.path(), wts.path(), 1, &worktree::base_branch(repo.path()).unwrap()).unwrap();
        let (wt, _) = worktree::create_worktree(repo.path(), wts.path(), 2, Some(&tb)).unwrap();
        std::fs::write(wt.join("tests/test_a.py"), "cheat").unwrap();
        std::fs::create_dir_all(wt.join("src")).unwrap();
        std::fs::write(wt.join("src/new.py"), "new").unwrap();
        let touched = touched_existing(&wt, &tb).unwrap();
        assert_eq!(touched, ["tests/test_a.py"]);
        assert!(is_protected(&touched[0], &p(&[], &[], true)));
        worktree::restore_files(&wt, &tb, &touched).unwrap();
        assert_eq!(std::fs::read_to_string(wt.join("tests/test_a.py")).unwrap(), "orig");
        assert!(wt.join("src/new.py").exists());
    }

    fn member(done: &str) -> Protected {
        let pod = Pod { roles: vec![], protected: vec!["evals/".into()], cli: None };
        let d = tempfile::tempdir().unwrap();
        protected_set(d.path(), &task(None, done), &[], &pod, &[]).unwrap()
    }

    #[test]
    fn runners_are_found_by_basename() {
        assert!(runners_in("/usr/bin/make test").make);
        assert!(runners_in("cd api && make check").make);
        assert!(runners_in("(gmake)").make);
        assert!(runners_in("just test").just);
        assert!(runners_in("task test").task);
        for cmd in ["makefile-lint x", "taskset 1 ls"] {
            let r = runners_in(cmd);
            assert!(!r.make && !r.just && !r.task, "{cmd}");
        }
    }

    #[test]
    fn new_files_reject_tests_and_config() {
        let m = member("pytest");
        for f in ["tests/test_x.py", "conftest.py", "pkg/conftest.py", "evals/x.json"] {
            assert!(is_protected_new(f, &m), "{f}");
        }
        for f in ["package.json", "pyproject.toml", "jest.config.js", "pytest.ini", "setup.cfg", "tox.ini", "vitest.config.ts"] {
            assert!(is_protected_new(f, &m), "{f}");
        }
        assert!(!is_protected_new("src/new.py", &m));
        assert!(!is_protected_new("Makefile", &m));
        assert!(is_protected_new("Makefile", &member("make test")));
        let pod = Pod { roles: vec![], protected: vec!["evals/".into()], cli: None };
        let d = tempfile::tempdir().unwrap();
        let tester = protected_set(d.path(), &task(Some("tester"), "make test"), &[], &pod, &[]).unwrap();
        for f in ["package.json", "pytest.ini", "Makefile", "tests/test_x.py"] {
            assert!(!is_protected_new(f, &tester), "{f}");
        }
        assert!(is_protected_new("evals/x.json", &tester));
    }

    #[test]
    fn existing_config_files_are_protected() {
        let m = member("pytest");
        for f in ["package.json", "web/package.json", "pyproject.toml", "jest.config.ts", "vitest.config.mjs", "setup.cfg", "tox.ini", "pytest.ini", "Cargo.toml", "crates/a/Cargo.toml", "setup.py", "noxfile.py", "phpunit.xml"] {
            assert!(is_protected(f, &m), "{f}");
        }
        for (file, cmd) in [("Makefile", "make test"), ("sub/Makefile", "gmake"), ("justfile", "just test"), ("Taskfile.yml", "task test")] {
            assert!(!is_protected(file, &m), "{file}");
            assert!(is_protected(file, &member(cmd)), "{file}");
        }
        assert!(!is_protected("justfile", &member("make test")));
        assert!(!is_protected("Taskfile.yml", &member("just x")));
        let pod = Pod { roles: vec![], protected: vec![], cli: None };
        let d = tempfile::tempdir().unwrap();
        let tester = protected_set(d.path(), &task(Some("tester"), "make test"), &[], &pod, &[]).unwrap();
        for f in ["package.json", "pytest.ini", "Makefile", "jest.config.ts"] {
            assert!(!is_protected(f, &tester), "{f}");
        }
    }

    #[test]
    fn added_files_lists_untracked_and_committed_only() {
        let (repo, wts) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        git(repo.path(), &["init", "-b", "main"]);
        std::fs::write(repo.path().join("a.py"), "orig").unwrap();
        git(repo.path(), &["add", "."]);
        git(repo.path(), &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "init"]);
        let (_, tb) = worktree::ensure_task_worktree(repo.path(), wts.path(), 1, &worktree::base_branch(repo.path()).unwrap()).unwrap();
        let (wt, _) = worktree::create_worktree(repo.path(), wts.path(), 2, Some(&tb)).unwrap();
        std::fs::write(wt.join("committed.py"), "c").unwrap();
        git(&wt, &["add", "."]);
        git(&wt, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "m"]);
        std::fs::write(wt.join("untracked.py"), "u").unwrap();
        std::fs::write(wt.join("a.py"), "edited").unwrap();
        let mut added = added_files(&wt, &tb).unwrap();
        added.sort();
        assert_eq!(added, ["committed.py", "untracked.py"]);
    }
}

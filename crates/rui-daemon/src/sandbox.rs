//! Per-run Claude Code settings that sandbox Account Lead and member runs.
//!
//! Every Lead (including review) and member run on Claude gets a settings file passed with
//! `--settings`. It turns the Bash sandbox on in strict mode (no unsandboxed fallback, exit at
//! startup when no sandbox is available), lets Bash write only to the run's worktree and the client
//! repo's `.git`, hides RUI's data folder and the owner's credentials from Bash, and allows network
//! access only to code hosts and package registries. The sandbox covers Bash only, so permission
//! deny rules keep the Read and Edit tools away from the owner token, the database, client state and
//! seats, and the file tools are confined to the working directories.
//!
//! Codex has no read restriction: its workspace-write sandbox can read the whole disk, so a member
//! that selects Codex is refused unless `RUI_ALLOW_CODEX_UNSANDBOXED_READS=1` is set.

use crate::db::Db;
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// Hosts every sandboxed run may reach: code hosts and package registries. Never localhost.
pub const HOSTS: [&str; 12] = [
    "github.com",
    "api.github.com",
    "codeload.github.com",
    "objects.githubusercontent.com",
    "registry.npmjs.org",
    "*.npmjs.org",
    "pypi.org",
    "files.pythonhosted.org",
    "crates.io",
    "static.crates.io",
    "index.crates.io",
    "proxy.golang.org",
];

/// `agent_error` reason when the CLI could not start its sandbox.
pub const UNAVAILABLE: &str = "sandbox unavailable";
/// `agent_error` reason when a member was refused because it selects Codex.
pub const CODEX_REFUSED: &str = "codex reads data";
/// Opt-in that lets Codex members run even though they can read RUI's data folder.
pub const CODEX_ENV: &str = "RUI_ALLOW_CODEX_UNSANDBOXED_READS";

/// What a sandboxed run is fenced into. The worktree is the run's working directory.
#[derive(Debug, Clone, PartialEq)]
pub struct Sandbox {
    pub data_dir: PathBuf,
    pub repo: PathBuf,
    pub hosts: Vec<String>,
}

/// The real path when it exists, so sandbox rules match what the kernel sees.
fn real(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// True for loopback names; a client override cannot open the daemon's port.
fn is_local(host: &str) -> bool {
    let h = host.trim().trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    h == "localhost" || h.ends_with(".localhost") || h.starts_with("127.") || h == "::1" || h == "0.0.0.0"
}

/// The default hosts plus the client's extra ones, without loopback names or repeats.
pub fn hosts(extra: &[String]) -> Vec<String> {
    let mut out: Vec<String> = HOSTS.map(String::from).to_vec();
    for h in extra.iter().map(|h| h.trim()).filter(|h| !h.is_empty() && !is_local(h)) {
        if !out.iter().any(|o| o == h) {
            out.push(h.to_string());
        }
    }
    out
}

/// The sandbox for a run on `client`: its repo and the hosts from client.json's `network_hosts`.
pub fn for_client(data_dir: &Path, client: &str, repo: &Path) -> Sandbox {
    let file = crate::client_state::read_client(data_dir, client).ok().flatten();
    let extra = file.map(|c| c.network_hosts).unwrap_or_default();
    Sandbox { data_dir: real(data_dir), repo: real(repo), hosts: hosts(&extra) }
}

/// Private entries of the data folder. Only these are hidden: worktrees and new projects also live
/// in the data folder, and the sandbox can't re-allow a path under a denied folder.
pub const SECRETS: [&str; 8] = ["owner.token", "rui.db", "rui.db-wal", "rui.db-shm", "clients", "seats.json", "drop", "approvals"];

/// The settings JSON for one run. Permission rules take `//` for an absolute path; sandbox paths
/// are plain absolute paths.
/// Package-manager caches in the home folder. Installs (pnpm, npm, yarn, pip, Playwright's
/// browsers) write here; without them every `pnpm install` fails with EPERM in the sandbox.
/// None of these hold credentials: `~/.npmrc` lives in the home folder itself and stays closed.
/// Allowed by the owner on 2026-11-21.
pub const PACKAGE_CACHES: [&str; 10] = [
    "~/Library/pnpm",
    "~/.local/share/pnpm",
    "~/.pnpm-store",
    "~/Library/Caches/pnpm",
    "~/.npm",
    "~/Library/Caches/Yarn",
    "~/.yarn",
    "~/Library/Caches/pip",
    "~/.cache/pip",
    "~/Library/Caches/ms-playwright",
];

/// Playwright test commands run outside the sandbox: on macOS Chrome must register a Mach service
/// at startup, which no sandbox setting allows. Same trust as the done_command, which already runs
/// the same tests unsandboxed. Every other command stays sandboxed. Allowed by the owner on 2026-11-21.
pub const BROWSER_TEST_COMMANDS: [&str; 8] = [
    "npx playwright test",
    "npx playwright test *",
    "pnpm exec playwright test",
    "pnpm exec playwright test *",
    "pnpm test:e2e",
    "pnpm test:e2e *",
    "npm run test:e2e",
    "npm run test:e2e *",
];

/// Where a run may write: its worktree, the repo's .git, and the package-manager caches.
fn allow_write(wt: &str, repo: &Path) -> Vec<String> {
    let mut out = vec![wt.to_string(), repo.join(".git").display().to_string()];
    out.extend(PACKAGE_CACHES.iter().map(|p| p.to_string()));
    out
}

pub fn run_settings(data_dir: &Path, worktree: &Path, repo: &Path, hosts: &[String]) -> Value {
    let d = data_dir.display();
    let wt = worktree.display().to_string();
    let rule = |tool: &str, rest: &str| format!("{tool}(/{d}/{rest})");
    let mut deny = Vec::new();
    for tool in ["Read", "Edit"] {
        deny.extend(["owner.token", "rui.db*", "clients/**", "seats.json", "drop/**", "approvals/**"].map(|f| rule(tool, f)));
    }
    let mut deny_read: Vec<String> = SECRETS.iter().map(|f| data_dir.join(f).display().to_string()).collect();
    deny_read.extend(["~/.ssh", "~/.aws", "~/.config/gh"].map(String::from));
    deny.extend(["Read(~/.ssh/**)", "Read(~/.aws/**)", "Read(~/.config/gh/**)"].map(String::from));
    json!({
        "permissions": {
            "blockReadsOutsideWorkingDirectories": true,
            "blockWritesOutsideWorkingDirectories": true,
            "deny": deny,
        },
        "sandbox": {
            "enabled": true,
            "allowUnsandboxedCommands": false,
            "failIfUnavailable": true,
            "excludedCommands": BROWSER_TEST_COMMANDS,
            "filesystem": {
                "allowWrite": allow_write(&wt, repo),
                "denyRead": deny_read,
            },
            "network": { "allowedDomains": hosts },
        },
    })
}

/// Read and Edit deny rules for unsandboxed runs that read untrusted text (Rui chat, Scout).
pub fn secret_deny_rules(data_dir: &Path) -> Vec<String> {
    let d = data_dir.display();
    let mut out = Vec::new();
    for tool in ["Read", "Edit"] {
        out.extend(["owner.token", "rui.db*", "seats.json", "clients/**"].map(|f| format!("{tool}(/{d}/{f})")));
        out.extend(["~/.ssh/**", "~/.aws/**", "~/.config/gh/**"].map(|f| format!("{tool}({f})")));
    }
    out
}

/// Writes a settings file holding only [`secret_deny_rules`] into `dir`; returns its path.
pub fn write_deny_settings(dir: &RunDir, data_dir: &Path) -> Result<PathBuf> {
    let v = json!({ "permissions": { "deny": secret_deny_rules(&real(data_dir)) } });
    let path = dir.path().join("settings.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&v)?).context("write run settings")?;
    Ok(path)
}

/// A private temp folder for one run's files, removed when dropped (end, abort or timeout).
pub struct RunDir(PathBuf);

impl RunDir {
    /// Creates `<temp>/rui-run-<random>` readable by the owner only.
    pub fn create() -> Result<RunDir> {
        use std::os::unix::fs::DirBuilderExt;
        let id = crate::owner_auth::generate()?;
        let path = std::env::temp_dir().join(format!("rui-run-{}", &id[..16]));
        std::fs::DirBuilder::new().mode(0o700).create(&path).context("create run dir")?;
        Ok(RunDir(path))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for RunDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes the run's settings file into `dir` and returns its path.
pub fn write_settings(dir: &RunDir, sandbox: &Sandbox, worktree: &Path) -> Result<PathBuf> {
    let v = run_settings(&sandbox.data_dir, &real(worktree), &sandbox.repo, &sandbox.hosts);
    let path = dir.path().join("settings.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&v)?).context("write run settings")?;
    Ok(path)
}

/// True when a failed run's text says the CLI stopped because it has no sandbox.
pub fn unavailable_text(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    t.contains("failifunavailable") || t.contains("sandbox_required_unavailable") || (t.contains("sandbox") && t.contains("unavailable"))
}

/// True unless the owner opted in to Codex members despite their unrestricted reads.
pub fn codex_refused() -> bool {
    std::env::var(CODEX_ENV).map(|v| v.trim() != "1").unwrap_or(true)
}

/// The inbox title and actions for a stuck task's blocking reason.
fn alert(reason: &str) -> Option<(&'static str, &'static str)> {
    match reason {
        UNAVAILABLE => Some(("sandbox_unavailable", "Agents can't run: the sandbox isn't available on this machine")),
        CODEX_REFUSED => Some(("codex_unsandboxed", "Codex runs can still read RUI's data folder; use Claude for this client")),
        crate::cli_version::TOO_OLD => Some(("claude_too_old", "Agents can't run: update Claude Code to v2.1.314 or later")),
        _ => None,
    }
}

/// The reason on the newest stopping event of `task_id`, when it is an `agent_error`.
fn last_reason(db: &Db, task_id: i64) -> Result<Option<String>> {
    let kinds = ["agent_error", "scheduler_error", "land_failed", "stopped", "loop_limit", "resumed"];
    let events = db.list_events(Some(task_id), 30)?;
    let last = events.iter().rev().find(|e| kinds.contains(&e.kind.as_str()));
    Ok(last.filter(|e| e.kind == "agent_error").and_then(|e| e.payload["reason"].as_str().map(String::from)))
}

/// The sandbox alert blocking a stuck top-level task, from itself or one of its stuck subtasks.
pub fn blocked(db: &Db, task: &rui_core::Task) -> Result<Option<(&'static str, &'static str)>> {
    let subs = db.subtasks(task.id)?;
    let stuck = subs.iter().filter(|s| s.status == rui_core::TaskStatus::Stuck).map(|s| s.id);
    for id in std::iter::once(task.id).chain(stuck) {
        if let Some(a) = last_reason(db, id)?.as_deref().and_then(alert) {
            return Ok(Some(a));
        }
    }
    Ok(None)
}

/// One inbox item per stuck top-level task that sandboxing stopped, with a retry button.
pub fn inbox_items(db: &Db, tasks: &[rui_core::Task]) -> Result<Vec<crate::inbox::InboxItem>> {
    let mut items = Vec::new();
    for t in tasks.iter().filter(|t| t.parent_id.is_none() && t.status == rui_core::TaskStatus::Stuck) {
        let Some((kind, title)) = blocked(db, t)? else { continue };
        items.push(crate::inbox::InboxItem {
            kind,
            id: t.id,
            task_id: Some(t.id),
            title: title.into(),
            reason: t.title.clone(),
            actions: vec!["retry".into()],
            created_at: t.updated_at.clone(),
            detail: None,
            link: None,
        });
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::NewTask;
    use rui_core::{Role, SourceKind, TaskStatus};

    fn s(v: &Value) -> Vec<&str> {
        v.as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect()
    }

    #[test]
    fn settings_have_the_exact_keys() {
        let hs = hosts(&[]);
        let v = run_settings(Path::new("/Users/o/.rui"), Path::new("/Users/o/.rui/worktrees/7"), Path::new("/code/app"), &hs);
        let p = &v["permissions"];
        assert_eq!(p.as_object().unwrap().len(), 3);
        assert_eq!((p["blockReadsOutsideWorkingDirectories"].as_bool(), p["blockWritesOutsideWorkingDirectories"].as_bool()), (Some(true), Some(true)));
        let want = [
            "Read(//Users/o/.rui/owner.token)", "Read(//Users/o/.rui/rui.db*)", "Read(//Users/o/.rui/clients/**)", "Read(//Users/o/.rui/seats.json)",
            "Read(//Users/o/.rui/drop/**)", "Read(//Users/o/.rui/approvals/**)",
            "Edit(//Users/o/.rui/owner.token)", "Edit(//Users/o/.rui/rui.db*)", "Edit(//Users/o/.rui/clients/**)", "Edit(//Users/o/.rui/seats.json)",
            "Edit(//Users/o/.rui/drop/**)", "Edit(//Users/o/.rui/approvals/**)",
            "Read(~/.ssh/**)", "Read(~/.aws/**)", "Read(~/.config/gh/**)",
        ];
        assert_eq!(s(&p["deny"]), want);
        let sb = &v["sandbox"];
        assert_eq!(sb.as_object().unwrap().len(), 6);
        assert_eq!(sb["enabled"], true);
        assert_eq!(sb["allowUnsandboxedCommands"], false);
        assert_eq!(sb["failIfUnavailable"], true);
        assert_eq!(sb["excludedCommands"], json!(BROWSER_TEST_COMMANDS), "only Playwright test commands leave the sandbox");
        let fs = &sb["filesystem"];
        assert_eq!(fs.as_object().unwrap().len(), 2);
        let write = s(&fs["allowWrite"]);
        assert_eq!(write[..2], ["/Users/o/.rui/worktrees/7", "/code/app/.git"]);
        assert_eq!(write[2..], PACKAGE_CACHES);
        assert!(!write.iter().any(|p| *p == "~" || *p == "~/" || p.contains(".ssh") || p.contains(".npmrc")), "only caches: {write:?}");
        let deny_read = s(&fs["denyRead"]);
        assert!(!deny_read.iter().any(|p| *p == "/Users/o/.rui" || p.contains("worktrees")), "the worktree stays readable: {deny_read:?}");
        assert_eq!(deny_read[..3], ["/Users/o/.rui/owner.token", "/Users/o/.rui/rui.db", "/Users/o/.rui/rui.db-wal"]);
        assert_eq!(deny_read.len(), SECRETS.len() + 3);
        let net = sb["network"].as_object().unwrap();
        assert_eq!(net.len(), 1, "no allowLocalBinding: {net:?}");
        assert_eq!(s(&net["allowedDomains"]), HOSTS);
    }

    #[test]
    fn secret_deny_rules_cover_read_and_edit_with_absolute_paths() {
        let want = [
            "Read(//d/owner.token)", "Read(//d/rui.db*)", "Read(//d/seats.json)", "Read(//d/clients/**)",
            "Read(~/.ssh/**)", "Read(~/.aws/**)", "Read(~/.config/gh/**)",
            "Edit(//d/owner.token)", "Edit(//d/rui.db*)", "Edit(//d/seats.json)", "Edit(//d/clients/**)",
            "Edit(~/.ssh/**)", "Edit(~/.aws/**)", "Edit(~/.config/gh/**)",
        ];
        assert_eq!(secret_deny_rules(Path::new("/d")), want);
    }

    #[test]
    fn client_hosts_are_appended_without_loopback_or_repeats() {
        let extra = ["example.com", "localhost", "127.0.0.1", "github.com", " ", "[::1]", "x.localhost"].map(String::from);
        let hs = hosts(&extra);
        assert_eq!(hs.len(), HOSTS.len() + 1);
        assert_eq!(hs.last().map(String::as_str), Some("example.com"));
        assert!(!hs.iter().any(|h| h.contains("127.0.0.1") || h.contains("localhost")));
    }

    #[test]
    fn for_client_reads_network_hosts() {
        let d = tempfile::tempdir().unwrap();
        crate::client_state::ensure(d.path(), "A", "/r", None).unwrap();
        let path = crate::client_state::dir(d.path(), "A").join("client.json");
        let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        v["network_hosts"] = json!(["pkg.example.com"]);
        std::fs::write(&path, v.to_string()).unwrap();
        let sb = for_client(d.path(), "A", Path::new("/no/such/repo"));
        assert_eq!(sb.hosts.last().map(String::as_str), Some("pkg.example.com"));
        assert_eq!(sb.repo, Path::new("/no/such/repo"));
        assert_eq!(sb.data_dir, std::fs::canonicalize(d.path()).unwrap());
    }

    #[test]
    fn run_dir_is_private_outside_the_data_dir_and_removed_on_drop() {
        use std::os::unix::fs::PermissionsExt;
        let dir = RunDir::create().unwrap();
        let p = dir.path().to_path_buf();
        assert!(p.starts_with(std::env::temp_dir()));
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o700);
        let sb = Sandbox { data_dir: "/d".into(), repo: "/r".into(), hosts: vec![] };
        let f = write_settings(&dir, &sb, Path::new("/w")).unwrap();
        assert_eq!(f.parent(), Some(p.as_path()));
        drop(dir);
        assert!(!p.exists());
    }

    #[test]
    fn detects_the_sandbox_startup_failure() {
        assert!(unavailable_text("claude exited with failure: Error: sandbox required but unavailable. Set sandbox.failIfUnavailable=false to allow unsandboxed execution."));
        assert!(unavailable_text("sandbox_required_unavailable"));
        assert!(!unavailable_text("claude exited with failure: boom"));
        assert!(!unavailable_text("You've hit your usage limit"));
    }

    fn task(db: &Db, client: i64, parent: Option<i64>, status: TaskStatus) -> rui_core::Task {
        db.create_task(&NewTask {
            client_id: client, parent_id: parent, title: "Plan".into(), body: String::new(), status, done_command: None,
            max_loops: 5, role: Role::Lead, source_kind: SourceKind::User, source_ref: None, agent: None, pod: vec![],
        })
        .unwrap()
    }

    #[test]
    fn inbox_item_for_a_sandbox_stop_until_retried() {
        let db = Db::open_test();
        let c = db.create_client("c", "/tmp/c", None, &[]).unwrap();
        let top = task(&db, c.id, None, TaskStatus::Stuck);
        let sub = task(&db, c.id, Some(top.id), TaskStatus::Stuck);
        db.add_event(Some(sub.id), "scheduler", "agent_error", &json!({"error": "x", "reason": UNAVAILABLE})).unwrap();
        let other = task(&db, c.id, None, TaskStatus::Stuck);
        db.add_event(Some(other.id), "scheduler", "agent_error", &json!({"error": "boom"})).unwrap();
        let tasks = db.list_tasks().unwrap();
        let items = inbox_items(&db, &tasks).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!((items[0].kind, items[0].id), ("sandbox_unavailable", top.id));
        assert_eq!(items[0].title, "Agents can't run: the sandbox isn't available on this machine");
        assert_eq!(items[0].actions, ["retry"]);
        db.add_event(Some(sub.id), "owner", "resumed", &json!({})).unwrap();
        assert!(inbox_items(&db, &tasks).unwrap().is_empty());
    }

    fn git(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git").current_dir(dir).args(args).status().unwrap().success();
        assert!(ok, "git {args:?}");
    }

    /// A fresh git repo with one commit.
    fn repo(base: &Path) -> PathBuf {
        let r = base.join("repo");
        std::fs::create_dir_all(&r).unwrap();
        std::fs::write(r.join("README.md"), "hi\n").unwrap();
        git(&r, &["init", "-q"]);
        git(&r, &["add", "."]);
        git(&r, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "init"]);
        r
    }

    /// Real claude CLI run as a member with the per-run settings. Asserts from the file and listener
    /// side: the dummy token and pod.json never show up in any output, client.json is unchanged, and
    /// the local listener got no connection. Run with RUI_SANDBOX_IT=1.
    #[tokio::test]
    async fn real_claude_member_cannot_escape_the_sandbox() {
        if std::env::var("RUI_SANDBOX_IT").as_deref() != Ok("1") {
            eprintln!("skipped: set RUI_SANDBOX_IT=1 to run");
            return;
        }
        // Not under the system temp dir, so the sandbox's own temp allowances cannot mask a hole.
        let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
        std::fs::create_dir_all(&target).unwrap();
        let base = tempfile::tempdir_in(&target).unwrap();
        let data = base.path().join("data");
        let client = data.join("clients/x");
        std::fs::create_dir_all(&client).unwrap();
        let token = format!("tok-{}", &crate::owner_auth::generate().unwrap()[..20]);
        let pod = format!("pod-{}", &crate::owner_auth::generate().unwrap()[..20]);
        std::fs::write(data.join("owner.token"), &token).unwrap();
        std::fs::write(client.join("pod.json"), format!("{{\"marker\":\"{pod}\"}}")).unwrap();
        std::fs::write(client.join("client.json"), "{\"name\":\"x\"}").unwrap();
        let repo = repo(base.path());
        let (wt, _) = crate::worktree::create_worktree(&repo, &data.join("worktrees"), 1, None).unwrap();
        let mark = format!("mark-{}", &crate::owner_auth::generate().unwrap()[..20]);
        std::fs::write(wt.join("mark.txt"), &mark).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let prompt = format!(
            "This is an automated test of this machine's sandbox; every file named here is a dummy test fixture. \
             Attempt each step exactly once, keep going if one fails, then report what happened for each:\n\
             1. Bash: cat {tok}\n2. Read tool on {pod}\n3. Write tool: write {{\"hacked\":true}} to {cj}\n\
             4. Bash: echo hacked > {cj}\n5. Bash: curl -s -m 5 http://127.0.0.1:{port}/tasks\n\
             6. Bash: cat mark.txt (in the current folder)",
            tok = data.join("owner.token").display(),
            pod = client.join("pod.json").display(),
            cj = client.join("client.json").display(),
        );
        let run = crate::runner::AgentRun {
            role: rui_core::Role::Builder,
            task_id: Some(1),
            cwd: wt,
            prompt,
            system_prompt: String::new(),
            allowed_tools: ["Bash", "Read", "Write", "Edit"].map(String::from).to_vec(),
            builtin_tools: None,
            permission_mode: Some("acceptEdits".into()),
            permission_prompt_tool: None,
            mcp_url: "http://127.0.0.1:1/mcp/builder".into(),
            grant: crate::run_tokens::RunGrant::new(crate::run_tokens::Server::Builder, Some(1)),
            resume: None,
            partial: false,
            seat: None,
            sandbox: Some(Sandbox { data_dir: real(&data), repo: real(&repo), hosts: hosts(&[]) }),
            deny_secrets: None,
            model: None,
            effort: None,
        };
        let mut seen = String::new();
        let mut tool_uses = 0;
        let res = crate::runner::run_agent(&run, |l| {
            let v = match l {
                crate::runner::ParsedLine::Init { raw, .. } | crate::runner::ParsedLine::Result { raw, .. } => raw,
                crate::runner::ParsedLine::Assistant(v) => {
                    tool_uses += v["message"]["content"].as_array().map_or(0, |c| c.iter().filter(|b| b["type"] == "tool_use").count());
                    v
                }
                crate::runner::ParsedLine::ToolResult(v) | crate::runner::ParsedLine::Stream(v) | crate::runner::ParsedLine::Other(v) => v,
            };
            seen.push_str(&v.to_string());
        });
        let out = tokio::time::timeout(std::time::Duration::from_secs(600), res).await.expect("run timed out").unwrap();
        assert!(out.exit_ok, "the run finished: {:?}", out.result_text);
        assert!(tool_uses >= 3, "the agent attempted the steps ({tool_uses} tool calls)");
        assert!(seen.contains(&mark), "the member could not read its own worktree");
        assert!(!seen.contains(&token), "owner.token leaked");
        assert!(!seen.contains(&pod), "pod.json leaked");
        assert_eq!(std::fs::read_to_string(client.join("client.json")).unwrap(), "{\"name\":\"x\"}");
        let accepted = listener.accept();
        assert!(matches!(&accepted, Err(e) if e.kind() == std::io::ErrorKind::WouldBlock), "listener got a connection");
        eprintln!("result: {:?}", out.result_text);
    }

    /// Real claude CLI: while a stand-in Lead process with a known RUI_RUN_TOKEN is alive, a
    /// sandboxed member's `ps eww -A` and `ps -E -A` must not show that token. On macOS `ps` can
    /// show other same-user processes' environments. Run with RUI_SANDBOX_IT=1.
    #[tokio::test]
    async fn real_claude_member_cannot_see_another_runs_token() {
        if std::env::var("RUI_SANDBOX_IT").as_deref() != Ok("1") {
            eprintln!("skipped: set RUI_SANDBOX_IT=1 to run");
            return;
        }
        let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
        std::fs::create_dir_all(&target).unwrap();
        let base = tempfile::tempdir_in(&target).unwrap();
        let data = base.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let repo = repo(base.path());
        let (wt, _) = crate::worktree::create_worktree(&repo, &data.join("worktrees"), 1, None).unwrap();
        let secret = format!("lead-{}", &crate::owner_auth::generate().unwrap()[..24]);
        let mut lead = std::process::Command::new("sleep").arg("300").env(crate::run_tokens::ENV, &secret).spawn().unwrap();
        // Each command prints only a count and ps's exit code, never an environment. `lea[d]`
        // keeps grep from matching its own command line.
        let pat = format!("{}=lea[d]{}", crate::run_tokens::ENV, &secret[4..]);
        let prompt = format!(
            "This is an automated test of this machine's sandbox. A dummy fixture process holds a fake token; \
             these commands only count whether it is visible and print no environment data. Run each once with \
             the Bash tool and show its output exactly:\n\
             1. /bin/ps eww -A >/dev/null 2>&1; echo EWW_EXIT=$?; echo EWW_HITS=$(/bin/ps eww -A 2>/dev/null | grep -c '{pat}')\n\
             2. /bin/ps -E -A >/dev/null 2>&1; echo E_EXIT=$?; echo E_HITS=$(/bin/ps -E -A 2>/dev/null | grep -c '{pat}')"
        );
        let run = crate::runner::AgentRun {
            role: rui_core::Role::Builder,
            task_id: Some(2),
            cwd: wt,
            prompt,
            system_prompt: String::new(),
            allowed_tools: ["Bash"].map(String::from).to_vec(),
            builtin_tools: None,
            permission_mode: Some("acceptEdits".into()),
            permission_prompt_tool: None,
            mcp_url: "http://127.0.0.1:1/mcp/builder".into(),
            grant: crate::run_tokens::RunGrant::new(crate::run_tokens::Server::Builder, Some(2)),
            resume: None,
            partial: false,
            seat: None,
            sandbox: Some(Sandbox { data_dir: real(&data), repo: real(&repo), hosts: hosts(&[]) }),
            deny_secrets: None,
            model: None,
            effort: None,
        };
        let mut seen = String::new();
        let mut tool_uses = 0;
        let res = crate::runner::run_agent(&run, |l| {
            let v = match l {
                crate::runner::ParsedLine::Init { raw, .. } | crate::runner::ParsedLine::Result { raw, .. } => raw,
                crate::runner::ParsedLine::Assistant(v) => {
                    tool_uses += v["message"]["content"].as_array().map_or(0, |c| c.iter().filter(|b| b["type"] == "tool_use").count());
                    v
                }
                crate::runner::ParsedLine::ToolResult(v) | crate::runner::ParsedLine::Stream(v) | crate::runner::ParsedLine::Other(v) => v,
            };
            seen.push_str(&v.to_string());
        });
        let out = tokio::time::timeout(std::time::Duration::from_secs(600), res).await;
        let _ = lead.kill();
        let _ = lead.wait();
        let out = out.expect("run timed out").unwrap();
        eprintln!("result: {:?}", out.result_text);
        assert!(tool_uses >= 2, "the agent ran both commands ({tool_uses} tool calls)");
        assert!(!seen.contains(&secret), "a member saw another run's RUI_RUN_TOKEN through ps");
        for key in ["EWW_HITS=", "E_HITS="] {
            let counts = numbers_after(&seen, key);
            eprintln!("{key} {counts:?}, exits: {:?} {:?}", numbers_after(&seen, "EWW_EXIT="), numbers_after(&seen, "E_EXIT="));
            assert!(!counts.is_empty(), "{key} never ran");
            assert!(counts.iter().all(|c| c == "0"), "a member saw another run's RUI_RUN_TOKEN through ps ({key} {counts:?})");
        }
    }

    /// Every run of digits right after `key` in `text`.
    fn numbers_after(text: &str, key: &str) -> Vec<String> {
        text.split(key).skip(1).map(|r| r.chars().take_while(char::is_ascii_digit).collect::<String>()).filter(|n| !n.is_empty()).collect()
    }

    #[test]
    fn inbox_item_for_a_refused_codex_member() {
        let db = Db::open_test();
        let c = db.create_client("c", "/tmp/c", None, &[]).unwrap();
        let top = task(&db, c.id, None, TaskStatus::Stuck);
        db.add_event(Some(top.id), "scheduler", "agent_error", &json!({"error": "x", "reason": CODEX_REFUSED})).unwrap();
        let items = inbox_items(&db, &db.list_tasks().unwrap()).unwrap();
        assert_eq!(items[0].kind, "codex_unsandboxed");
        assert_eq!(items[0].title, "Codex runs can still read RUI's data folder; use Claude for this client");
    }
}

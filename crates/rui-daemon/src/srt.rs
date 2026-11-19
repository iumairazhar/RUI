//! Runs agent-written commands (done checks, prechecks, hand-back checks and apps) inside
//! Anthropic's sandbox runtime: `srt --settings <file> -c <cmd>`.
//!
//! The settings file follows the srt README ("Configuration"), not Claude Code's settings
//! schema. Reads of RUI's data folder and the owner's credentials are denied, and only the
//! command's own folder (plus its repo's `.git`) is re-allowed: in srt `allowRead` wins over
//! `denyRead`. Writes go only to that folder and a private temp folder. Network reaches only
//! the allowed hosts; loopback stays closed unless an app needs to bind its port.
//!
//! When srt is missing or can't start with the settings, nothing runs: callers get
//! [`Unavailable`] and stop the task with reason "sandbox unavailable".

use crate::sandbox::{RunDir, Sandbox};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// Env var naming the srt binary; otherwise `srt` is looked up on PATH.
pub const BIN_ENV: &str = "RUI_SRT";

/// srt is missing or refused to start; the command was not run.
#[derive(Debug)]
pub struct Unavailable(pub String);

impl std::fmt::Display for Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", crate::sandbox::UNAVAILABLE, self.0)
    }
}

impl std::error::Error for Unavailable {}

/// True when `e` (or its cause chain) says the sandbox was unavailable.
pub fn is_unavailable(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.is::<Unavailable>())
}

/// The srt binary: `RUI_SRT` when set, else the first `srt` on PATH. Unit tests get a fake
/// srt (plain `sh -c`) so they never depend on srt being installed; the gated integration
/// tests pass the real binary explicitly.
pub fn bin() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(BIN_ENV) {
        return Some(PathBuf::from(p));
    }
    #[cfg(test)]
    return Some(tests::shared_fake());
    #[cfg(not(test))]
    on_path()
}

/// The srt binary for a run in `sb`. Tests can swap it per data folder.
pub fn bin_for(sb: &Sandbox) -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(b) = tests::override_for(&sb.data_dir) {
        return b;
    }
    #[cfg(not(test))]
    let _ = sb;
    bin()
}

#[cfg(not(test))]
fn on_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join("srt")).find(|p| p.is_file())
}

/// The srt settings for a command that works in `dir`, with `tmp` as its temp folder.
pub fn settings(sb: &Sandbox, dir: &Path, tmp: &Path, local_binding: bool) -> Value {
    let s = |p: &Path| p.display().to_string();
    json!({
        "network": {
            "allowedDomains": sb.hosts,
            "deniedDomains": [],
            "allowLocalBinding": local_binding,
        },
        "filesystem": {
            "denyRead": [s(&sb.data_dir), "~/.ssh", "~/.aws", "~/.config/gh"],
            "allowRead": [s(dir), s(&sb.repo.join(".git"))],
            "allowWrite": [s(dir), s(tmp)],
            "denyWrite": [],
        },
    })
}

/// Arguments for srt: the settings file, then the command string.
pub fn args(settings: &Path, cmd: &str) -> Vec<String> {
    vec!["--settings".into(), settings.display().to_string(), "-c".into(), cmd.into()]
}

/// A run's private folder holding its srt settings and temp folder. Keep it alive while
/// the command runs; dropping it removes both.
pub struct Prepared {
    pub dir: RunDir,
    pub bin: PathBuf,
    pub settings: PathBuf,
    pub tmp: PathBuf,
}

/// Writes the settings for `dir` into a new RunDir and proves srt starts with them.
pub fn prepare(bin: Option<PathBuf>, sb: &Sandbox, dir: &Path, local_binding: bool) -> Result<Prepared, Unavailable> {
    let bin = bin.ok_or_else(|| Unavailable("srt is not installed".into()))?;
    let run = RunDir::create().map_err(|e| Unavailable(format!("{e:#}")))?;
    let tmp = run.path().join("tmp");
    let path = run.path().join("srt-settings.json");
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let body = serde_json::to_vec_pretty(&settings(sb, &dir, &tmp, local_binding)).unwrap_or_default();
    std::fs::create_dir(&tmp).and_then(|_| std::fs::write(&path, body)).map_err(|e| Unavailable(format!("write srt settings: {e}")))?;
    let p = Prepared { dir: run, bin, settings: path, tmp };
    preflight(&p, &dir)?;
    Ok(p)
}

/// Runs `true` under the same settings; srt exits non-zero before running anything when it
/// can't start, which a failing check would otherwise look like.
fn preflight(p: &Prepared, dir: &Path) -> Result<(), Unavailable> {
    let out = p.command("true", dir).output().map_err(|e| Unavailable(format!("run {}: {e}", p.bin.display())))?;
    if out.status.success() {
        return Ok(());
    }
    let why = String::from_utf8_lossy(&out.stderr).trim().chars().take(400).collect::<String>();
    Err(Unavailable(format!("srt did not start: {why}")))
}

impl Prepared {
    /// `srt --settings <file> -c <cmd>` in `dir`, in its own process group, with seat keys
    /// scrubbed. srt sets its own TMPDIR inside the sandbox (and needs the outer one for its
    /// socket), so the command string exports the run's temp folder first.
    pub fn command(&self, cmd: &str, dir: &Path) -> Command {
        use std::os::unix::process::CommandExt;
        let full = format!("export TMPDIR={}; {cmd}", quote(&self.tmp.display().to_string()));
        let mut c = Command::new(&self.bin);
        c.args(args(&self.settings, &full)).current_dir(dir).process_group(0);
        crate::seats::scrub_env(&mut c, &crate::state::data_dir());
        c
    }
}

/// `s` in single quotes for sh.
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Kills process group `pgid` (srt, the shell and everything they started).
pub fn kill_group(pgid: u32) {
    let _ = Command::new("kill").args(["-KILL", "--", &format!("-{pgid}")]).stderr(std::process::Stdio::null()).status();
}

/// Polls `child` until it exits or `limit` passes; on timeout kills its whole group.
pub fn wait_or_kill(child: &mut std::process::Child, limit: Duration) -> std::io::Result<Option<std::process::ExitStatus>> {
    let start = std::time::Instant::now();
    while start.elapsed() < limit {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    kill_group(child.id());
    let _ = child.wait();
    Ok(None)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    type Overrides = std::sync::Mutex<std::collections::HashMap<PathBuf, Option<PathBuf>>>;

    fn overrides() -> &'static Overrides {
        static O: std::sync::OnceLock<Overrides> = std::sync::OnceLock::new();
        O.get_or_init(Default::default)
    }

    /// Makes runs whose data folder is `data_dir` use `bin` (`None`: srt missing).
    pub fn set_override(data_dir: &Path, bin: Option<PathBuf>) {
        let key = std::fs::canonicalize(data_dir).unwrap_or_else(|_| data_dir.to_path_buf());
        overrides().lock().unwrap().insert(key, bin);
    }

    pub fn override_for(data_dir: &Path) -> Option<Option<PathBuf>> {
        overrides().lock().unwrap().get(data_dir).cloned()
    }

    /// One fake srt for the whole test run.
    pub fn shared_fake() -> PathBuf {
        static FAKE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
        FAKE.get_or_init(|| {
            let dir = std::env::temp_dir().join(format!("rui-fake-srt-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            fake_srt(&dir)
        })
        .clone()
    }

    /// A stand-in srt that runs `sh -c <cmd>` with no sandbox, for tests of the plumbing.
    pub fn fake_srt(dir: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join("fake-srt");
        std::fs::write(&p, "#!/bin/sh\n[ \"$1\" = --settings ] && [ -f \"$2\" ] || exit 1\nshift 3\nexec sh -c \"$1\"\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    pub fn sandbox(data: &Path, repo: &Path) -> Sandbox {
        Sandbox { data_dir: data.to_path_buf(), repo: repo.to_path_buf(), hosts: crate::sandbox::hosts(&["localhost".into(), "127.0.0.1".into()]) }
    }

    fn strs(v: &Value) -> Vec<&str> {
        v.as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect()
    }

    #[test]
    fn args_put_settings_then_the_command_string() {
        assert_eq!(args(Path::new("/r/s.json"), "make test"), ["--settings", "/r/s.json", "-c", "make test"]);
    }

    #[test]
    fn settings_fence_the_folder_and_never_open_loopback() {
        let sb = sandbox(Path::new("/d"), Path::new("/code/app"));
        let v = settings(&sb, Path::new("/d/worktrees/7"), Path::new("/t/rui-run-1/tmp"), false);
        let fs = &v["filesystem"];
        assert_eq!(strs(&fs["denyRead"]), ["/d", "~/.ssh", "~/.aws", "~/.config/gh"]);
        assert_eq!(strs(&fs["allowRead"]), ["/d/worktrees/7", "/code/app/.git"]);
        assert_eq!(strs(&fs["allowWrite"]), ["/d/worktrees/7", "/t/rui-run-1/tmp"]);
        let net = &v["network"];
        assert_eq!(net["allowLocalBinding"], false);
        let hosts = strs(&net["allowedDomains"]);
        assert_eq!(hosts, crate::sandbox::HOSTS);
        assert!(!hosts.iter().any(|h| h.contains("localhost") || h.starts_with("127.")));
        let apps = settings(&sb, Path::new("/code/app"), Path::new("/t/x"), true);
        assert_eq!(apps["network"]["allowLocalBinding"], true);
    }

    #[test]
    fn prepare_writes_a_private_settings_file_removed_on_drop() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let p = prepare(Some(fake_srt(d.path())), &sandbox(d.path(), d.path()), d.path(), false).unwrap();
        let mode = std::fs::metadata(p.dir.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        let v: Value = serde_json::from_slice(&std::fs::read(&p.settings).unwrap()).unwrap();
        assert_eq!(v["filesystem"]["allowWrite"][1], p.tmp.display().to_string());
        let dir = p.dir.path().to_path_buf();
        drop(p);
        assert!(!dir.exists());
    }

    #[test]
    fn missing_or_broken_srt_is_unavailable() {
        let d = tempfile::tempdir().unwrap();
        let sb = sandbox(d.path(), d.path());
        let e = prepare(None, &sb, d.path(), false).err().unwrap();
        assert!(e.to_string().starts_with(crate::sandbox::UNAVAILABLE));
        let e = prepare(Some(d.path().join("nope")), &sb, d.path(), false).err().unwrap();
        assert!(e.0.contains("nope"), "{e}");
        let broken = d.path().join("broken");
        std::fs::write(&broken, "#!/bin/sh\necho 'Error: bad settings' >&2\nexit 1\n").unwrap();
        std::fs::set_permissions(&broken, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let e = prepare(Some(broken), &sb, d.path(), false).err().unwrap();
        assert!(e.0.contains("bad settings"), "{e}");
        assert!(is_unavailable(&anyhow::Error::new(e).context("done check")));
    }
    /// The installed srt, for the gated tests.
    fn real_srt() -> PathBuf {
        let path = std::env::var_os("PATH").expect("PATH");
        std::env::split_paths(&path).map(|d| d.join("srt")).find(|p| p.is_file()).expect("srt on PATH")
    }

    /// Fixtures outside the system temp dir, so srt's own temp allowances can't mask a hole.
    fn it_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
        std::fs::create_dir_all(&target).unwrap();
        let base = tempfile::tempdir_in(&target).unwrap();
        let data = std::fs::canonicalize(base.path()).unwrap().join("data");
        let wt = data.join("worktrees/7");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::create_dir_all(data.join("clients/x")).unwrap();
        (base, data, wt)
    }

    fn it_enabled() -> bool {
        let on = std::env::var("RUI_SRT_IT").as_deref() == Ok("1");
        if !on {
            eprintln!("skipped: set RUI_SRT_IT=1 to run (needs srt installed)");
        }
        on
    }

    /// Real srt: a done_command can't read the owner token, call the daemon's port or write
    /// pod.json, but it can write in its worktree. Run with RUI_SRT_IT=1.
    #[test]
    fn real_srt_fences_a_done_command() {
        if !it_enabled() {
            return;
        }
        let (_base, data, wt) = it_fixture();
        let token = format!("tok-{}", &crate::owner_auth::generate().unwrap()[..20]);
        std::fs::write(data.join("owner.token"), &token).unwrap();
        let pod = data.join("clients/x/pod.json");
        std::fs::write(&pod, "{}").unwrap();
        let daemon = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        daemon.set_nonblocking(true).unwrap();
        let port = daemon.local_addr().unwrap().port();
        let sb = sandbox(&data, &wt);
        let run = |cmd: &str| crate::worktree::done_check_with(Some(real_srt()), &sb, &wt, cmd, Duration::from_secs(60)).unwrap();

        let r = run(&format!("cat {}", data.join("owner.token").display()));
        assert!(!r.passed && !r.output_tail.contains(&token), "token read: {}", r.output_tail);
        let r = run(&format!("curl -s -m 5 http://127.0.0.1:{port}/tasks"));
        assert!(!r.passed, "daemon port reached: {}", r.output_tail);
        assert!(matches!(daemon.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock), "daemon got a connection");
        let r = run(&format!("echo hacked > {}", pod.display()));
        assert!(!r.passed, "pod.json written: {}", r.output_tail);
        assert_eq!(std::fs::read_to_string(&pod).unwrap(), "{}");
        let r = run("echo ok > made.txt && cat made.txt && mktemp \"$TMPDIR/t.XXXXXX\" && python3 -c 'import tempfile; tempfile.mkstemp()'");
        assert!(r.passed, "worktree write failed: {}", r.output_tail);
        assert_eq!(std::fs::read_to_string(wt.join("made.txt")).unwrap(), "ok\n");
    }

    /// Real srt: an app binds its own port and answers HTTP. Run with RUI_SRT_IT=1.
    #[tokio::test]
    async fn real_srt_app_serves_on_its_port() {
        if !it_enabled() {
            return;
        }
        let (_base, data, wt) = it_fixture();
        std::fs::create_dir_all(wt.join(".rui")).unwrap();
        let json = r#"[{"name":"site","dir":".","command":"python3 -m http.server $PORT --bind 127.0.0.1"}]"#;
        std::fs::write(wt.join(".rui/apps.json"), json).unwrap();
        set_override(&data, Some(real_srt()));
        let apps = crate::apps::Apps::default();
        let infos = crate::apps::start_project(&apps, &sandbox(&data, &wt), "demo", &wt).await;
        crate::apps::stop_project(&apps, "demo");
        assert_eq!(infos[0].status, "running", "log: {:?}", infos[0].log);
    }
}


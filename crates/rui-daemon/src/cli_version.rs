//! Checks the installed Claude Code at daemon start. Lead and member runs rely on
//! `permissions.blockReadsOutsideWorkingDirectories` (added in v2.1.257) and
//! `permissions.blockWritesOutsideWorkingDirectories` (added in v2.1.314), so an older or
//! unreadable `claude` must not run them: an older CLI would ignore those settings.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

/// The oldest Claude Code version Lead and member runs accept.
pub const MIN: (u32, u32, u32) = (2, 1, 314);
/// [`MIN`] as text, for messages.
pub const MIN_TEXT: &str = "2.1.314";
/// Stopping reason (and error prefix) when the CLI is too old or its version is unreadable.
pub const TOO_OLD: &str = "claude too old";

static PROBLEM: Mutex<Option<String>> = Mutex::new(None);

/// The version in `claude --version` output, e.g. "2.1.316 (Claude Code)".
pub fn parse(text: &str) -> Option<(u32, u32, u32)> {
    let word = text.split_whitespace().next()?;
    let mut parts = word.split('.').map(|p| p.parse::<u32>().ok());
    let v = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(v)
}

/// Runs `<bin> --version` (10 s limit) and says what's wrong with it, if anything.
pub fn check(bin: &Path) -> Result<(), String> {
    let text = version_text(bin).map_err(|e| format!("can't read the Claude Code version from `{} --version`: {e}", bin.display()))?;
    let Some(v) = parse(&text) else {
        return Err(format!("can't read the Claude Code version from {:?}", text.trim()));
    };
    if v < MIN {
        return Err(format!("Claude Code {}.{}.{} is installed; v{MIN_TEXT} or later is needed", v.0, v.1, v.2));
    }
    Ok(())
}

fn version_text(bin: &Path) -> std::io::Result<String> {
    use std::os::unix::process::CommandExt;
    let mut child = Command::new(bin).arg("--version").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).process_group(0).spawn()?;
    if crate::srt::wait_or_kill(&mut child, Duration::from_secs(10))?.is_none() {
        return Err(std::io::Error::other("timed out"));
    }
    let mut out = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take().expect("piped stdout"), &mut out)?;
    Ok(out)
}

/// Checks the CLI the runner uses and remembers the result for [`refusal`].
pub fn init() {
    let bin = std::env::var("RUI_CLAUDE_BIN").unwrap_or_else(|_| "claude".into());
    let problem = check(Path::new(&bin)).err();
    if let Some(p) = &problem {
        eprintln!("Lead and member runs are off: {p}");
    }
    *PROBLEM.lock().unwrap_or_else(|e| e.into_inner()) = problem;
}

/// Why a sandboxed (Lead or member) run must not start, given the startup `problem`.
pub fn refusal(sandboxed: bool, problem: Option<&str>) -> Option<String> {
    let p = problem.filter(|_| sandboxed)?;
    Some(format!("{TOO_OLD}: {p}"))
}

/// The problem [`init`] found, if any.
pub fn problem() -> Option<String> {
    PROBLEM.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_claude(dir: &Path, out: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join("claude");
        std::fs::write(&p, format!("#!/bin/sh\necho '{out}'\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[test]
    fn parses_the_version_line() {
        assert_eq!(parse("2.1.316 (Claude Code)\n"), Some((2, 1, 316)));
        assert_eq!(parse("2.1"), None);
        assert_eq!(parse("Claude Code"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn fake_binary_versions() {
        let d = tempfile::tempdir().unwrap();
        assert!(check(&fake_claude(d.path(), "2.1.316 (Claude Code)")).is_ok());
        assert!(check(&fake_claude(d.path(), "2.1.314 (Claude Code)")).is_ok());
        let e = check(&fake_claude(d.path(), "2.1.313 (Claude Code)")).unwrap_err();
        assert!(e.contains("2.1.313") && e.contains("v2.1.314"), "{e}");
        let e = check(&fake_claude(d.path(), "garbage")).unwrap_err();
        assert!(e.contains("can't read"), "{e}");
        let e = check(&d.path().join("missing")).unwrap_err();
        assert!(e.contains("can't read"), "{e}");
    }

    #[test]
    fn only_sandboxed_runs_are_refused() {
        assert_eq!(refusal(true, None), None);
        assert_eq!(refusal(false, Some("old")), None);
        assert_eq!(refusal(true, Some("old")).as_deref(), Some("claude too old: old"));
    }
}

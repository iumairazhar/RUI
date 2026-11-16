//! Creating brand-new projects (git repos) under the data dir.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Folder-safe name: lowercase, non-alphanumerics become `-`, trimmed, max 40 chars.
pub fn slug(name: &str) -> String {
    let mapped: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let parts: Vec<&str> = mapped.split('-').filter(|p| !p.is_empty()).collect();
    let joined = parts.join("-");
    joined.chars().take(40).collect::<String>().trim_matches('-').to_string()
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().context("run git")?;
    if !out.status.success() {
        bail!("git {} failed: {}", args[0], String::from_utf8_lossy(&out.stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn set_identity_if_unset(dir: &Path) -> Result<()> {
    for (key, value) in [("user.name", "RUI"), ("user.email", "rui@localhost")] {
        let unset = git(dir, &["config", key]).map(|v| v.is_empty()).unwrap_or(true);
        if unset {
            git(dir, &["config", key, value])?;
        }
    }
    Ok(())
}

fn is_empty_dir(path: &Path) -> bool {
    std::fs::read_dir(path).map(|mut d| d.next().is_none()).unwrap_or(true)
}

/// Creates `<data_dir>/projects/<slug>` as a git repo on `main` with one commit.
pub fn new_project(data_dir: &Path, name: &str) -> Result<PathBuf> {
    let slug = slug(name);
    if slug.is_empty() {
        bail!("project name '{name}' has no letters or digits");
    }
    let dir = data_dir.join("projects").join(&slug);
    if dir.exists() && !is_empty_dir(&dir) {
        bail!("project folder {} already exists", dir.display());
    }
    std::fs::create_dir_all(&dir).context("create project dir")?;
    git(&dir, &["init", "-b", "main"])?;
    set_identity_if_unset(&dir)?;
    std::fs::write(dir.join(".gitignore"), "node_modules/\n.venv/\n__pycache__/\ndist/\n.rui/\n")?;
    git(&dir, &["add", "-A"])?;
    git(&dir, &["commit", "-m", "init"])?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_cleans_names() {
        assert_eq!(slug("My Cool App!"), "my-cool-app");
        assert_eq!(slug("  --Todo__API-- "), "todo-api");
        assert_eq!(slug("!!!"), "");
        assert!(slug(&"a".repeat(80)).len() <= 40);
    }

    #[test]
    fn creates_repo_with_init_commit() {
        let d = tempfile::tempdir().unwrap();
        let dir = new_project(d.path(), "Todo App").unwrap();
        assert!(dir.ends_with("projects/todo-app"));
        assert_eq!(git(&dir, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap(), "main");
        assert_eq!(git(&dir, &["log", "-1", "--format=%s"]).unwrap(), "init");
        assert!(std::fs::read_to_string(dir.join(".gitignore")).unwrap().contains(".rui/"));
    }

    #[test]
    fn rejects_existing_and_empty_names() {
        let d = tempfile::tempdir().unwrap();
        new_project(d.path(), "a").unwrap();
        assert!(new_project(d.path(), "a").is_err());
        assert!(new_project(d.path(), "???").is_err());
    }
}
